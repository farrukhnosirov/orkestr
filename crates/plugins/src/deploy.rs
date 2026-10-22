//! Plugin `DeploymentProvider`s (ADR-0034 §13, ADR-0041): a
//! [`PluginDeployAdapter`] per `deployProviders` entry of an enabled plugin is
//! registered into `orkestr_deploy::DeployService` (target
//! `DeploymentTarget::Plugin("<packageId>/<provider>")`, adapter id
//! `plugin:<packageId>/<provider>`) and unregistered when the plugin is
//! disabled, killed, quarantined, uninstalled or safe mode is turned on.
//!
//! The adapter calls the optional guest export `orkestr_deploy` with
//! `{"op", "provider", "request", "target"?}`:
//!
//! | op | result (`ok`) |
//! |----|---------------|
//! | `availability` | `AdapterAvailability` (`{"state": "Ready"}`, ...) |
//! | `plan` | `{steps?, warnings?, requests?: [{method, target, body?}]}` |
//! | `deploy` | `{url?, version?, providerRef?}` |
//! | `rollback` | same as `deploy`; `target` = `{deploymentId, version, providerRef?}` |
//!
//! `request` carries ids, the environment (no secret values), settings,
//! version, `sourceDir` and `secretNames`: credential **names** only. A
//! provider reads a value with the `SecretGet` host op, which needs a
//! `Secrets` grant for that name. `Log` host calls made during
//! `deploy` / `rollback` stream into the deployment log (redacted by the
//! deploy service with the deployment's secrets + the global redactor).
//! Cancel (deploy.cancel / emergency stop) interrupts the guest through the
//! epoch callback.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use orkestr_deploy::{
    AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink, RollbackTarget,
};
use orkestr_domain::{Deployment, DeploymentTarget, PluginId};
use orkestr_protocol::methods::deploy::RequestPreview;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::host::{GrantedHost, HostApi, HostOp};
use crate::service::PluginService;
use crate::PluginError;

/// Deploy target of a plugin provider.
#[must_use]
pub fn target_of(package_id: &str, provider: &str) -> DeploymentTarget {
    DeploymentTarget::Plugin(format!("{package_id}/{provider}"))
}

/// Host used during `deploy` / `rollback`: `Log` lines also go to the deployment log.
struct DeployHost {
    inner: GrantedHost,
    sink: LogSink,
}

#[async_trait]
impl HostApi for DeployHost {
    async fn call(&self, caller: &PluginId, op: HostOp, input: Value) -> Result<Value, PluginError> {
        if op == HostOp::Log {
            if let Some(m) = input.get("message").and_then(Value::as_str) {
                self.sink.line(&m.chars().take(4_000).collect::<String>());
            }
        }
        self.inner.call(caller, op, input).await
    }
}

/// A deployment provider contributed by a plugin.
pub struct PluginDeployAdapter {
    id: String,
    target: DeploymentTarget,
    plugin_id: PluginId,
    provider: String,
    svc: Weak<PluginService>,
    availability: Mutex<AdapterAvailability>,
}

impl std::fmt::Debug for PluginDeployAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDeployAdapter").field("id", &self.id).finish_non_exhaustive()
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GuestPlan {
    steps: Vec<String>,
    warnings: Vec<String>,
    requests: Vec<RequestPreview>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GuestOutcome {
    url: Option<String>,
    version: Option<String>,
    provider_ref: Option<String>,
}

fn deploy_err(e: PluginError) -> DeployError {
    match e {
        PluginError::Cancelled => DeployError::Cancelled,
        PluginError::Killed => DeployError::Failed("the plugin was stopped by the kill switch".into()),
        PluginError::SafeMode => DeployError::Unavailable("plugin safe mode is on".into()),
        PluginError::Denied(m) => DeployError::Denied(m),
        PluginError::NotFound(m) => DeployError::Unavailable(m),
        other => DeployError::Failed(other.to_string()),
    }
}

fn request_json(req: &DeployRequest) -> Value {
    json!({
        "deploymentId": req.deployment_id,
        "projectId": req.project_id,
        "environment": req.environment,
        "target": req.target,
        "settings": req.settings,
        "version": req.version,
        "sourceDir": req.source_dir.to_string_lossy(),
        // Names only: values are read with `SecretGet` (needs a Secrets grant).
        "secretNames": req.credentials.0.keys().collect::<Vec<_>>(),
    })
}

fn outcome(v: Value, fallback_version: &str) -> Result<DeployOutcome, DeployError> {
    let o: GuestOutcome = serde_json::from_value(v)
        .map_err(|e| DeployError::Failed(format!("plugin returned an invalid deploy result: {e}")))?;
    let url = o.url.filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    Ok(DeployOutcome {
        url,
        version: o.version.filter(|v| !v.trim().is_empty()).unwrap_or_else(|| fallback_version.to_owned()),
        provider_ref: o.provider_ref.map(|r| r.chars().take(256).collect()),
    })
}

impl PluginDeployAdapter {
    /// Adapter for `provider` of `plugin`.
    #[must_use]
    pub(crate) fn new(svc: Weak<PluginService>, plugin_id: PluginId, package_id: &str, provider: &str) -> Self {
        Self {
            id: format!("plugin:{package_id}/{provider}"),
            target: target_of(package_id, provider),
            plugin_id,
            provider: provider.to_owned(),
            svc,
            availability: Mutex::new(AdapterAvailability::Ready),
        }
    }

    /// Plugin.
    #[must_use]
    pub fn plugin_id(&self) -> PluginId {
        self.plugin_id
    }

    async fn guest(
        &self,
        op: &str,
        body: Value,
        host: Arc<dyn HostApi>,
        cancel: CancellationToken,
    ) -> Result<Value, DeployError> {
        let svc = self.svc.upgrade().ok_or_else(|| DeployError::Unavailable("the plugin host is gone".into()))?;
        let mut input = json!({ "op": op, "provider": self.provider });
        if let (Value::Object(m), Value::Object(b)) = (&mut input, body) {
            m.extend(b);
        }
        svc.run_export(self.plugin_id, crate::abi::EXPORT_DEPLOY, input, host, cancel, &format!("deploy `{op}`"))
            .await
            .map(|r| r.output)
            .map_err(deploy_err)
    }

    fn host(&self, project: Option<orkestr_domain::ProjectId>) -> Result<GrantedHost, DeployError> {
        let svc = self.svc.upgrade().ok_or_else(|| DeployError::Unavailable("the plugin host is gone".into()))?;
        Ok(svc.host_for(project))
    }

    /// Asks the guest for its availability and caches the answer.
    pub(crate) async fn refresh_availability(&self) {
        let host = match self.host(None) {
            Ok(h) => Arc::new(h) as Arc<dyn HostApi>,
            Err(_) => return,
        };
        let a = match self.guest("availability", json!({}), host, CancellationToken::new()).await {
            Ok(v) => serde_json::from_value::<AdapterAvailability>(v).unwrap_or_else(|e| {
                AdapterAvailability::Unavailable { reason: format!("invalid availability from the plugin: {e}") }
            }),
            Err(e) => AdapterAvailability::Unavailable { reason: e.to_string() },
        };
        *self.availability.lock() = a;
    }

    async fn run(
        &self,
        op: &str,
        body: Value,
        project: orkestr_domain::ProjectId,
        log: LogSink,
        cancel: CancellationToken,
        fallback_version: &str,
    ) -> Result<DeployOutcome, DeployError> {
        let host = Arc::new(DeployHost { inner: self.host(Some(project))?, sink: log });
        let v = self.guest(op, body, host, cancel).await?;
        outcome(v, fallback_version)
    }
}

#[async_trait]
impl DeployAdapter for PluginDeployAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn target(&self) -> DeploymentTarget {
        self.target.clone()
    }

    fn availability(&self) -> AdapterAvailability {
        let Some(svc) = self.svc.upgrade() else {
            return AdapterAvailability::Unavailable { reason: "the plugin host is gone".into() };
        };
        if svc.safe_mode() {
            return AdapterAvailability::Unavailable { reason: "plugin safe mode is on".into() };
        }
        match svc.plugin_state(self.plugin_id) {
            Some(orkestr_domain::PluginState::Enabled) => self.availability.lock().clone(),
            Some(s) => AdapterAvailability::Unavailable { reason: format!("plugin is {s:?}") },
            None => AdapterAvailability::Unavailable { reason: "plugin is not installed".into() },
        }
    }

    fn verification(&self) -> &str {
        "plugin"
    }

    fn settings_schema(&self) -> Value {
        json!({ "type": "object", "additionalProperties": { "type": "string" } })
    }

    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        let host = Arc::new(self.host(Some(req.project_id))?);
        let v = self.guest("plan", json!({ "request": request_json(req) }), host, CancellationToken::new()).await?;
        let g: GuestPlan = serde_json::from_value(v)
            .map_err(|e| DeployError::Failed(format!("plugin returned an invalid plan: {e}")))?;
        Ok(DeployPlan {
            adapter_id: self.id.clone(),
            target: self.target.clone(),
            environment_id: req.environment.id,
            version: req.version.clone(),
            requires_approval: false,
            steps: g.steps.into_iter().take(100).collect(),
            requests: g.requests.into_iter().take(100).collect(),
            warnings: g.warnings.into_iter().take(100).collect(),
        })
    }

    async fn deploy(
        &self,
        req: &DeployRequest,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        self.run("deploy", json!({ "request": request_json(req) }), req.project_id, log, cancel, &req.version).await
    }

    async fn rollback_to(
        &self,
        req: &DeployRequest,
        target: &RollbackTarget,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let body = json!({
            "request": request_json(req),
            "target": {
                "deploymentId": target.deployment.id,
                "version": target.deployment.version,
                "providerRef": target.provider_ref,
            },
        });
        self.run("rollback", body, req.project_id, log, cancel, &req.version).await
    }

    async fn rollback(
        &self,
        dep: &Deployment,
        version: &str,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        let body = json!({ "target": { "deploymentId": dep.id, "version": version } });
        self.run("rollback", body, dep.project_id, log, cancel, version).await
    }
}
