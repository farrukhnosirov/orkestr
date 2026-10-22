//! Static site: optional build command, copy the output directory to a
//! versioned directory `<data_dir>/<env>/versions/<version>`, serve it with a
//! small local HTTP server on the configured port, health check, atomic
//! switch of `current`. **Owner: WP4.**

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::Response;
use orkestr_domain::{Deployment, DeploymentTarget, EnvironmentId};
use orkestr_protocol::methods::deploy::RequestPreview;
use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::common::{self, number, sanitize, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

#[derive(Clone)]
struct SiteRoot {
    dir: Arc<RwLock<PathBuf>>,
    spa: bool,
}

struct Server {
    port: u16,
    root: SiteRoot,
    stop: CancellationToken,
}

/// Static site adapter.
pub struct StaticSiteAdapter {
    data_dir: PathBuf,
    servers: Mutex<HashMap<EnvironmentId, Server>>,
}

impl std::fmt::Debug for StaticSiteAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticSiteAdapter").field("data_dir", &self.data_dir).finish_non_exhaustive()
    }
}

impl Drop for StaticSiteAdapter {
    fn drop(&mut self) {
        for s in self.servers.get_mut().values() {
            s.stop.cancel();
        }
    }
}

impl StaticSiteAdapter {
    /// Adapter keeping versions under `data_dir`.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, servers: Mutex::new(HashMap::new()) }
    }

    fn env_dir(&self, env: EnvironmentId) -> PathBuf {
        self.data_dir.join(env.to_string())
    }

    fn version_dir(&self, env: EnvironmentId, version: &str) -> PathBuf {
        self.env_dir(env).join("versions").join(sanitize(version))
    }

    /// Version currently live in `env` (the `current` pointer).
    #[must_use]
    pub fn current_version(&self, env: EnvironmentId) -> Option<String> {
        common::read_text(&self.env_dir(env).join("current"))
    }

    /// Port the environment is served on, if serving.
    #[must_use]
    pub fn serving_port(&self, env: EnvironmentId) -> Option<u16> {
        self.servers.lock().get(&env).map(|s| s.port)
    }

    /// Serves `dir` for `env` on `port` (starting / moving the server as needed).
    async fn serve(&self, env: EnvironmentId, port: u16, dir: PathBuf, spa: bool) -> Result<(), DeployError> {
        {
            let mut servers = self.servers.lock();
            if let Some(s) = servers.get(&env) {
                if s.port == port && s.root.spa == spa {
                    *s.root.dir.write() = dir;
                    return Ok(());
                }
                s.stop.cancel();
                servers.remove(&env);
            }
        }
        let listener = bind_with_retry(port).await?;
        let root = SiteRoot { dir: Arc::new(RwLock::new(dir)), spa };
        let stop = CancellationToken::new();
        let app = axum::Router::new().fallback(serve_file).with_state(root.clone());
        let s2 = stop.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).with_graceful_shutdown(async move { s2.cancelled().await }).await;
        });
        self.servers.lock().insert(env, Server { port, root, stop });
        Ok(())
    }

    /// Stops serving `env`.
    pub fn stop(&self, env: EnvironmentId) {
        if let Some(s) = self.servers.lock().remove(&env) {
            s.stop.cancel();
        }
    }

    /// Re-serves the `current` version after a restart (best effort).
    pub async fn restore(&self, env: EnvironmentId, settings: &std::collections::BTreeMap<String, String>) {
        let Some(port) = settings.get("port").and_then(|p| p.trim().parse::<u16>().ok()) else { return };
        let Some(version) = self.current_version(env) else { return };
        let dir = self.version_dir(env, &version);
        if dir.is_dir() {
            let spa = settings.get("spaFallback").is_some_and(|v| v == "true");
            if let Err(e) = self.serve(env, port, dir, spa).await {
                tracing::warn!(error = %e, %env, "could not restore static site server");
            }
        }
    }

    async fn switch_and_check(
        &self,
        req: &DeployRequest,
        version: &str,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let env = req.environment.id;
        let port: u16 = number(req, "port", 0)?;
        if port == 0 {
            return Err(DeployError::Invalid("adapter setting `port` is required (1-65535)".into()));
        }
        let spa = setting(req, "spaFallback") == Some("true");
        let dir = self.version_dir(env, version);
        if !dir.is_dir() {
            return Err(DeployError::NotFound(format!("version {version} is not kept for this environment")));
        }
        let previous = self.current_version(env);
        log.line(&format!("switching current → {version} (port {port})"));
        common::write_atomic(&self.env_dir(env).join("current"), version)?;
        self.serve(env, port, dir, spa).await?;
        let health_path = setting(req, "healthPath").unwrap_or("/");
        let url = format!("http://127.0.0.1:{port}");
        let timeout = Duration::from_secs(number(req, "healthTimeoutSecs", 30u64)?);
        if let Err(e) = common::wait_healthy(&format!("{url}{health_path}"), timeout, log, cancel).await {
            match previous {
                Some(prev) if prev != version && self.version_dir(env, &prev).is_dir() => {
                    log.line(&format!("restoring previous version {prev}"));
                    common::write_atomic(&self.env_dir(env).join("current"), &prev)?;
                    self.serve(env, port, self.version_dir(env, &prev), spa).await?;
                }
                _ => {}
            }
            return Err(e);
        }
        Ok(DeployOutcome { url: Some(format!("{url}/")), version: version.to_owned(), provider_ref: None })
    }
}

async fn bind_with_retry(port: u16) -> Result<tokio::net::TcpListener, DeployError> {
    let mut last = None;
    for _ in 0..20 {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(DeployError::Failed(format!(
        "cannot listen on 127.0.0.1:{port}: {}",
        last.map_or_else(String::new, |e| e.to_string())
    )))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Maps a URL path to a file under `root` (no traversal).
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path)?;
    let mut p = root.to_path_buf();
    for seg in decoded.split('/').filter(|s| !s.is_empty()) {
        if seg == "." || seg == ".." || seg.contains(['\\', ':', '\0']) {
            return None;
        }
        p.push(seg);
    }
    if p.is_dir() {
        p.push("index.html");
    }
    Some(p)
}

async fn serve_file(State(root): State<SiteRoot>, uri: Uri) -> Response {
    let base = root.dir.read().clone();
    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("not found"))
            .unwrap_or_default()
    };
    let Some(mut path) = resolve(&base, uri.path()) else { return not_found() };
    if !path.is_file() {
        if root.spa {
            path = base.join("index.html");
        } else {
            return not_found();
        }
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&path))
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(bytes))
            .unwrap_or_default(),
        Err(_) => not_found(),
    }
}

#[async_trait]
impl DeployAdapter for StaticSiteAdapter {
    fn id(&self) -> &str {
        "staticSite"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::StaticSite
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Ready
    }

    fn settings_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["port"],
            "properties": {
                "buildCommand": { "type": "string", "description": "Optional build command run in the source directory" },
                "outputDir": { "type": "string", "default": "dist", "description": "Directory to publish (relative to the source directory)" },
                "port": { "type": "string", "description": "Local port the site is served on (127.0.0.1)" },
                "healthPath": { "type": "string", "default": "/" },
                "healthTimeoutSecs": { "type": "string", "default": "30" },
                "spaFallback": { "type": "string", "enum": ["true", "false"], "default": "false" }
            }
        })
    }

    fn commands(&self, req: &DeployRequest) -> Vec<String> {
        setting(req, "buildCommand").map(str::to_owned).into_iter().collect()
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let mut steps = Vec::new();
        let mut requests = Vec::new();
        let mut warnings = Vec::new();
        if let Some(b) = setting(req, "buildCommand") {
            steps.push(format!("Run build command `{b}`"));
            requests.push(RequestPreview { method: "exec".into(), target: b.to_owned(), body: None });
        }
        let out = setting(req, "outputDir").unwrap_or("dist");
        steps.push(format!("Copy `{out}` to versions/{}", sanitize(&req.version)));
        let port = setting(req, "port");
        if port.and_then(|p| p.parse::<u16>().ok()).is_none() {
            warnings.push("adapter setting `port` is missing or invalid".into());
        }
        let port = port.unwrap_or("?");
        steps.push(format!("Serve on http://127.0.0.1:{port} and switch `current` atomically"));
        steps.push(format!("Health check GET http://127.0.0.1:{port}{}", setting(req, "healthPath").unwrap_or("/")));
        requests.push(RequestPreview {
            method: "GET".into(),
            target: format!("http://127.0.0.1:{port}{}", setting(req, "healthPath").unwrap_or("/")),
            body: None,
        });
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps,
            requests,
            warnings,
        })
    }

    async fn deploy(
        &self,
        req: &DeployRequest,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        if let Some(b) = setting(req, "buildCommand") {
            common::run_shell(b, &req.source_dir, &common::command_env(req, false), &log, &cancel).await?;
        }
        let out = common::under_source(req, setting(req, "outputDir").unwrap_or("dist"))?;
        let dst = self.version_dir(req.environment.id, &req.version);
        log.line(&format!("copying {} → versions/{}", out.display(), sanitize(&req.version)));
        let n = common::copy_dir(out, dst, Vec::new()).await?;
        log.line(&format!("copied {n} files"));
        if cancel.is_cancelled() {
            return Err(DeployError::Cancelled);
        }
        self.switch_and_check(req, &req.version, &log, &cancel).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        _target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        self.switch_and_check(req, &req.version, &log, &cancel).await
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("static site rollback needs the environment settings (use rollback_to)".into()))
    }

    fn release(&self, env: EnvironmentId) {
        self.stop(env);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_refuses_traversal() {
        let root = Path::new("/srv/site");
        assert!(resolve(root, "/../etc/passwd").is_none());
        assert!(resolve(root, "/a/%2e%2e/b").is_none());
        assert!(resolve(root, "/a%5cb").is_none());
        assert_eq!(resolve(root, "/a/b.css").unwrap(), root.join("a").join("b.css"));
    }
}
