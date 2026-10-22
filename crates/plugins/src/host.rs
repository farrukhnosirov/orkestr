//! Host API exposed to plugins (ADR-0034). Every call is checked against the
//! plugin's grants and audited; denials publish `PluginCallDenied`.
//!
//! Wire format (guest → `orkestr.host_call`): `{"op": "<HostOp>", "input": {...}}`;
//! reply `{"ok": <value>}` or `{"error": "<message>"}`.
//!
//! | op | input | output | check |
//! |----|-------|--------|-------|
//! | `Log` | `{level?, message}` | `null` | always (rate limited, redacted) |
//! | `HttpFetch` | `{url, method?, headers?, body?}` | `{status, headers, body}` | `Network` grant for the URL host; http(s) only; no redirects; 1 MiB body cap; Disable Network Tools honoured |
//! | `EmitEvent` | `{kind, payload?}` | `null` | always; published as a `Log` event with target `plugin:<packageId>` |
//! | `KvGet` | `{key}` | value or `null` | own store only |
//! | `KvSet` | `{key, value}` (`null` deletes) | `null` | own store only; quotas |
//! | `RpcRead` | `{method, params?}` | method result | `Read` grant for the method **and** the method is Read-class and not local-only |
//! | `SecretGet` | `{name}` | `{value}` | `Secrets` grant for the name; vault audit |

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orkestr_domain::{Actor, AuditOutcome, Capability, PluginId, ProjectId};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
use orkestr_protocol::methods::plugins::PluginCallDeniedPayload;
use orkestr_protocol::NewEvent;
use orkestr_security::audit::NewAudit;
use orkestr_security::Security;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::grants::GrantSet;
use crate::store::PluginStore;
use crate::PluginError;

/// Operations a plugin may request through `orkestr.host_call`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostOp {
    /// Write to the plugin log (always allowed, rate limited).
    Log,
    /// HTTP request to a granted `Network` scope host.
    HttpFetch,
    /// Publish a plugin event.
    EmitEvent,
    /// Read from the plugin's private KV store.
    KvGet,
    /// Write to the plugin's private KV store.
    KvSet,
    /// Read-class RPC method (granted per method).
    RpcRead,
    /// Read a granted secret reference.
    SecretGet,
}

/// Host side of the plugin ABI.
#[async_trait]
pub trait HostApi: Send + Sync {
    /// Executes `op` for `caller` after the capability check.
    async fn call(&self, caller: &PluginId, op: HostOp, input: Value) -> Result<Value, PluginError>;
}

/// Denies everything (safe default before the real host is wired).
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllHost;

#[async_trait]
impl HostApi for DenyAllHost {
    async fn call(&self, _caller: &PluginId, op: HostOp, _input: Value) -> Result<Value, PluginError> {
        Err(PluginError::Denied(format!("host op {op:?} is not available")))
    }
}

/// Executes Read-class RPC methods for plugins (installed by the core:
/// `orkestr_core::plugin_tools::install`). The host checks the method policy
/// and the plugin grant before calling it.
#[async_trait]
pub trait RpcReader: Send + Sync {
    /// Calls `method` with `params` in the context of `project`.
    async fn read(&self, method: &str, params: Value, project: Option<ProjectId>) -> Result<Value, String>;
}

/// Whether plugins may read `method` at all: known, Read rate class, Viewer
/// role, not local-only, not coordinator-only.
#[must_use]
pub fn rpc_read_allowed(method: &str) -> bool {
    use orkestr_protocol::policy::{policy_for, RateClass, Role};
    policy_for(method).is_some_and(|p| {
        p.rate_class == RateClass::Read && p.min_role == Role::Viewer && !p.local_only && !p.coordinator_only
    })
}

/// Max HTTP response body.
pub const MAX_HTTP_BODY: usize = 1024 * 1024;
/// Max KV key length.
pub const MAX_KV_KEY: usize = 256;
/// Max KV value / event payload size (serialized).
pub const MAX_KV_VALUE: usize = 64 * 1024;
/// Log lines per plugin per [`LOG_WINDOW`].
pub const LOG_BUDGET: u32 = 200;
/// Log rate window.
pub const LOG_WINDOW: Duration = Duration::from_secs(10);

/// Shared state of the enforcing host (one per [`crate::PluginService`]).
pub struct HostEnv {
    pub(crate) security: Arc<Security>,
    pub(crate) events: EventBus,
    pub(crate) store: PluginStore,
    pub(crate) http: Option<reqwest::Client>,
    pub(crate) rpc: RwLock<Option<Arc<dyn RpcReader>>>,
    /// Plugin id → package id of installed plugins.
    pub(crate) directory: RwLock<HashMap<PluginId, String>>,
    log_window: Mutex<HashMap<PluginId, (Instant, u32)>>,
}

impl std::fmt::Debug for HostEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostEnv").finish_non_exhaustive()
    }
}

impl HostEnv {
    /// New environment.
    ///
    /// # Errors
    /// HTTP client construction.
    pub fn new(security: Arc<Security>, events: EventBus, store: PluginStore) -> Result<Self, PluginError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent("orkestr-plugin-host/1")
            .build()
            .map_err(|e| PluginError::Internal(e.to_string()))?;
        Ok(Self {
            security,
            events,
            store,
            http: Some(http),
            rpc: RwLock::new(None),
            directory: RwLock::new(HashMap::new()),
            log_window: Mutex::new(HashMap::new()),
        })
    }

    /// Environment without an HTTP client (`HttpFetch` fails).
    #[must_use]
    pub fn without_http(security: Arc<Security>, events: EventBus, store: PluginStore) -> Self {
        Self {
            security,
            events,
            store,
            http: None,
            rpc: RwLock::new(None),
            directory: RwLock::new(HashMap::new()),
            log_window: Mutex::new(HashMap::new()),
        }
    }

    fn log_allowed(&self, id: PluginId) -> bool {
        let mut w = self.log_window.lock();
        let e = w.entry(id).or_insert((Instant::now(), 0));
        if e.0.elapsed() > LOG_WINDOW {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
        e.1 <= LOG_BUDGET
    }
}

/// The enforcing [`HostApi`]: grants, audit, `PluginCallDenied`.
#[derive(Debug, Clone)]
pub struct GrantedHost {
    env: Arc<HostEnv>,
    project: Option<ProjectId>,
}

impl GrantedHost {
    /// Host for calls made in the context of `project`.
    #[must_use]
    pub fn new(env: Arc<HostEnv>, project: Option<ProjectId>) -> Self {
        Self { env, project }
    }

    async fn audit(&self, id: &PluginId, op: HostOp, target: &str, outcome: AuditOutcome, detail: Value) {
        self.env
            .security
            .audit
            .record(
                NewAudit::new(Actor::System, format!("plugins.host.{op:?}"), target.to_owned(), outcome)
                    .capability(capability_of(op))
                    .project(self.project)
                    .details(json!({ "principal": crate::grants::principal(id), "detail": detail })),
            )
            .await;
    }

    async fn deny(&self, id: &PluginId, package_id: &str, op: HostOp, target: &str, reason: String) -> PluginError {
        self.audit(id, op, target, AuditOutcome::Denied, json!({ "reason": reason })).await;
        let _ = self.env.store.log(*id, "warn", &format!("host call {op:?} denied: {reason}")).await;
        let payload = PluginCallDeniedPayload {
            plugin_id: *id,
            package_id: package_id.to_owned(),
            op: format!("{op:?}"),
            target: (!target.is_empty()).then(|| orkestr_security::redact::redact(target)),
            reason: reason.clone(),
        };
        let mut ev = NewEvent::new(EventKind::PluginCallDenied(payload));
        ev.project_id = self.project;
        if let Err(e) = self.env.events.publish(ev).await {
            tracing::warn!(error = %e, "could not publish PluginCallDenied");
        }
        PluginError::Denied(reason)
    }

    async fn dispatch(
        &self,
        id: &PluginId,
        package_id: &str,
        op: HostOp,
        input: Value,
    ) -> Result<(String, Value), Result<PluginError, PluginError>> {
        // Err(Ok(e)) = denial (already published), Err(Err(e)) = failure.
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(ToOwned::to_owned);
        match op {
            HostOp::Log => {
                let msg = s(&input, "message").unwrap_or_default();
                let level = match s(&input, "level").as_deref() {
                    Some(l @ ("trace" | "debug" | "info" | "warn" | "error")) => l.to_owned(),
                    _ => "info".to_owned(),
                };
                if self.env.log_allowed(*id) {
                    let msg = orkestr_security::redact::redact(&msg.chars().take(4_000).collect::<String>());
                    self.env.store.log(*id, &level, &msg).await.map_err(Err)?;
                }
                Ok((String::new(), Value::Null))
            }
            HostOp::KvGet | HostOp::KvSet => {
                let key = s(&input, "key").ok_or_else(|| Err(PluginError::Invalid("`key` is required".into())))?;
                if key.is_empty() || key.len() > MAX_KV_KEY {
                    return Err(Err(PluginError::Invalid(format!("key must be 1-{MAX_KV_KEY} bytes"))));
                }
                if op == HostOp::KvGet {
                    let v = self.env.store.kv_get(*id, key.clone()).await.map_err(Err)?;
                    return Ok((key, v.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(Value::Null)));
                }
                let value = input.get("value").cloned().unwrap_or(Value::Null);
                let ser = (!value.is_null()).then(|| value.to_string());
                if ser.as_ref().is_some_and(|v| v.len() > MAX_KV_VALUE) {
                    return Err(Err(PluginError::Invalid(format!("value larger than {MAX_KV_VALUE} bytes"))));
                }
                self.env.store.kv_set(*id, key.clone(), ser).await.map_err(Err)?;
                Ok((key, Value::Null))
            }
            HostOp::EmitEvent => {
                let kind = s(&input, "kind").unwrap_or_default();
                if kind.is_empty() || kind.len() > 100 {
                    return Err(Err(PluginError::Invalid("`kind` must be 1-100 chars".into())));
                }
                let payload = input.get("payload").cloned().unwrap_or(Value::Null);
                if payload.to_string().len() > MAX_KV_VALUE {
                    return Err(Err(PluginError::Invalid(format!("payload larger than {MAX_KV_VALUE} bytes"))));
                }
                let mut ev = NewEvent::new(EventKind::Log(LogPayload {
                    level: LogLevel::Info,
                    target: format!("plugin:{package_id}"),
                    message: format!("plugin event {kind}"),
                    fields: orkestr_security::redact::redact_json(
                        &json!({ "pluginId": id, "packageId": package_id, "kind": kind, "payload": payload }),
                    ),
                }));
                ev.project_id = self.project;
                self.env.events.publish(ev).await.map_err(|e| Err(PluginError::Internal(e.to_string())))?;
                Ok((kind, Value::Null))
            }
            HostOp::HttpFetch => self.http_fetch(id, package_id, &input).await,
            HostOp::RpcRead => {
                let method = s(&input, "method").unwrap_or_default();
                if !rpc_read_allowed(&method) {
                    let r = format!("`{method}` is not a Read-class method plugins may call");
                    return Err(Ok(self.deny(id, package_id, op, &method, r).await));
                }
                let grants = GrantSet::load(&self.env.security, id).await.map_err(Err)?;
                if !grants.allows(Capability::Read, &method) {
                    let r = format!("RPC read `{method}` is not granted");
                    return Err(Ok(self.deny(id, package_id, op, &method, r).await));
                }
                let reader = self.env.rpc.read().clone();
                let Some(reader) = reader else {
                    return Err(Err(PluginError::Unimplemented("plugin RPC reads (no reader installed)")));
                };
                let params = input.get("params").cloned().unwrap_or_else(|| json!({}));
                let out = reader.read(&method, params, self.project).await.map_err(|e| Err(PluginError::Invalid(e)))?;
                Ok((method, out))
            }
            HostOp::SecretGet => {
                let name = s(&input, "name").unwrap_or_default();
                let grants = GrantSet::load(&self.env.security, id).await.map_err(Err)?;
                if name.is_empty() || !grants.allows(Capability::Secrets, &name) {
                    let r = format!("secret `{name}` is not granted");
                    return Err(Ok(self.deny(id, package_id, op, &name, r).await));
                }
                let v = self
                    .env
                    .security
                    .secrets
                    .resolve_by_name(&name, Actor::System, orkestr_security::AccessContext { project_id: self.project })
                    .await
                    .map_err(|e| Err(PluginError::Denied(e.to_string())))?;
                Ok((name, json!({ "value": v.as_str() })))
            }
        }
    }

    async fn http_fetch(
        &self,
        id: &PluginId,
        package_id: &str,
        input: &Value,
    ) -> Result<(String, Value), Result<PluginError, PluginError>> {
        let op = HostOp::HttpFetch;
        let raw = input.get("url").and_then(Value::as_str).unwrap_or_default();
        let url = reqwest::Url::parse(raw).map_err(|e| Err(PluginError::Invalid(format!("invalid url: {e}"))))?;
        let host = url.host_str().unwrap_or_default().to_owned();
        if !matches!(url.scheme(), "http" | "https") || host.is_empty() {
            return Err(Ok(self.deny(id, package_id, op, &host, "only http(s) URLs with a host".into()).await));
        }
        if self.env.security.engine.flags().network_tools_disabled {
            let r = "network tools are disabled by an emergency control".to_owned();
            return Err(Ok(self.deny(id, package_id, op, &host, r).await));
        }
        let grants = GrantSet::load(&self.env.security, id).await.map_err(Err)?;
        if !grants.allows(Capability::Network, &host) {
            let r = format!("network access to `{host}` is not granted");
            return Err(Ok(self.deny(id, package_id, op, &host, r).await));
        }
        let method = input.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| Err(PluginError::Invalid(format!("invalid method `{method}`"))))?;
        let Some(http) = &self.env.http else {
            return Err(Err(PluginError::Internal("the plugin HTTP client is unavailable".into())));
        };
        let mut req = http.request(method, url);
        if let Some(h) = input.get("headers").and_then(Value::as_object) {
            for (k, v) in h {
                if let Some(v) = v.as_str() {
                    req = req.header(k.as_str(), v);
                }
            }
        }
        if let Some(b) = input.get("body") {
            req = req.body(b.as_str().map_or_else(|| b.to_string(), ToOwned::to_owned));
        }
        let mut resp = req.send().await.map_err(|e| Err(PluginError::Invalid(format!("request failed: {e}"))))?;
        let status = resp.status().as_u16();
        let headers: serde_json::Map<String, Value> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_owned(), Value::String(v.to_owned()))))
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) =
            resp.chunk().await.map_err(|e| Err(PluginError::Invalid(format!("read failed: {e}"))))?
        {
            if body.len() + chunk.len() > MAX_HTTP_BODY {
                return Err(Err(PluginError::Invalid(format!("response body larger than {MAX_HTTP_BODY} bytes"))));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((host, json!({ "status": status, "headers": headers, "body": String::from_utf8_lossy(&body) })))
    }
}

fn capability_of(op: HostOp) -> Capability {
    match op {
        HostOp::HttpFetch => Capability::Network,
        HostOp::SecretGet => Capability::Secrets,
        _ => Capability::Read,
    }
}

#[async_trait]
impl HostApi for GrantedHost {
    async fn call(&self, caller: &PluginId, op: HostOp, input: Value) -> Result<Value, PluginError> {
        let package_id = self.env.directory.read().get(caller).cloned();
        let Some(package_id) = package_id else {
            return Err(PluginError::Denied("unknown plugin".into()));
        };
        match self.dispatch(caller, &package_id, op, input).await {
            Ok((target, v)) => {
                if op != HostOp::Log {
                    self.audit(caller, op, &target, AuditOutcome::Allowed, Value::Null).await;
                }
                Ok(v)
            }
            Err(Ok(denied)) => Err(denied),
            Err(Err(e)) => {
                self.audit(caller, op, "", AuditOutcome::Failed, json!({ "error": e.to_string() })).await;
                Err(e)
            }
        }
    }
}
