//! Docker: real when `docker` is on PATH and the daemon answers, otherwise
//! `Unavailable`. Builds `orkestr-<env>:<version>`, replaces the
//! environment's container (`orkestr-<env>`) with port mapping, waits for the
//! health URL; failed starts restore the previous image. Rollback runs the
//! earlier version's image (kept locally). **Owner: WP4.**

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget, EnvironmentId};
use orkestr_protocol::methods::deploy::RequestPreview;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::common::{self, number, sanitize, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Docker adapter.
#[derive(Debug)]
pub struct DockerAdapter {
    data_dir: PathBuf,
    probe: OnceLock<AdapterAvailability>,
}

fn probe() -> AdapterAvailability {
    let Some(docker) = orkestr_platform::process::which("docker") else {
        return AdapterAvailability::Unavailable { reason: "`docker` is not on PATH".into() };
    };
    match orkestr_platform::process::run_with_timeout(
        &docker,
        &["version", "--format", "{{.Server.Version}}"],
        Duration::from_secs(8),
    ) {
        Ok(out) if out.success => AdapterAvailability::Ready,
        Ok(_) => AdapterAvailability::Unavailable { reason: "the Docker daemon is not running".into() },
        Err(e) => AdapterAvailability::Unavailable { reason: format!("docker did not answer: {e}") },
    }
}

fn short(env: EnvironmentId) -> String {
    env.to_string().chars().filter(char::is_ascii_alphanumeric).take(12).collect::<String>().to_ascii_lowercase()
}

fn docker() -> Command {
    let mut c = Command::new("docker");
    orkestr_platform::process::isolate_command(&mut c);
    c
}

impl DockerAdapter {
    /// Adapter keeping its `current` pointers under `data_dir`.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, probe: OnceLock::new() }
    }

    fn image(env: EnvironmentId, version: &str) -> String {
        format!("orkestr-{}:{}", short(env), sanitize(version).to_ascii_lowercase())
    }

    fn container(env: EnvironmentId) -> String {
        format!("orkestr-{}", short(env))
    }

    fn current_file(&self, env: EnvironmentId) -> PathBuf {
        self.data_dir.join(env.to_string()).join("docker-current")
    }

    async fn remove_container(env: EnvironmentId, log: &LogSink) {
        let mut c = docker();
        c.args(["rm", "-f", &Self::container(env)]);
        let _ = common::run_command(
            c,
            "docker rm -f",
            &LogSink::new({
                let log = log.clone();
                move |l| {
                    if !l.contains("No such container") {
                        log.line(l)
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .await;
    }

    async fn run_image(
        &self,
        req: &DeployRequest,
        version: &str,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let env = req.environment.id;
        let host: u16 = number(req, "hostPort", 0)?;
        let cport: u16 = number(req, "containerPort", 0)?;
        if host == 0 || cport == 0 {
            return Err(DeployError::Invalid("adapter settings `hostPort` and `containerPort` are required".into()));
        }
        let previous = common::read_text(&self.current_file(env));
        Self::remove_container(env, log).await;
        let image = Self::image(env, version);
        let vars = common::command_env(req, true);
        let mut c = docker();
        c.args(["run", "-d", "--name", &Self::container(env), "-p", &format!("127.0.0.1:{host}:{cport}")]);
        for k in vars.keys() {
            // Values come from the docker CLI's environment, never the command line.
            c.arg("-e").arg(k);
        }
        c.envs(&vars).arg(&image);
        common::run_command(c, &format!("docker run {image}"), log, cancel).await?;
        let url = format!("http://127.0.0.1:{host}");
        let health = format!("{url}{}", setting(req, "healthPath").unwrap_or("/"));
        let timeout = Duration::from_secs(number(req, "healthTimeoutSecs", 60u64)?);
        if let Err(e) = common::wait_healthy(&health, timeout, log, cancel).await {
            let mut logs = docker();
            logs.args(["logs", "--tail", "40", &Self::container(env)]);
            let _ = common::run_command(logs, "docker logs", log, &CancellationToken::new()).await;
            Self::remove_container(env, log).await;
            if let Some(prev) = previous.filter(|p| p != version) {
                log.line(&format!("restoring previous version {prev}"));
                let mut c = docker();
                c.args(["run", "-d", "--name", &Self::container(env), "-p", &format!("127.0.0.1:{host}:{cport}")]);
                for k in vars.keys() {
                    c.arg("-e").arg(k);
                }
                c.envs(&vars).arg(Self::image(env, &prev));
                let _ = common::run_command(c, "docker run (previous)", log, &CancellationToken::new()).await;
            }
            return Err(e);
        }
        common::write_atomic(&self.current_file(env), version)?;
        Ok(DeployOutcome { url: Some(format!("{url}/")), version: version.to_owned(), provider_ref: Some(image) })
    }
}

#[async_trait]
impl DeployAdapter for DockerAdapter {
    fn id(&self) -> &str {
        "docker"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Docker
    }

    fn availability(&self) -> AdapterAvailability {
        self.probe.get_or_init(probe).clone()
    }

    fn settings_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["hostPort", "containerPort"],
            "properties": {
                "context": { "type": "string", "default": "." },
                "dockerfile": { "type": "string", "default": "Dockerfile" },
                "hostPort": { "type": "string", "description": "Port on 127.0.0.1" },
                "containerPort": { "type": "string" },
                "healthPath": { "type": "string", "default": "/" },
                "healthTimeoutSecs": { "type": "string", "default": "60" }
            }
        })
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let image = Self::image(req.environment.id, &req.version);
        let mut warnings = Vec::new();
        if let AdapterAvailability::Unavailable { reason } = self.availability() {
            warnings.push(format!("Docker is unavailable: {reason}"));
        }
        for k in ["hostPort", "containerPort"] {
            if setting(req, k).is_none() {
                warnings.push(format!("adapter setting `{k}` is required"));
            }
        }
        let ctx = setting(req, "context").unwrap_or(".");
        let df = setting(req, "dockerfile").unwrap_or("Dockerfile");
        let host = setting(req, "hostPort").unwrap_or("?");
        let cport = setting(req, "containerPort").unwrap_or("?");
        let name = Self::container(req.environment.id);
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps: vec![
                format!("Build image {image}"),
                format!("Replace container {name}"),
                format!("Wait for http://127.0.0.1:{host}{}", setting(req, "healthPath").unwrap_or("/")),
            ],
            requests: vec![
                RequestPreview {
                    method: "exec".into(),
                    target: format!("docker build -t {image} -f {df} {ctx}"),
                    body: None,
                },
                RequestPreview { method: "exec".into(), target: format!("docker rm -f {name}"), body: None },
                RequestPreview {
                    method: "exec".into(),
                    target: format!("docker run -d --name {name} -p 127.0.0.1:{host}:{cport} {image}"),
                    body: Some(
                        "environment variables and secrets passed by name (-e KEY), values never on the command line"
                            .into(),
                    ),
                },
            ],
            warnings,
        })
    }

    async fn deploy(
        &self,
        req: &DeployRequest,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        if let AdapterAvailability::Unavailable { reason } = self.availability() {
            return Err(DeployError::Unavailable(reason));
        }
        let ctx = common::under_source(req, setting(req, "context").unwrap_or("."))?;
        let df = ctx.join(setting(req, "dockerfile").unwrap_or("Dockerfile"));
        let image = Self::image(req.environment.id, &req.version);
        let mut c = docker();
        c.arg("build").arg("-t").arg(&image).arg("-f").arg(&df).arg(&ctx);
        common::run_command(c, &format!("docker build -t {image}"), &log, &cancel).await?;
        self.run_image(req, &req.version, &log, &cancel).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        _target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        if let AdapterAvailability::Unavailable { reason } = self.availability() {
            return Err(DeployError::Unavailable(reason));
        }
        self.run_image(req, &req.version, &log, &cancel).await
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("Docker rollback needs the environment settings (use rollback_to)".into()))
    }
}
