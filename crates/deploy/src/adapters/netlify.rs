//! Netlify REST API (base URL override `ORKESTR_NETLIFY_API`; request-shape
//! verified). File-digest deploy: `POST /api/v1/sites/{site}/deploys` with
//! `{files: {"/path": sha1}}`, upload the `required` files with
//! `PUT /api/v1/deploys/{id}/files/{path}`, poll `state`, cancel
//! (`POST /api/v1/deploys/{id}/cancel`), rollback
//! (`POST /api/v1/sites/{site}/deploys/{id}/restore`). **Owner: WP4.**

use std::collections::BTreeSet;

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use reqwest::Method;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::cloud::{self, Api, Body, Poll};
use super::common::{self, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

const SECRET: &str = "NETLIFY_AUTH_TOKEN";

/// Netlify adapter.
#[derive(Debug, Default, Clone)]
pub struct NetlifyAdapter {
    base: Option<String>,
}

impl NetlifyAdapter {
    /// Adapter with an explicit API base (tests); env vars / default otherwise.
    #[must_use]
    pub fn with_base(base: Option<String>) -> Self {
        Self { base }
    }

    fn base(&self) -> String {
        cloud::base_url("NETLIFY", self.base.as_deref(), Some("https://api.netlify.com")).unwrap_or_default()
    }

    async fn wait_ready(
        api: &Api,
        req: &DeployRequest,
        id: &str,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<Value, DeployError> {
        let r = cloud::poll(req, log, cancel, || async {
            let d = api.send(Method::GET, &format!("/api/v1/deploys/{}", cloud::encode_path(id)), Body::None).await?;
            match cloud::s(&d, "state").unwrap_or("unknown") {
                "ready" => Ok(Poll::Ready(d)),
                "error" => Err(DeployError::Failed(format!(
                    "Netlify deploy {id} failed: {}",
                    cloud::s(&d, "error_message").unwrap_or("error")
                ))),
                other => Ok(Poll::Pending(other.to_owned())),
            }
        })
        .await;
        if matches!(r, Err(DeployError::Cancelled)) {
            log.line("cancelling the Netlify deploy");
            let _ =
                api.send(Method::POST, &format!("/api/v1/deploys/{}/cancel", cloud::encode_path(id)), Body::None).await;
        }
        r
    }
}

fn url_of(d: &Value) -> Option<String> {
    ["ssl_url", "deploy_ssl_url", "url", "deploy_url"].iter().find_map(|k| cloud::s(d, k)).map(str::to_owned)
}

#[async_trait]
impl DeployAdapter for NetlifyAdapter {
    fn id(&self) -> &str {
        "netlify"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Netlify
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
            "required": ["siteId"],
            "properties": {
                "siteId": { "type": "string", "description": "Netlify site id" },
                "dir": { "type": "string", "default": "dist", "description": "Published directory (relative to the source directory)" },
                "buildCommand": { "type": "string", "description": "Optional local build before upload" },
                "draft": { "type": "string", "enum": ["true", "false"], "default": "false" },
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
        let site = setting(req, "siteId").unwrap_or("<siteId>");
        let mut warnings: Vec<String> = cloud::credential_warning(req, SECRET).into_iter().collect();
        if setting(req, "siteId").is_none() {
            warnings.push("adapter setting `siteId` is required".into());
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
            format!("{base}/api/v1/sites/{site}/deploys"),
            Some(
                json!({ "files": { "/<path>": "<sha1>" }, "draft": setting(req, "draft") == Some("true") }).to_string(),
            ),
        ));
        requests.push(cloud::preview(
            "PUT",
            format!("{base}/api/v1/deploys/<id>/files/<path>"),
            Some("application/octet-stream (required files only)".into()),
        ));
        requests.push(cloud::preview("GET", format!("{base}/api/v1/deploys/<id>"), None));
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps: vec![
                format!("Create a file-digest deploy of `{}` on site {site}", setting(req, "dir").unwrap_or("dist")),
                "Upload the files Netlify does not have yet".into(),
                "Poll state until ready".into(),
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
        let site = common::required(req, "siteId")?;
        if let Some(b) = setting(req, "buildCommand") {
            common::run_shell(b, &req.source_dir, &common::command_env(req, false), &log, &cancel).await?;
        }
        let files = common::collect_files(common::under_source(req, setting(req, "dir").unwrap_or("dist"))?).await?;
        let digests: Map<String, Value> =
            files.iter().map(|f| (format!("/{}", f.rel), Value::String(f.sha1.clone()))).collect();
        let body = json!({
            "files": digests,
            "draft": setting(req, "draft") == Some("true"),
            "title": format!("orkestr {}", req.version),
        });
        let created = api
            .send(Method::POST, &format!("/api/v1/sites/{}/deploys", cloud::encode_path(site)), Body::Json(body, None))
            .await?;
        let id = cloud::s(&created, "id")
            .ok_or_else(|| DeployError::Failed("Netlify returned no deploy id".into()))?
            .to_owned();
        let required: BTreeSet<&str> = created
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        log.line(&format!("Netlify deploy {id} created; uploading {} of {} files", required.len(), files.len()));
        for f in files.iter().filter(|f| required.contains(f.sha1.as_str())) {
            if cancel.is_cancelled() {
                return Err(DeployError::Cancelled);
            }
            api.send(
                Method::PUT,
                &format!("/api/v1/deploys/{}/files/{}", cloud::encode_path(&id), cloud::encode_path(&f.rel)),
                Body::Bytes(f.bytes.clone(), Vec::new()),
            )
            .await?;
        }
        let ready = Self::wait_ready(&api, req, &id, &log, &cancel).await?;
        Ok(DeployOutcome { url: url_of(&ready), version: req.version.clone(), provider_ref: Some(id) })
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let api = Api::new(self.base(), req, SECRET, false)?;
        let site = common::required(req, "siteId")?;
        let id = target
            .provider_ref
            .clone()
            .ok_or_else(|| DeployError::Invalid("the earlier deployment has no Netlify deploy id".into()))?;
        log.line(&format!("restoring Netlify deploy {id} on site {site}"));
        api.send(
            Method::POST,
            &format!("/api/v1/sites/{}/deploys/{}/restore", cloud::encode_path(site), cloud::encode_path(&id)),
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
        Err(DeployError::Invalid("Netlify rollback needs the environment settings (use rollback_to)".into()))
    }
}
