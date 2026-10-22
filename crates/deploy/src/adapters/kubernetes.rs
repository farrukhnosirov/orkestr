//! Kubernetes API server (base URL override `ORKESTR_KUBERNETES_API`, else
//! the `apiServer` setting; request-shape verified). Sets the container image
//! with a strategic-merge `PATCH` of `apps/v1` Deployment and polls the
//! rollout status (`observedGeneration`, `updatedReplicas`,
//! `availableReplicas`). Rollback patches the earlier version's image.
//! **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use reqwest::Method;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::cloud::{self, Api, Body, Poll};
use super::common::{self, setting};
use crate::adapter::RollbackTarget;
use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

const SECRET: &str = "KUBE_TOKEN";
const PATCH_CT: &str = "application/strategic-merge-patch+json";

/// Kubernetes adapter.
#[derive(Debug, Default, Clone)]
pub struct KubernetesAdapter {
    base: Option<String>,
}

fn int(v: &Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for p in path {
        match cur.get(p) {
            Some(n) => cur = n,
            None => return 0,
        }
    }
    cur.as_i64().unwrap_or(0)
}

impl KubernetesAdapter {
    /// Adapter with an explicit API base (tests); env vars / `apiServer` otherwise.
    #[must_use]
    pub fn with_base(base: Option<String>) -> Self {
        Self { base }
    }

    fn base(&self, req: &DeployRequest) -> Option<String> {
        cloud::base_url("KUBERNETES", self.base.as_deref(), setting(req, "apiServer"))
    }

    fn path(req: &DeployRequest) -> Result<String, DeployError> {
        let ns = setting(req, "namespace").unwrap_or("default");
        let name = common::required(req, "deployment")?;
        Ok(format!("/apis/apps/v1/namespaces/{}/deployments/{}", cloud::encode_path(ns), cloud::encode_path(name)))
    }

    fn patch(req: &DeployRequest, image: &str) -> Value {
        let container = setting(req, "container").or_else(|| setting(req, "deployment")).unwrap_or("app");
        json!({
            "metadata": { "annotations": { "orkestr.dev/version": req.version } },
            "spec": { "template": {
                "metadata": { "annotations": { "orkestr.dev/version": req.version } },
                "spec": { "containers": [{ "name": container, "image": image }] }
            } }
        })
    }

    async fn rollout(
        &self,
        req: &DeployRequest,
        log: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let base =
            self.base(req).ok_or_else(|| DeployError::Invalid("adapter setting `apiServer` is required".into()))?;
        let insecure = setting(req, "insecureSkipTlsVerify") == Some("true");
        let api = Api::new(base, req, SECRET, insecure)?;
        let path = Self::path(req)?;
        let image = cloud::image(req)?;
        log.line(&format!("patching {path} → {image}"));
        let patched = api.send(Method::PATCH, &path, Body::Json(Self::patch(req, &image), Some(PATCH_CT))).await?;
        let generation = int(&patched, &["metadata", "generation"]);
        cloud::poll(req, log, cancel, || async {
            let d = api.send(Method::GET, &path, Body::None).await?;
            let want = d.pointer("/spec/replicas").and_then(Value::as_i64).unwrap_or(1);
            let observed = int(&d, &["status", "observedGeneration"]);
            let updated = int(&d, &["status", "updatedReplicas"]);
            let available = int(&d, &["status", "availableReplicas"]);
            let failed = d
                .pointer("/status/conditions")
                .and_then(Value::as_array)
                .and_then(|c| c.iter().find(|c| cloud::s(c, "reason") == Some("ProgressDeadlineExceeded")).cloned());
            if let Some(c) = failed {
                return Err(DeployError::Failed(format!(
                    "rollout failed: {}",
                    cloud::s(&c, "message").unwrap_or("progress deadline exceeded")
                )));
            }
            if observed >= generation && updated >= want && available >= want {
                Ok(Poll::Ready(()))
            } else {
                Ok(Poll::Pending(format!("{updated}/{want} updated, {available}/{want} available")))
            }
        })
        .await?;
        Ok(DeployOutcome {
            url: setting(req, "url").map(str::to_owned),
            version: req.version.clone(),
            provider_ref: Some(format!("{}@{generation}", setting(req, "deployment").unwrap_or_default())),
        })
    }
}

#[async_trait]
impl DeployAdapter for KubernetesAdapter {
    fn id(&self) -> &str {
        "kubernetes"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Kubernetes
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
            "required": ["apiServer", "deployment", "image"],
            "properties": {
                "apiServer": { "type": "string", "description": "https://<cluster>:6443" },
                "namespace": { "type": "string", "default": "default" },
                "deployment": { "type": "string" },
                "container": { "type": "string", "description": "Container name (default: deployment name)" },
                "image": { "type": "string", "description": "Image; `{version}` is replaced" },
                "url": { "type": "string" },
                "insecureSkipTlsVerify": { "type": "string", "enum": ["true", "false"], "default": "false" },
                "pollIntervalMs": { "type": "string", "default": "2000" },
                "timeoutSecs": { "type": "string", "default": "900" }
            }
        })
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let mut warnings: Vec<String> = cloud::credential_warning(req, SECRET).into_iter().collect();
        let base = self.base(req).unwrap_or_else(|| {
            warnings.push("adapter setting `apiServer` is required".into());
            "<apiServer>".into()
        });
        for k in ["deployment", "image"] {
            if setting(req, k).is_none() {
                warnings.push(format!("adapter setting `{k}` is required"));
            }
        }
        if setting(req, "insecureSkipTlsVerify") == Some("true") {
            warnings.push("TLS verification of the API server is disabled".into());
        }
        let path = Self::path(req).unwrap_or_else(|_| "/apis/apps/v1/namespaces/<ns>/deployments/<name>".into());
        let image = cloud::image(req).unwrap_or_else(|_| "<image>".into());
        Ok(DeployPlan {
            adapter_id: self.id().into(),
            target: self.target(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: req.environment.is_protected(),
            steps: vec![
                format!("Set image {image} (strategic-merge patch)"),
                "Wait for the rollout to complete".into(),
            ],
            requests: vec![
                cloud::preview(
                    "PATCH",
                    format!("{base}{path}"),
                    Some(format!("Content-Type: {PATCH_CT}\n{}", Self::patch(req, &image))),
                ),
                cloud::preview("GET", format!("{base}{path}"), None),
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
        self.rollout(req, &log, &cancel).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        _target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        log.line(&format!("rolling back to the image of version {}", req.version));
        self.rollout(req, &log, &cancel).await
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Invalid("Kubernetes rollback needs the environment settings (use rollback_to)".into()))
    }
}
