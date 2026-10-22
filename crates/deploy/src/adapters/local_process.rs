//! Local process: snapshot the working directory into
//! `<data_dir>/<env>/versions/<version>`, optional build there, stop the
//! previous version, start the command (supervised, restarted on crash),
//! wait for the health URL; failed starts restore the previous version.
//! Versions are kept for rollback. **Owner: WP4.**

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget, EnvironmentId};
use orkestr_protocol::methods::deploy::RequestPreview;
use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::common::{self, number, sanitize, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Lines of process output kept per environment.
const TAIL: usize = 200;
/// Restarts after an unexpected exit before giving up.
const MAX_RESTARTS: u32 = 3;

#[derive(Default)]
struct Output {
    sink: Mutex<Option<LogSink>>,
    tail: Mutex<VecDeque<String>>,
}

impl Output {
    fn line(&self, l: &str) {
        let l = orkestr_security::redact::redact(l);
        if let Some(s) = self.sink.lock().as_ref() {
            s.line(&l);
        }
        let mut t = self.tail.lock();
        if t.len() >= TAIL {
            t.pop_front();
        }
        t.push_back(l);
    }
}

struct Proc {
    version: String,
    stop: CancellationToken,
    done: watch::Receiver<bool>,
    exited: watch::Receiver<Option<String>>,
    output: Arc<Output>,
    spec: Spec,
}

#[derive(Clone)]
struct Spec {
    dir: PathBuf,
    command: String,
    env: BTreeMap<String, String>,
}

/// Local process adapter.
pub struct LocalProcessAdapter {
    data_dir: PathBuf,
    procs: Mutex<HashMap<EnvironmentId, Proc>>,
}

impl std::fmt::Debug for LocalProcessAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProcessAdapter").field("data_dir", &self.data_dir).finish_non_exhaustive()
    }
}

impl Drop for LocalProcessAdapter {
    fn drop(&mut self) {
        for p in self.procs.get_mut().values() {
            p.stop.cancel();
        }
    }
}

fn spawn_supervised(spec: Spec, output: Arc<Output>) -> Proc {
    let stop = CancellationToken::new();
    let (done_tx, done) = watch::channel(false);
    let (exit_tx, exited) = watch::channel(None);
    let (s, out, sp) = (stop.clone(), Arc::clone(&output), spec.clone());
    tokio::spawn(async move {
        let mut restarts = 0u32;
        loop {
            let mut cmd = common::shell(&sp.command);
            cmd.current_dir(&sp.dir).envs(&sp.env);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = exit_tx.send(Some(format!("cannot start `{}`: {e}", sp.command)));
                    break;
                }
            };
            let o = Arc::clone(&out);
            let _ = common::pipe_output(&mut child, &LogSink::new(move |l| o.line(l)));
            tokio::select! {
                status = child.wait() => {
                    let msg = match status {
                        Ok(st) => format!("process exited ({})", st.code().map_or("signal".into(), |c| c.to_string())),
                        Err(e) => format!("process wait failed: {e}"),
                    };
                    out.line(&format!("[orkestr] {msg}"));
                    let _ = exit_tx.send(Some(msg));
                    if s.is_cancelled() || restarts >= MAX_RESTARTS {
                        break;
                    }
                    restarts += 1;
                    let backoff = Duration::from_secs(1 << restarts.min(4));
                    out.line(&format!("[orkestr] restarting in {}s ({restarts}/{MAX_RESTARTS})", backoff.as_secs()));
                    tokio::select! {
                        () = tokio::time::sleep(backoff) => {}
                        () = s.cancelled() => break,
                    }
                }
                () = s.cancelled() => {
                    common::kill_child(&mut child).await;
                    break;
                }
            }
        }
        let _ = done_tx.send(true);
    });
    Proc { version: String::new(), stop, done, exited, output, spec }
}

async fn stop_proc(p: Proc) {
    p.stop.cancel();
    let mut done = p.done;
    let _ = tokio::time::timeout(Duration::from_secs(15), done.wait_for(|d| *d)).await;
}

impl LocalProcessAdapter {
    /// Adapter keeping versions under `data_dir`.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, procs: Mutex::new(HashMap::new()) }
    }

    fn env_dir(&self, env: EnvironmentId) -> PathBuf {
        self.data_dir.join(env.to_string())
    }

    fn version_dir(&self, env: EnvironmentId, version: &str) -> PathBuf {
        self.env_dir(env).join("versions").join(sanitize(version))
    }

    /// Version running in `env`, if any.
    #[must_use]
    pub fn running_version(&self, env: EnvironmentId) -> Option<String> {
        self.procs.lock().get(&env).map(|p| p.version.clone())
    }

    /// Last lines of the running process output (redacted).
    #[must_use]
    pub fn output_tail(&self, env: EnvironmentId) -> Vec<String> {
        self.procs.lock().get(&env).map(|p| p.output.tail.lock().iter().cloned().collect()).unwrap_or_default()
    }

    /// Stops the process of `env`.
    pub async fn stop(&self, env: EnvironmentId) {
        let p = self.procs.lock().remove(&env);
        if let Some(p) = p {
            stop_proc(p).await;
        }
    }

    fn spec(&self, req: &DeployRequest, version: &str) -> Result<Spec, DeployError> {
        let command = common::required(req, "command")?.to_owned();
        let mut env = common::command_env(req, true);
        env.insert("ORKESTR_DEPLOY_VERSION".into(), version.to_owned());
        if let Some(p) = setting(req, "port") {
            env.insert("PORT".into(), p.to_owned());
        }
        Ok(Spec { dir: self.version_dir(req.environment.id, version), command, env })
    }

    async fn start_and_check(
        &self,
        req: &DeployRequest,
        version: &str,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let env = req.environment.id;
        let spec = self.spec(req, version)?;
        if !spec.dir.is_dir() {
            return Err(DeployError::NotFound(format!("version {version} is not kept for this environment")));
        }
        let previous = self.procs.lock().remove(&env);
        let previous_spec = previous.as_ref().map(|p| (p.version.clone(), p.spec.clone()));
        if let Some(p) = previous {
            log.line(&format!("stopping previous version {}", p.version));
            stop_proc(p).await;
        }
        log.line(&format!("$ {} (in versions/{})", spec.command, sanitize(version)));
        let output = Arc::new(Output::default());
        *output.sink.lock() = Some(log.clone());
        let mut proc = spawn_supervised(spec, Arc::clone(&output));
        proc.version = version.to_owned();
        let health = setting(req, "healthUrl").map(str::to_owned);
        let timeout = Duration::from_secs(number(req, "healthTimeoutSecs", 30u64)?);
        let mut exited = proc.exited.clone();
        let checked = async {
            match &health {
                Some(url) => common::wait_healthy(url, timeout, log, cancel).await,
                None => {
                    log.line("no healthUrl configured: checking the process stays up for 2s");
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(2)) => Ok(()),
                        () = cancel.cancelled() => Err(DeployError::Cancelled),
                    }
                }
            }
        };
        let result = tokio::select! {
            r = checked => r,
            r = exited.wait_for(Option::is_some) => {
                let msg = r.ok().and_then(|v| v.as_ref().cloned()).unwrap_or_else(|| "process exited".into());
                Err(DeployError::Failed(format!("process stopped before it became healthy: {msg}")))
            }
        };
        *output.sink.lock() = None;
        match result {
            Ok(()) => {
                common::write_atomic(&self.env_dir(env).join("current"), version)?;
                self.procs.lock().insert(env, proc);
                Ok(DeployOutcome {
                    url: health.or_else(|| setting(req, "url").map(str::to_owned)),
                    version: version.to_owned(),
                    provider_ref: None,
                })
            }
            Err(e) => {
                stop_proc(proc).await;
                if let Some((v, spec)) = previous_spec {
                    log.line(&format!("restoring previous version {v}"));
                    let mut p = spawn_supervised(spec, Arc::new(Output::default()));
                    p.version = v;
                    self.procs.lock().insert(env, p);
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl DeployAdapter for LocalProcessAdapter {
    fn id(&self) -> &str {
        "localProcess"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::LocalProcess
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Ready
    }

    fn settings_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": "Start command (run in the version directory)" },
                "workdir": { "type": "string", "default": ".", "description": "Directory snapshotted per version (relative to the source directory)" },
                "buildCommand": { "type": "string", "description": "Optional build command run in the version directory" },
                "healthUrl": { "type": "string", "description": "URL that answers 2xx when the process is healthy" },
                "port": { "type": "string", "description": "Exported as PORT" },
                "healthTimeoutSecs": { "type": "string", "default": "30" },
                "snapshotExclude": { "type": "string", "default": "target", "description": "Comma-separated directory names not copied" }
            }
        })
    }

    fn commands(&self, req: &DeployRequest) -> Vec<String> {
        ["buildCommand", "command"].iter().filter_map(|k| setting(req, k)).map(str::to_owned).collect()
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let mut steps = Vec::new();
        let mut requests = Vec::new();
        let mut warnings = Vec::new();
        let workdir = setting(req, "workdir").unwrap_or(".");
        steps.push(format!("Snapshot `{workdir}` to versions/{}", sanitize(&req.version)));
        if let Some(b) = setting(req, "buildCommand") {
            steps.push(format!("Run build command `{b}`"));
            requests.push(RequestPreview { method: "exec".into(), target: b.to_owned(), body: None });
        }
        steps.push("Stop the previous version".into());
        match setting(req, "command") {
            Some(c) => {
                steps.push(format!("Start `{c}` (supervised)"));
                requests.push(RequestPreview { method: "exec".into(), target: c.to_owned(), body: None });
            }
            None => warnings.push("adapter setting `command` is required".into()),
        }
        match setting(req, "healthUrl") {
            Some(h) => {
                steps.push(format!("Wait for {h} to answer 2xx"));
                requests.push(RequestPreview { method: "GET".into(), target: h.to_owned(), body: None });
            }
            None => warnings.push("no healthUrl: only checks that the process stays up".into()),
        }
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
        common::required(req, "command")?;
        let src = common::under_source(req, setting(req, "workdir").unwrap_or("."))?;
        let excludes: Vec<String> = setting(req, "snapshotExclude")
            .unwrap_or("target")
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let dst = self.version_dir(req.environment.id, &req.version);
        log.line(&format!("snapshot {} → versions/{}", src.display(), sanitize(&req.version)));
        let n = common::copy_dir(src, dst.clone(), excludes).await?;
        log.line(&format!("copied {n} files"));
        if let Some(b) = setting(req, "buildCommand") {
            common::run_shell(b, &dst, &common::command_env(req, true), &log, &cancel).await?;
        }
        if cancel.is_cancelled() {
            return Err(DeployError::Cancelled);
        }
        self.start_and_check(req, &req.version, &log, &cancel).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        _target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        self.start_and_check(req, &req.version, &log, &cancel).await
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("local process rollback needs the environment settings (use rollback_to)".into()))
    }

    fn release(&self, env: EnvironmentId) {
        if let Some(p) = self.procs.lock().remove(&env) {
            p.stop.cancel();
        }
    }
}
