//! Fly.io Machines API (base URL override `ORKESTR_FLY_API`; request-shape
//! verified). Deploys an already pushed image: lists machines
//! (`GET /v1/apps/{app}/machines`), creates one when the app has none
//! (`POST /v1/apps/{app}/machines`) or updates each
//! (`POST /v1/apps/{app}/machines/{id}`), then waits for `started`
//! (`GET .../machines/{id}/wait?state=started`). Rollback redeploys the
//! earlier version's image. **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use reqwest::Method;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::cloud::{self, Api, Body};
use super::common::{self, number, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

const SECRET: &str = "FLY_API_TOKEN";

/// Fly adapter.
#[derive(Debug, Default, Clone)]
pub struct FlyAdapter {
    base: Option<String>,
}

impl FlyAdapter {
    /// Adapter with an explicit API base (tests); env vars / default otherwise.
    #[must_use]
    pub fn with_base(base: Option<String>) -> Self {
        Self { base }
    }

    fn base(&self) -> String {
        cloud::base_url("FLY", self.base.as_deref(), Some("https://api.machines.dev")).unwrap_or_default()
    }

    fn config(req: &DeployRequest, image: &str) -> Result<Value, DeployError> {
        let port: u16 = number(req, "internalPort", 8080u16)?;
        Ok(json!({
            "image": image,
            "env": req.environment.variables,
            "metadata": { "orkestr_version": req.version },
            "services": [{
                "protocol": "tcp",
                "internal_port": port,
                "ports": [
                    { "port": 80, "handlers": ["http"] },
                    { "port": 443, "handlers": ["tls", "http"] }
                ]
            }]
        }))
    }

    async fn roll_out(
        &self,
        req: &DeployRequest,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let api = Api::new(self.base(), req, SECRET, false)?;
        let app = common::required(req, "app")?;
        let image = cloud::image(req)?;
        let config = Self::config(req, &image)?;
        let app_path = format!("/v1/apps/{}/machines", cloud::encode_path(app));
        let list = api.send(Method::GET, &app_path, Body::None).await?;
        let machines: Vec<String> = list
            .as_array()
            .map(|a| a.iter().filter_map(|m| cloud::s(m, "id").map(str::to_owned)).collect())
            .unwrap_or_default();
        let mut ids = Vec::new();
        if machines.is_empty() {
            let mut body = json!({ "config": config });
            if let Some(r) = setting(req, "region") {
                body["region"] = json!(r);
            }
            log.line(&format!("creating a machine for {app} with {image}"));
            let m = api.send(Method::POST, &app_path, Body::Json(body, None)).await?;
            ids.push(
                cloud::s(&m, "id").ok_or_else(|| DeployError::Failed("Fly returned no machine id".into()))?.to_owned(),
            );
        } else {
            for id in machines {
                if cancel.is_cancelled() {
                    return Err(DeployError::Cancelled);
                }
                log.line(&format!("updating machine {id} → {image}"));
                api.send(
                    Method::POST,
                    &format!("{app_path}/{}", cloud::encode_path(&id)),
                    Body::Json(json!({ "config": config }), None),
                )
                .await?;
                ids.push(id);
            }
        }
        let timeout = number(req, "waitTimeoutSecs", 60u64)?;
        for id in &ids {
            if cancel.is_cancelled() {
                return Err(DeployError::Cancelled);
            }
            log.line(&format!("waiting for machine {id} to start"));
            api.send(
                Method::GET,
                &format!("{app_path}/{}/wait?state=started&timeout={timeout}", cloud::encode_path(id)),
                Body::None,
            )
            .await?;
        }
        Ok(DeployOutcome {
            url: Some(setting(req, "url").map_or_else(|| format!("https://{app}.fly.dev"), str::to_owned)),
            version: req.version.clone(),
            provider_ref: Some(ids.join(",")),
        })
    }
}

#[async_trait]
impl DeployAdapter for FlyAdapter {
    fn id(&self) -> &str {
        "fly"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Fly
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
            "required": ["app", "image"],
            "properties": {
                "app": { "type": "string", "description": "Fly app name" },
                "image": { "type": "string", "description": "Pushed image; `{version}` is replaced (registry.fly.io/app:{version})" },
                "internalPort": { "type": "string", "default": "8080" },
                "region": { "type": "string" },
                "url": { "type": "string", "description": "Public URL (default https://<app>.fly.dev)" },
                "waitTimeoutSecs": { "type": "string", "default": "60" }
            }
        })
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let base = self.base();
        let app = setting(req, "app").unwrap_or("<app>");
        let mut warnings: Vec<String> = cloud::credential_warning(req, SECRET).into_iter().collect();
        for k in ["app", "image"] {
            if setting(req, k).is_none() {
                warnings.push(format!("adapter setting `{k}` is required"));
            }
        }
        let image = cloud::image(req).unwrap_or_else(|_| "<image>".into());
        let config = Self::config(req, &image).unwrap_or(Value::Null);
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps: vec![
                format!("List machines of app `{app}`"),
                format!("Create or update machines with image {image}"),
                "Wait for every machine to reach `started`".into(),
            ],
            requests: vec![
                cloud::preview("GET", format!("{base}/v1/apps/{app}/machines"), None),
                cloud::preview(
                    "POST",
                    format!("{base}/v1/apps/{app}/machines[/<id>]"),
                    Some(json!({ "config": config }).to_string()),
                ),
                cloud::preview("GET", format!("{base}/v1/apps/{app}/machines/<id>/wait?state=started"), None),
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
        self.roll_out(req, &log, &cancel).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        _target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        log.line(&format!("redeploying the image of version {}", req.version));
        self.roll_out(req, &log, &cancel).await
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("Fly rollback needs the environment settings (use rollback_to)".into()))
    }
}
