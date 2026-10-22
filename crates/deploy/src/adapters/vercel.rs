//! Vercel REST API (base URL override `ORKESTR_VERCEL_API`; request-shape
//! verified). Uploads files (`POST /v2/files` with `x-vercel-digest`),
//! creates a deployment (`POST /v13/deployments`), polls `readyState`,
//! cancels (`PATCH /v12/deployments/{id}/cancel`) and rolls back
//! (`POST /v9/projects/{project}/rollback/{deploymentId}`). **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget, EnvironmentKind};
use reqwest::Method;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::cloud::{self, Api, Body, Poll};
use super::common::{self, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

const SECRET: &str = "VERCEL_TOKEN";

/// Vercel adapter.
#[derive(Debug, Default, Clone)]
pub struct VercelAdapter {
    base: Option<String>,
}

impl VercelAdapter {
    /// Adapter with an explicit API base (tests); env vars / default otherwise.
    #[must_use]
    pub fn with_base(base: Option<String>) -> Self {
        Self { base }
    }

    fn base(&self) -> String {
        cloud::base_url("VERCEL", self.base.as_deref(), Some("https://api.vercel.com")).unwrap_or_default()
    }

    fn query(req: &DeployRequest) -> String {
        setting(req, "teamId").map_or_else(String::new, |t| format!("?teamId={}", cloud::encode_path(t)))
    }

    fn target_of(req: &DeployRequest) -> Value {
        match setting(req, "target") {
            Some("preview") => Value::Null,
            Some(t) => Value::String(t.to_owned()),
            None if req.environment.kind == EnvironmentKind::Production => Value::String("production".into()),
            None => Value::Null,
        }
    }

    async fn wait_ready(
        api: &Api,
        req: &DeployRequest,
        id: &str,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<Value, DeployError> {
        let q = Self::query(req);
        let r = cloud::poll(req, log, cancel, || async {
            let d =
                api.send(Method::GET, &format!("/v13/deployments/{}{q}", cloud::encode_path(id)), Body::None).await?;
            match cloud::s(&d, "readyState").unwrap_or("UNKNOWN") {
                "READY" => Ok(Poll::Ready(d)),
                "ERROR" => Err(DeployError::Failed(format!(
                    "Vercel deployment {id} failed: {}",
                    cloud::s(&d, "errorMessage").unwrap_or("build error")
                ))),
                "CANCELED" => Err(DeployError::Failed(format!("Vercel deployment {id} was canceled"))),
                other => Ok(Poll::Pending(other.to_owned())),
            }
        })
        .await;
        if matches!(r, Err(DeployError::Cancelled)) {
            log.line("cancelling the Vercel deployment");
            let _ = api
                .send(Method::PATCH, &format!("/v12/deployments/{}/cancel{q}", cloud::encode_path(id)), Body::None)
                .await;
        }
        r
    }
}

fn url_of(d: &Value) -> Option<String> {
    cloud::s(d, "url").map(|u| if u.starts_with("http") { u.to_owned() } else { format!("https://{u}") })
}

#[async_trait]
impl DeployAdapter for VercelAdapter {
    fn id(&self) -> &str {
        "vercel"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Vercel
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Ready
    }

    fn required_secret(&self) -> Option<&str> {
        Some(SECRET)
    }

    fn verification(&self) -> &str {
        "requestShape"
    }

    fn settings_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["project"],
            "properties": {
                "project": { "type": "string", "description": "Vercel project name" },
                "teamId": { "type": "string" },
                "dir": { "type": "string", "default": ".", "description": "Directory uploaded (relative to the source directory)" },
                "buildCommand": { "type": "string", "description": "Optional local build before upload" },
                "target": { "type": "string", "description": "production | preview | <custom environment>" },
                "pollIntervalMs": { "type": "string", "default": "2000" },
                "timeoutSecs": { "type": "string", "default": "900" }
            }
        })
    }

    fn commands(&self, req: &DeployRequest) -> Vec<String> {
        setting(req, "buildCommand").map(str::to_owned).into_iter().collect()
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let base = self.base();
        let q = Self::query(req);
        let project = setting(req, "project").unwrap_or("<project>");
        let mut warnings: Vec<String> = cloud::credential_warning(req, SECRET).into_iter().collect();
        if setting(req, "project").is_none() {
            warnings.push("adapter setting `project` is required".into());
        }
        let mut requests = Vec::new();
        if let Some(b) = setting(req, "buildCommand") {
            requests.push(orkestr_protocol::methods::deploy::RequestPreview {
                method: "exec".into(),
                target: b.into(),
                body: None,
            });
        }
        requests.push(cloud::preview(
            "POST",
            format!("{base}/v2/files"),
            Some("x-vercel-digest: <sha1> (one per file)".into()),
        ));
        requests.push(cloud::preview(
            "POST",
            format!("{base}/v13/deployments{q}"),
            Some(json!({ "name": project, "project": project, "target": Self::target_of(req), "files": "[{file, sha, size}]" }).to_string()),
        ));
        requests.push(cloud::preview("GET", format!("{base}/v13/deployments/<id>{q}"), None));
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps: vec![
                format!("Upload `{}` to Vercel (file digests)", setting(req, "dir").unwrap_or(".")),
                format!("Create a deployment of project `{project}`"),
                "Poll readyState until READY".into(),
            ],
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
        let api = Api::new(self.base(), req, SECRET, false)?;
        let project = common::required(req, "project")?;
        if let Some(b) = setting(req, "buildCommand") {
            common::run_shell(b, &req.source_dir, &common::command_env(req, false), &log, &cancel).await?;
        }
        let files = common::collect_files(common::under_source(req, setting(req, "dir").unwrap_or("."))?).await?;
        log.line(&format!("uploading {} files to Vercel", files.len()));
        let q = Self::query(req);
        for f in &files {
            if cancel.is_cancelled() {
                return Err(DeployError::Cancelled);
            }
            api.send(
                Method::POST,
                &format!("/v2/files{q}"),
                Body::Bytes(f.bytes.clone(), vec![("x-vercel-digest", f.sha1.clone())]),
            )
            .await?;
        }
        let body = json!({
            "name": project,
            "project": project,
            "target": Self::target_of(req),
            "files": files.iter().map(|f| json!({ "file": f.rel, "sha": f.sha1, "size": f.bytes.len() })).collect::<Vec<_>>(),
            "meta": { "orkestrVersion": req.version, "orkestrDeploymentId": req.deployment_id.to_string() },
            "projectSettings": { "framework": Value::Null },
        });
        let created = api.send(Method::POST, &format!("/v13/deployments{q}"), Body::Json(body, None)).await?;
        let id = cloud::s(&created, "id")
            .ok_or_else(|| DeployError::Failed("Vercel returned no deployment id".into()))?
            .to_owned();
        log.line(&format!("Vercel deployment {id} created"));
        let ready = Self::wait_ready(&api, req, &id, &log, &cancel).await?;
        Ok(DeployOutcome {
            url: url_of(&ready).or_else(|| url_of(&created)),
            version: req.version.clone(),
            provider_ref: Some(id),
        })
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let api = Api::new(self.base(), req, SECRET, false)?;
        let project = common::required(req, "project")?;
        let id = target
            .provider_ref
            .clone()
            .ok_or_else(|| DeployError::Invalid("the earlier deployment has no Vercel deployment id".into()))?;
        let q = Self::query(req);
        log.line(&format!("rolling back project {project} to Vercel deployment {id}"));
        api.send(
            Method::POST,
            &format!("/v9/projects/{}/rollback/{}{q}", cloud::encode_path(project), cloud::encode_path(&id)),
            Body::None,
        )
        .await?;
        let ready = Self::wait_ready(&api, req, &id, &log, &cancel).await?;
        Ok(DeployOutcome { url: url_of(&ready), version: req.version.clone(), provider_ref: Some(id) })
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("Vercel rollback needs the environment settings (use rollback_to)".into()))
    }
}
