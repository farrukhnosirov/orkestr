//! [`PluginService`]: the plugin host facade used by `core/src/rpc/plugins.rs`.
//!
//! Lifecycle: `install` (verify, ABI check, extract; state `Installed` with
//! `pendingGrants`) → `grant` (decisions become permission rules of principal
//! `plugin:<id>`) → `set_enabled` → `invoke`. `kill` (per plugin or all)
//! interrupts running calls through the epoch callback and moves plugins to
//! `Killed`; safe mode (persisted) refuses every plugin code / widget load.
//! Three consecutive traps quarantine a plugin.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use base64::Engine as _;
use orkestr_deploy::DeployService;
use orkestr_domain::{
    now, AuditOutcome, Capability, DeploymentTarget, PermissionRequest, Plugin, PluginId, PluginKind, PluginState,
    ProjectId, ToolDecl, WidgetDecl,
};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::plugins::{
    GrantPluginParams, InstallPluginParams, InstallPluginResult, InvokePluginParams, InvokePluginResult,
    KillAllPluginsResult, PluginGrant, PluginInfo, PluginInstalledPayload, PluginLogLine, PluginStateChangedPayload,
};
use orkestr_protocol::NewEvent;
use orkestr_security::audit::NewAudit;
use orkestr_security::Security;
use orkestr_storage::Db;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::deploy::PluginDeployAdapter;
use crate::events::{EventQueue, EventStats};
use crate::host::{GrantedHost, HostApi, HostEnv, RpcReader};
use crate::package::{self, Trust};
use crate::runtime::{CallControl, WasmSandbox};
use crate::signing::TrustStore;
use crate::store::{PluginStore, StoredPlugin};
use crate::PluginError;

/// Consecutive traps after which a plugin is quarantined.
pub const QUARANTINE_AFTER_TRAPS: u32 = 3;
/// Max size of a widget entry HTML.
pub const MAX_WIDGET_HTML_BYTES: usize = 512 * 1024;
const SAFE_MODE_KEY: &str = "safe_mode";

/// Services the plugin host needs.
#[derive(Clone)]
pub struct PluginDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Permission engine, audit, vault.
    pub security: Arc<Security>,
    /// `<data_dir>/plugins`.
    pub data_dir: PathBuf,
}

/// A tool of an enabled `Tool` plugin (for agents, see `core/src/plugin_tools.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginTool {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Package id.
    pub package_id: String,
    /// Display name of the plugin.
    pub plugin_name: String,
    /// The tool.
    pub tool: ToolDecl,
    /// Most sensitive capability the plugin was granted (the agent needs it too).
    pub capability: Option<Capability>,
}

/// A widget of an enabled `Widget` plugin (WP3: dashboards).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginWidget {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Package id.
    pub package_id: String,
    /// The widget (its `entry_html` is read with [`PluginService::package_file`]).
    pub widget: WidgetDecl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Delivered,
    Denied,
    Failed,
    /// Plugin no longer runnable (disabled, killed, safe mode, uninstalled).
    Stop,
}

struct Running {
    plugin: PluginId,
    ctl: CallControl,
}

/// Plugin host.
pub struct PluginService {
    deps: PluginDeps,
    store: PluginStore,
    trust: TrustStore,
    host: Arc<HostEnv>,
    sandbox: std::sync::OnceLock<Result<WasmSandbox, String>>,
    plugins: RwLock<HashMap<PluginId, StoredPlugin>>,
    modules: Mutex<HashMap<PluginId, wasmtime::Module>>,
    running: Mutex<HashMap<u64, Running>>,
    next_call: AtomicU64,
    safe_mode: AtomicBool,
    loaded: tokio::sync::OnceCell<()>,
    op: tokio::sync::Mutex<()>,
    self_ref: std::sync::OnceLock<Weak<PluginService>>,
    event_queues: Mutex<HashMap<PluginId, EventQueue>>,
    event_capacity: AtomicUsize,
    events_started: AtomicBool,
    deploy: RwLock<Option<DeployService>>,
    deploy_adapters: Mutex<HashMap<DeploymentTarget, Arc<PluginDeployAdapter>>>,
}

impl std::fmt::Debug for PluginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginService").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

type R<T> = Result<T, PluginError>;

fn pending_of(manifest_requests: &[PermissionRequest], grants: &[PluginGrant]) -> Vec<PermissionRequest> {
    manifest_requests
        .iter()
        .filter(|r| !grants.iter().any(|g| g.capability == r.capability && g.scope == r.scope))
        .cloned()
        .collect()
}

fn info(p: &StoredPlugin) -> PluginInfo {
    PluginInfo {
        plugin: p.plugin.clone(),
        manifest: p.manifest.clone(),
        grants: p.grants.clone(),
        pending_grants: pending_of(&p.manifest.permissions, &p.grants),
        signer_fingerprint: p.signer_fingerprint.clone(),
        sha256: p.sha256.clone(),
        last_error: p.last_error.clone(),
    }
}

impl PluginService {
    /// Creates the service (no plugin code is loaded here; state is loaded
    /// lazily on first use or by [`Self::init`]).
    #[must_use]
    pub fn new(deps: PluginDeps) -> Self {
        let store = PluginStore::new(deps.db.clone());
        let host = HostEnv::new(Arc::clone(&deps.security), deps.events.clone(), store.clone()).unwrap_or_else(|e| {
            tracing::error!(error = %e, "plugin host HTTP client unavailable");
            HostEnv::without_http(Arc::clone(&deps.security), deps.events.clone(), store.clone())
        });
        Self {
            trust: TrustStore::new(deps.db.clone()),
            store,
            host: Arc::new(host),
            sandbox: std::sync::OnceLock::new(),
            plugins: RwLock::new(HashMap::new()),
            modules: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            next_call: AtomicU64::new(1),
            safe_mode: AtomicBool::new(false),
            loaded: tokio::sync::OnceCell::new(),
            op: tokio::sync::Mutex::new(()),
            self_ref: std::sync::OnceLock::new(),
            event_queues: Mutex::new(HashMap::new()),
            event_capacity: AtomicUsize::new(crate::events::DEFAULT_QUEUE_CAPACITY),
            events_started: AtomicBool::new(false),
            deploy: RwLock::new(None),
            deploy_adapters: Mutex::new(HashMap::new()),
            deps,
        }
    }

    /// Loads installed plugins and the persisted safe mode (idempotent; call at boot).
    ///
    /// # Errors
    /// Storage.
    pub async fn init(&self) -> R<()> {
        self.loaded
            .get_or_try_init(|| async {
                let safe = self.store.meta(SAFE_MODE_KEY).await?.is_some_and(|v| v == "1");
                self.safe_mode.store(safe, Ordering::SeqCst);
                let all = self.store.all().await?;
                let mut dir = self.host.directory.write();
                let mut map = self.plugins.write();
                for p in all {
                    dir.insert(p.plugin.id, p.plugin.package_id.clone());
                    map.insert(p.plugin.id, p);
                }
                if safe {
                    tracing::warn!("plugin safe mode is on: no plugin code will run");
                }
                Ok::<(), PluginError>(())
            })
            .await?;
        Ok(())
    }

    /// The publisher trust store (shared with the marketplace, ADR-0040).
    #[must_use]
    pub fn trust_store(&self) -> &TrustStore {
        &self.trust
    }

    /// Installs the executor of `RpcRead` host calls.
    pub fn set_rpc_reader(&self, reader: Arc<dyn RpcReader>) {
        *self.host.rpc.write() = Some(reader);
    }

    fn sandbox(&self) -> R<WasmSandbox> {
        self.sandbox
            .get_or_init(|| WasmSandbox::new().map_err(|e| e.to_string()))
            .clone()
            .map_err(PluginError::Internal)
    }

    fn package_dir(&self, id: PluginId) -> PathBuf {
        self.deps.data_dir.join(id.to_string())
    }

    fn cached(&self, id: PluginId) -> R<StoredPlugin> {
        self.plugins.read().get(&id).cloned().ok_or_else(|| PluginError::NotFound(format!("plugin {id}")))
    }

    async fn save(&self, p: StoredPlugin) -> R<()> {
        self.store.save(&p).await?;
        self.host.directory.write().insert(p.plugin.id, p.plugin.package_id.clone());
        self.plugins.write().insert(p.plugin.id, p);
        Ok(())
    }

    async fn publish(&self, kind: EventKind) {
        if let Err(e) = self.deps.events.publish(NewEvent::new(kind)).await {
            tracing::warn!(error = %e, "could not publish plugin event");
        }
    }

    async fn audit(&self, action: &str, resource: &str, outcome: AuditOutcome, details: Value) {
        self.deps
            .security
            .audit
            .record(NewAudit::new(self.deps.security.user(), action, resource, outcome).details(details))
            .await;
    }

    async fn set_state(&self, mut p: StoredPlugin, state: PluginState, reason: Option<String>) -> R<StoredPlugin> {
        p.plugin.state = state;
        p.plugin.enabled = state == PluginState::Enabled;
        if state == PluginState::Enabled {
            p.trap_count = 0;
        }
        self.save(p.clone()).await?;
        self.publish(EventKind::PluginStateChanged(PluginStateChangedPayload {
            plugin_id: p.plugin.id,
            package_id: p.plugin.package_id.clone(),
            state,
            reason,
        }))
        .await;
        if state != PluginState::Enabled {
            self.clear_events(Some(p.plugin.id));
        }
        self.sync_deploy();
        Ok(p)
    }

    /// Flags running calls (`None` = all) as killed. Returns the count.
    fn interrupt(&self, id: Option<PluginId>) -> u32 {
        let running = self.running.lock();
        let mut n = 0;
        for r in running.values().filter(|r| id.is_none_or(|i| i == r.plugin)) {
            r.ctl.killed.store(true, Ordering::SeqCst);
            r.ctl.cancel.cancel();
            n += 1;
        }
        n
    }

    /// Installed plugins.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn list(&self) -> R<Vec<PluginInfo>> {
        self.init().await?;
        let mut v: Vec<StoredPlugin> = self.plugins.read().values().cloned().collect();
        v.sort_by(|a, b| a.plugin.installed_at.cmp(&b.plugin.installed_at).then(a.plugin.name.cmp(&b.plugin.name)));
        Ok(v.iter().map(info).collect())
    }

    /// One plugin.
    ///
    /// # Errors
    /// Not found.
    pub async fn get(&self, id: PluginId) -> R<PluginInfo> {
        self.init().await?;
        Ok(info(&self.cached(id)?))
    }

    /// One plugin by package id.
    ///
    /// # Errors
    /// Storage.
    pub async fn find_package(&self, package_id: &str) -> R<Option<PluginInfo>> {
        self.init().await?;
        Ok(self.plugins.read().values().find(|p| p.plugin.package_id == package_id).map(info))
    }

    /// Installs (or updates, same package id) a package.
    ///
    /// # Errors
    /// Invalid / unsigned / untrusted package, ABI mismatch, key change on update.
    #[allow(clippy::too_many_lines)]
    pub async fn install(&self, params: InstallPluginParams) -> R<InstallPluginResult> {
        self.init().await?;
        let bytes = match (&params.path, &params.package_base64) {
            (Some(path), None) => {
                let path = PathBuf::from(path.trim().trim_matches('"'));
                let meta = tokio::fs::metadata(&path).await?;
                if meta.len() > package::MAX_PACKAGE_BYTES as u64 {
                    return Err(PluginError::Package("package file is too large".into()));
                }
                tokio::fs::read(&path).await?
            }
            (None, Some(b64)) => base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|_| PluginError::Invalid("packageBase64 is not base64".into()))?,
            _ => return Err(PluginError::Invalid("give exactly one of `path` or `packageBase64`".into())),
        };
        let source =
            params.source.clone().unwrap_or_else(|| if params.path.is_some() { "file" } else { "upload" }.into());
        let result = self.install_bytes(&bytes, &source).await;
        if let Err(e) = &result {
            self.audit("plugins.install", &source, AuditOutcome::Denied, json!({ "error": e.to_string() })).await;
        }
        result
    }

    /// Installs package bytes (marketplace entry point). See [`Self::install`].
    ///
    /// # Errors
    /// See [`Self::install`].
    pub async fn install_bytes(&self, bytes: &[u8], source: &str) -> R<InstallPluginResult> {
        self.init().await?;
        let owned = bytes.to_vec();
        let pkg = tokio::task::spawn_blocking(move || package::parse(&owned))
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))??;
        let trust = package::verify(&pkg, &self.trust, package::dev_mode()).await?;
        if let Some(wasm) = pkg.wasm() {
            let sb = self.sandbox()?;
            let wasm = wasm.to_vec();
            let module = tokio::task::spawn_blocking(move || sb.compile(&wasm))
                .await
                .map_err(|e| PluginError::Internal(e.to_string()))??;
            let needs = [
                (!pkg.manifest.subscribes.is_empty(), crate::abi::EXPORT_ON_EVENT, "subscribes"),
                (!pkg.manifest.deploy_providers.is_empty(), crate::abi::EXPORT_DEPLOY, "deployProviders"),
            ];
            for (needed, export, field) in needs {
                if needed && !crate::runtime::has_export(&module, export) {
                    return Err(PluginError::Package(format!(
                        "ABI v1: `{field}` needs the export `{export}(i32, i32) -> i64`"
                    )));
                }
            }
        }
        let _guard = self.op.lock().await;
        let existing = self.plugins.read().values().find(|p| p.plugin.package_id == pkg.manifest.id).cloned();
        if let (Some(old), Some(new)) = (&existing, trust.signer()) {
            if old.signer_key_id.as_deref().is_some_and(|k| k != new.key_id) && !package::dev_mode() {
                return Err(PluginError::Signature(format!(
                    "{} is installed from another publisher key; uninstall it first",
                    pkg.manifest.id
                )));
            }
        }
        if let (Some(old), None) = (&existing, trust.signer()) {
            if old.signer_key_id.is_some() && !package::dev_mode() {
                return Err(PluginError::Signature("an unsigned package cannot update a signed one".into()));
            }
        }
        let id = existing.as_ref().map_or_else(PluginId::new, |p| p.plugin.id);
        if existing.is_some() {
            self.interrupt(Some(id));
        }
        let dir = self.package_dir(id);
        let pkg2 = pkg.clone();
        tokio::task::spawn_blocking(move || package::extract_to(&pkg2, &dir))
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))??;
        self.modules.lock().remove(&id);
        let m = &pkg.manifest;
        let mut caps: Vec<Capability> = Vec::new();
        for p in &m.permissions {
            if !caps.contains(&p.capability) {
                caps.push(p.capability);
            }
        }
        // Keep earlier decisions that still match a request.
        let grants: Vec<PluginGrant> = existing
            .as_ref()
            .map(|o| {
                o.grants
                    .iter()
                    .filter(|g| m.permissions.iter().any(|r| r.capability == g.capability && r.scope == g.scope))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if existing.is_some() {
            // Drop rules of requests that disappeared.
            let set = crate::grants::GrantSet::load(&self.deps.security, &id).await?;
            for r in set.rules {
                let still =
                    grants.iter().any(|g| g.capability == r.capability && g.scope.as_deref().unwrap_or("*") == r.scope);
                if !still {
                    let _ = self.deps.security.permissions.delete(&r.rule_id, self.deps.security.user()).await;
                }
            }
        }
        let pending = pending_of(&m.permissions, &grants);
        let state = match &existing {
            Some(o) if pending.is_empty() => o.plugin.state,
            _ => PluginState::Installed,
        };
        let stored = StoredPlugin {
            plugin: Plugin {
                id,
                package_id: m.id.clone(),
                state,
                name: m.name.clone(),
                version: m.version.clone(),
                publisher: m.publisher.clone(),
                kinds: m.kinds.clone(),
                permissions: caps,
                enabled: state == PluginState::Enabled,
                source: source.chars().take(500).collect(),
                installed_at: existing.as_ref().map_or_else(now, |o| o.plugin.installed_at),
            },
            manifest: m.clone(),
            grants,
            signer_fingerprint: trust.signer().map(|s| s.fingerprint.clone()),
            signer_key_id: trust.signer().map(|s| s.key_id.clone()),
            sha256: pkg.sha256.clone(),
            last_error: None,
            trap_count: 0,
        };
        self.save(stored.clone()).await?;
        self.sync_deploy();
        let trust_label = match &trust {
            Trust::Trusted(_) => "trusted",
            Trust::UntrustedDev(_) => "untrusted (dev mode)",
            Trust::UnsignedDev => "unsigned (dev mode)",
        };
        let _ = self
            .store
            .log(id, "info", &format!("installed {} {} from {source} ({trust_label})", m.id, m.version))
            .await;
        self.audit(
            "plugins.install",
            &m.id,
            AuditOutcome::Executed,
            json!({ "pluginId": id, "version": m.version, "sha256": pkg.sha256, "signer": stored.signer_fingerprint, "trust": trust_label, "update": existing.is_some() }),
        )
        .await;
        let plugin = info(&stored);
        self.publish(EventKind::PluginInstalled(PluginInstalledPayload { plugin: plugin.clone() })).await;
        Ok(InstallPluginResult { pending_grants: plugin.pending_grants.clone(), plugin })
    }

    /// Decides permission requests (each decision must match a manifest request).
    ///
    /// # Errors
    /// Not found / scope mismatch.
    pub async fn grant(&self, params: GrantPluginParams) -> R<PluginInfo> {
        self.init().await?;
        {
            let _guard = self.op.lock().await;
            let mut p = self.cached(params.plugin_id)?;
            for d in &params.decisions {
                if !p.manifest.permissions.iter().any(|r| r.capability == d.capability && r.scope == d.scope) {
                    return Err(PluginError::Invalid(format!(
                        "{:?} {} was not requested by the plugin",
                        d.capability,
                        d.scope.as_deref().unwrap_or("*")
                    )));
                }
            }
            for d in &params.decisions {
                crate::grants::store_decision(
                    &self.deps.security,
                    &p.plugin.id,
                    &p.plugin.package_id,
                    d.capability,
                    d.scope.as_deref(),
                    d.allow,
                )
                .await?;
                p.grants.retain(|g| !(g.capability == d.capability && g.scope == d.scope));
                p.grants.push(PluginGrant {
                    capability: d.capability,
                    scope: d.scope.clone(),
                    allowed: d.allow,
                    decided_at: now(),
                });
            }
            self.save(p.clone()).await?;
            self.audit(
                "plugins.grant",
                &p.plugin.package_id,
                AuditOutcome::Executed,
                json!({ "pluginId": p.plugin.id, "decisions": params.decisions.iter().map(|d| json!({"capability": d.capability, "scope": d.scope, "allow": d.allow})).collect::<Vec<_>>() }),
            )
            .await;
        }
        let p = self.cached(params.plugin_id)?;
        if params.enable
            && pending_of(&p.manifest.permissions, &p.grants).is_empty()
            && p.plugin.state != PluginState::Enabled
        {
            return self.set_enabled(params.plugin_id, true, Some("enabled after grants".into())).await;
        }
        Ok(info(&p))
    }

    /// Uninstalls a plugin (running calls are interrupted, grants revoked, files removed).
    ///
    /// # Errors
    /// Not found.
    pub async fn uninstall(&self, id: PluginId) -> R<()> {
        self.init().await?;
        let _guard = self.op.lock().await;
        let p = self.cached(id)?;
        self.interrupt(Some(id));
        crate::grants::revoke_all(&self.deps.security, &id).await?;
        self.store.delete(id).await?;
        self.plugins.write().remove(&id);
        self.host.directory.write().remove(&id);
        self.modules.lock().remove(&id);
        self.event_queues.lock().remove(&id);
        self.sync_deploy();
        let dir = self.package_dir(id);
        if dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                tracing::warn!(error = %e, "could not remove plugin files");
            }
        }
        self.audit("plugins.uninstall", &p.plugin.package_id, AuditOutcome::Executed, json!({ "pluginId": id })).await;
        self.publish(EventKind::PluginStateChanged(PluginStateChangedPayload {
            plugin_id: id,
            package_id: p.plugin.package_id,
            state: PluginState::Disabled,
            reason: Some("uninstalled".into()),
        }))
        .await;
        Ok(())
    }

    /// Enables (`true`) / disables (`false`) a plugin. Disabling interrupts its running calls.
    ///
    /// # Errors
    /// Not found / pending grants.
    pub async fn set_enabled(&self, id: PluginId, enabled: bool, reason: Option<String>) -> R<PluginInfo> {
        self.init().await?;
        if !enabled {
            self.interrupt(Some(id));
        }
        let _guard = self.op.lock().await;
        let p = self.cached(id)?;
        if enabled {
            let pending = pending_of(&p.manifest.permissions, &p.grants);
            if !pending.is_empty() {
                return Err(PluginError::Conflict(format!(
                    "{} permission request(s) are not decided yet (plugins.grant)",
                    pending.len()
                )));
            }
        }
        let state = if enabled { PluginState::Enabled } else { PluginState::Disabled };
        let action = if enabled { "plugins.enable" } else { "plugins.disable" };
        self.audit(action, &p.plugin.package_id, AuditOutcome::Executed, json!({ "pluginId": id, "reason": reason }))
            .await;
        let p = self.set_state(p, state, reason.or_else(|| Some("user".into()))).await?;
        Ok(info(&p))
    }

    /// Kill switch: interrupts running calls and moves the plugin (`Some`) or
    /// every plugin (`None`) to `Killed`. Used by `plugins.killAll` and the
    /// emergency stop; never fails (errors are logged).
    pub async fn kill(&self, id: Option<PluginId>, reason: &str) -> KillAllPluginsResult {
        let calls_interrupted = self.interrupt(id);
        if let Err(e) = self.init().await {
            tracing::warn!(error = %e, "plugin kill: state not loaded");
        }
        let _guard = self.op.lock().await;
        let targets: Vec<StoredPlugin> = self
            .plugins
            .read()
            .values()
            .filter(|p| id.is_none_or(|i| i == p.plugin.id))
            .filter(|p| !matches!(p.plugin.state, PluginState::Killed | PluginState::Quarantined))
            .cloned()
            .collect();
        let mut killed = 0u32;
        for p in targets {
            match self.set_state(p, PluginState::Killed, Some(reason.to_owned())).await {
                Ok(_) => killed += 1,
                Err(e) => tracing::warn!(error = %e, "could not kill plugin"),
            }
        }
        self.modules.lock().clear();
        self.audit(
            "plugins.killAll",
            id.map_or_else(|| "*".to_owned(), |i| i.to_string()).as_str(),
            AuditOutcome::Executed,
            json!({ "reason": reason, "killed": killed, "callsInterrupted": calls_interrupted }),
        )
        .await;
        KillAllPluginsResult { killed, calls_interrupted }
    }

    /// Current safe mode (`false` until loaded by [`Self::init`]).
    #[must_use]
    pub fn safe_mode(&self) -> bool {
        self.safe_mode.load(Ordering::SeqCst)
    }

    /// Sets safe mode (persisted). Turning it on interrupts running calls.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn set_safe_mode(&self, enabled: bool) -> R<bool> {
        self.init().await?;
        self.store.set_meta(SAFE_MODE_KEY, if enabled { "1" } else { "0" }.into()).await?;
        self.safe_mode.store(enabled, Ordering::SeqCst);
        if enabled {
            self.interrupt(None);
            self.modules.lock().clear();
            self.clear_events(None);
        }
        self.sync_deploy();
        self.audit("plugins.setSafeMode", "*", AuditOutcome::Executed, json!({ "enabled": enabled })).await;
        Ok(enabled)
    }

    /// Invokes a tool of a code plugin.
    ///
    /// # Errors
    /// Not enabled / denied / trapped.
    pub async fn invoke(&self, params: InvokePluginParams) -> R<InvokePluginResult> {
        self.invoke_with_cancel(params, CancellationToken::new()).await
    }

    /// [`Self::invoke`] with a caller cancellation token (agent stop).
    ///
    /// # Errors
    /// See [`Self::invoke`].
    pub async fn invoke_with_cancel(
        &self,
        params: InvokePluginParams,
        cancel: CancellationToken,
    ) -> R<InvokePluginResult> {
        self.init().await?;
        let p = self.runnable(params.plugin_id)?;
        if !p.manifest.tools.iter().any(|t| t.name == params.tool) {
            return Err(PluginError::NotFound(format!("tool `{}` of {}", params.tool, p.plugin.package_id)));
        }
        let input = json!({ "tool": params.tool, "input": params.input });
        let host = Arc::new(GrantedHost::new(Arc::clone(&self.host), params.project_id));
        let label = format!("tool `{}`", params.tool);
        self.run_export(params.plugin_id, crate::abi::EXPORT_CALL, input, host, cancel, &label).await
    }

    /// The plugin if it may run code now (not safe mode, `Enabled`).
    fn runnable(&self, id: PluginId) -> R<StoredPlugin> {
        if self.safe_mode() {
            return Err(PluginError::SafeMode);
        }
        let p = self.cached(id)?;
        match p.plugin.state {
            PluginState::Enabled => Ok(p),
            PluginState::Killed => Err(PluginError::Killed),
            s => Err(PluginError::Conflict(format!("plugin {} is {s:?}", p.plugin.package_id))),
        }
    }

    /// State of an installed plugin.
    #[must_use]
    pub fn plugin_state(&self, id: PluginId) -> Option<PluginState> {
        self.plugins.read().get(&id).map(|p| p.plugin.state)
    }

    /// Runs the guest export `export` of an enabled code plugin with `input`
    /// (JSON) under its limits: registered for the kill switch, traps counted
    /// for quarantine, result envelope parsed. `label` names the call in logs.
    pub(crate) async fn run_export(
        &self,
        id: PluginId,
        export: &'static str,
        input: Value,
        host: Arc<dyn HostApi>,
        cancel: CancellationToken,
        label: &str,
    ) -> R<InvokePluginResult> {
        self.init().await?;
        let p = self.runnable(id)?;
        let module = self.module(&p).await?;
        if !crate::runtime::has_export(&module, export) {
            return Err(PluginError::Invalid(format!("{} does not export `{export}`", p.plugin.package_id)));
        }
        let input = serde_json::to_vec(&input).map_err(|e| PluginError::Invalid(e.to_string()))?;
        let ctl = CallControl { killed: Arc::default(), cancel };
        let call_id = self.next_call.fetch_add(1, Ordering::Relaxed);
        self.running.lock().insert(call_id, Running { plugin: p.plugin.id, ctl: ctl.clone() });
        // A kill that raced the registration above.
        if self.cached(p.plugin.id).is_ok_and(|q| q.plugin.state != PluginState::Enabled) || self.safe_mode() {
            ctl.killed.store(true, Ordering::SeqCst);
        }
        let started = Instant::now();
        let result =
            self.sandbox()?.call_export(module, export, p.plugin.id, input, p.manifest.limits, host, ctl).await;
        self.running.lock().remove(&call_id);
        match result {
            Ok(out) => {
                if p.trap_count > 0 || p.last_error.is_some() {
                    if let Ok(mut q) = self.cached(p.plugin.id) {
                        q.trap_count = 0;
                        q.last_error = None;
                        let _ = self.save(q).await;
                    }
                }
                let output = crate::runtime::parse_result(&out.output)?;
                Ok(InvokePluginResult {
                    output,
                    fuel_used: out.fuel_used,
                    duration_ms: u64::try_from(out.duration.as_millis()).unwrap_or(u64::MAX),
                })
            }
            Err(PluginError::Trap(msg)) => {
                self.record_trap(p.plugin.id, label, &msg, started).await;
                Err(PluginError::Trap(msg))
            }
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------ events (subscribes)

    fn bind(self: &Arc<Self>) {
        let _ = self.self_ref.set(Arc::downgrade(self));
    }

    /// Starts delivering bus events to subscribed plugins (idempotent; call at
    /// boot). See [`crate::events`].
    pub fn start_event_delivery(self: &Arc<Self>) {
        self.bind();
        if self.events_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut sub = self.deps.events.subscribe_all();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(r) = sub.recv().await {
                let Some(this) = weak.upgrade() else { break };
                match r {
                    Received::Event(ev) => this.enqueue_event(ev),
                    Received::Lagged(n) => {
                        tracing::warn!(missed = n, "plugin event delivery lagged behind the event bus");
                        let mut q = this.event_queues.lock();
                        for e in q.values_mut() {
                            e.stats.dropped += n;
                        }
                    }
                }
            }
        });
    }

    /// Per-plugin queue capacity (default [`crate::events::DEFAULT_QUEUE_CAPACITY`]).
    pub fn set_event_queue_capacity(&self, capacity: usize) {
        self.event_capacity.store(capacity.max(1), Ordering::SeqCst);
    }

    /// Delivery counters of a plugin.
    #[must_use]
    pub fn event_stats(&self, id: PluginId) -> EventStats {
        self.event_queues.lock().get(&id).map(EventQueue::snapshot).unwrap_or_default()
    }

    fn clear_events(&self, id: Option<PluginId>) {
        let mut q = self.event_queues.lock();
        for (pid, e) in q.iter_mut() {
            if id.is_none_or(|i| i == *pid) {
                e.clear();
            }
        }
    }

    /// Bus side: never waits on plugin code (push + maybe spawn a worker).
    fn enqueue_event(&self, ev: Arc<orkestr_protocol::Event>) {
        if self.safe_mode() {
            return;
        }
        let kind = ev.kind_name();
        if !crate::events::subscribable(kind) {
            return;
        }
        let targets: Vec<PluginId> = self
            .plugins
            .read()
            .values()
            .filter(|p| {
                p.plugin.state == PluginState::Enabled
                    && p.manifest.entry.is_some()
                    && p.manifest.subscribes.iter().any(|k| k == kind)
            })
            .map(|p| p.plugin.id)
            .collect();
        if targets.is_empty() {
            return;
        }
        let cap = self.event_capacity.load(Ordering::SeqCst);
        let Some(weak) = self.self_ref.get().cloned() else { return };
        let mut queues = self.event_queues.lock();
        for id in targets {
            let q = queues.entry(id).or_default();
            q.push(Arc::clone(&ev), cap);
            if !q.worker {
                q.worker = true;
                tokio::spawn(Self::event_worker(weak.clone(), id));
            }
        }
    }

    async fn event_worker(weak: Weak<Self>, id: PluginId) {
        let mut batch = EventStats::default();
        let dropped_at_start = weak.upgrade().map_or(0, |s| s.event_stats(id).dropped);
        loop {
            let Some(this) = weak.upgrade() else { return };
            let next = {
                let mut queues = this.event_queues.lock();
                let Some(q) = queues.get_mut(&id) else { return };
                let n = q.items.pop_front();
                if n.is_none() {
                    q.worker = false;
                }
                n
            };
            let Some(ev) = next else {
                this.event_batch_done(id, batch, dropped_at_start).await;
                return;
            };
            let outcome = this.deliver_event(id, &ev).await;
            {
                let mut queues = this.event_queues.lock();
                let Some(q) = queues.get_mut(&id) else { return };
                match outcome {
                    Delivery::Delivered => q.stats.delivered += 1,
                    Delivery::Denied => q.stats.denied += 1,
                    Delivery::Failed => q.stats.failed += 1,
                    Delivery::Stop => q.clear(),
                }
            }
            match outcome {
                Delivery::Delivered => batch.delivered += 1,
                Delivery::Denied => batch.denied += 1,
                Delivery::Failed => batch.failed += 1,
                Delivery::Stop => {}
            }
        }
    }

    async fn event_batch_done(&self, id: PluginId, batch: EventStats, dropped_at_start: u64) {
        let dropped = self.event_stats(id).dropped.saturating_sub(dropped_at_start);
        if batch.delivered + batch.failed + batch.denied + dropped == 0 {
            return;
        }
        let Ok(p) = self.cached(id) else { return };
        if batch.denied > 0 {
            let _ = self
                .store
                .log(
                    id,
                    "warn",
                    &format!("{} event(s) not delivered: the `event/<Kind>` Read grant is missing", batch.denied),
                )
                .await;
        }
        if dropped > 0 {
            let _ = self.store.log(id, "warn", &format!("{dropped} event(s) dropped: delivery queue full")).await;
        }
        self.audit(
            "plugins.events.deliver",
            &p.plugin.package_id,
            if batch.failed > 0 { AuditOutcome::Failed } else { AuditOutcome::Executed },
            json!({ "pluginId": id, "delivered": batch.delivered, "failed": batch.failed, "denied": batch.denied, "dropped": dropped }),
        )
        .await;
    }

    async fn deliver_event(&self, id: PluginId, ev: &orkestr_protocol::Event) -> Delivery {
        let Ok(p) = self.runnable(id) else { return Delivery::Stop };
        let kind = ev.kind_name();
        let grants = match crate::grants::GrantSet::load(&self.deps.security, &id).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "plugin grants unavailable");
                return Delivery::Failed;
            }
        };
        if !grants.allows(Capability::Read, &crate::events::event_scope(kind)) {
            return Delivery::Denied;
        }
        let envelope = serde_json::to_value(ev).unwrap_or(Value::Null);
        let input = json!({ "event": orkestr_security::redact::redact_json(&envelope) });
        let host = Arc::new(GrantedHost::new(Arc::clone(&self.host), ev.project_id));
        let label = format!("event `{kind}`");
        match self.run_export(id, crate::abi::EXPORT_ON_EVENT, input, host, CancellationToken::new(), &label).await {
            Ok(_) => Delivery::Delivered,
            Err(PluginError::Killed | PluginError::SafeMode | PluginError::Conflict(_) | PluginError::NotFound(_)) => {
                Delivery::Stop
            }
            Err(e) => {
                tracing::debug!(plugin = %p.plugin.package_id, error = %e, "plugin event delivery failed");
                Delivery::Failed
            }
        }
    }

    // ------------------------------------------------------------ deployment providers

    /// Connects the deployment service: every `deployProviders` entry of an
    /// enabled plugin is registered as a [`PluginDeployAdapter`] (and kept in
    /// sync with enable / disable / kill / uninstall / safe mode).
    ///
    /// # Errors
    /// Plugin state could not be loaded.
    pub async fn attach_deploy(self: &Arc<Self>, deploy: DeployService) -> R<()> {
        self.bind();
        *self.deploy.write() = Some(deploy);
        self.init().await?;
        self.sync_deploy();
        Ok(())
    }

    /// Adapters this host registered (for tests / diagnostics).
    #[must_use]
    pub fn deploy_targets(&self) -> Vec<DeploymentTarget> {
        let mut v: Vec<DeploymentTarget> = self.deploy_adapters.lock().keys().cloned().collect();
        v.sort_by_key(|t| format!("{t:?}"));
        v
    }

    fn sync_deploy(&self) {
        let Some(deploy) = self.deploy.read().clone() else { return };
        let Some(weak) = self.self_ref.get().cloned() else { return };
        let safe = self.safe_mode();
        let desired: Vec<(PluginId, String, String)> = self
            .plugins
            .read()
            .values()
            .filter(|p| {
                !safe
                    && p.plugin.state == PluginState::Enabled
                    && p.manifest.entry.is_some()
                    && p.manifest.kinds.contains(&PluginKind::DeploymentProvider)
            })
            .flat_map(|p| {
                p.manifest.deploy_providers.iter().map(|d| (p.plugin.id, p.plugin.package_id.clone(), d.clone()))
            })
            .collect();
        let mut current = self.deploy_adapters.lock();
        let wanted: Vec<DeploymentTarget> =
            desired.iter().map(|(_, pkg, prov)| crate::deploy::target_of(pkg, prov)).collect();
        current.retain(|t, a| {
            let keep = wanted.contains(t) && desired.iter().any(|(id, ..)| *id == a.plugin_id());
            if !keep {
                deploy.unregister_adapter(t);
            }
            keep
        });
        for (id, pkg, prov) in desired {
            let target = crate::deploy::target_of(&pkg, &prov);
            if current.contains_key(&target) {
                continue;
            }
            let a = Arc::new(PluginDeployAdapter::new(weak.clone(), id, &pkg, &prov));
            deploy.register_adapter(Arc::clone(&a) as Arc<dyn orkestr_deploy::DeployAdapter>);
            current.insert(target, Arc::clone(&a));
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                h.spawn(async move { a.refresh_availability().await });
            }
        }
    }

    async fn record_trap(&self, id: PluginId, label: &str, msg: &str, started: Instant) {
        let _ = self
            .store
            .log(id, "error", &format!("{label} trapped after {} ms: {msg}", started.elapsed().as_millis()))
            .await;
        let _guard = self.op.lock().await;
        let Ok(mut q) = self.cached(id) else { return };
        q.trap_count += 1;
        q.last_error = Some(msg.to_owned());
        if q.trap_count >= QUARANTINE_AFTER_TRAPS && q.plugin.state == PluginState::Enabled {
            let reason = format!("quarantined after {} consecutive traps: {msg}", q.trap_count);
            let _ = self.set_state(q, PluginState::Quarantined, Some(reason)).await;
            self.modules.lock().remove(&id);
        } else {
            let _ = self.save(q).await;
        }
    }

    async fn module(&self, p: &StoredPlugin) -> R<wasmtime::Module> {
        if let Some(m) = self.modules.lock().get(&p.plugin.id) {
            return Ok(m.clone());
        }
        let entry = p.manifest.entry.as_deref().ok_or_else(|| PluginError::Invalid("not a code plugin".into()))?;
        let path = entry.split('/').fold(self.package_dir(p.plugin.id), |acc, c| acc.join(c));
        let bytes = tokio::fs::read(&path).await?;
        let sb = self.sandbox()?;
        let m = tokio::task::spawn_blocking(move || sb.compile(&bytes))
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))??;
        self.modules.lock().insert(p.plugin.id, m.clone());
        Ok(m)
    }

    /// Log lines.
    ///
    /// # Errors
    /// Not found.
    pub async fn logs(&self, id: PluginId, limit: u32) -> R<Vec<PluginLogLine>> {
        self.init().await?;
        self.cached(id)?;
        self.store.logs(id, limit.clamp(1, 2_000)).await
    }

    /// Tools of enabled `Tool` plugins (empty in safe mode / before [`Self::init`]).
    #[must_use]
    pub fn tool_catalog(&self) -> Vec<PluginTool> {
        if self.safe_mode() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let plugins = self.plugins.read();
        let mut sorted: Vec<&StoredPlugin> = plugins.values().collect();
        sorted.sort_by(|a, b| a.plugin.package_id.cmp(&b.plugin.package_id));
        for p in sorted {
            if p.plugin.state != PluginState::Enabled || !p.manifest.kinds.contains(&PluginKind::Tool) {
                continue;
            }
            let granted = |c: Capability| p.grants.iter().any(|g| g.capability == c && g.allowed);
            let capability =
                [Capability::Secrets, Capability::Network, Capability::Read].into_iter().find(|c| granted(*c));
            for t in &p.manifest.tools {
                out.push(PluginTool {
                    plugin_id: p.plugin.id,
                    package_id: p.plugin.package_id.clone(),
                    plugin_name: p.plugin.name.clone(),
                    tool: t.clone(),
                    capability,
                });
            }
        }
        out
    }

    /// Widgets of enabled `Widget` plugins (empty in safe mode).
    ///
    /// # Errors
    /// Storage.
    pub async fn widgets(&self) -> R<Vec<PluginWidget>> {
        self.init().await?;
        if self.safe_mode() {
            return Ok(Vec::new());
        }
        let plugins = self.plugins.read();
        Ok(plugins
            .values()
            .filter(|p| p.plugin.state == PluginState::Enabled && p.manifest.kinds.contains(&PluginKind::Widget))
            .flat_map(|p| {
                p.manifest.widgets.iter().map(|w| PluginWidget {
                    plugin_id: p.plugin.id,
                    package_id: p.plugin.package_id.clone(),
                    widget: w.clone(),
                })
            })
            .collect())
    }

    /// Reads one file of an installed package (widget HTML, agent template JSON, ...).
    /// The path is normalized and confined to the package folder; refused in
    /// safe mode for enabled-code assets (widgets).
    ///
    /// # Errors
    /// Not found / unsafe path / safe mode.
    pub async fn package_file(&self, id: PluginId, path: &str) -> R<Vec<u8>> {
        self.init().await?;
        if self.safe_mode() {
            return Err(PluginError::SafeMode);
        }
        self.cached(id)?;
        let rel = package::normalize_entry(path)?;
        if rel == package::SIGNATURE_FILE {
            return Err(PluginError::NotFound(path.to_owned()));
        }
        let full = rel.split('/').fold(self.package_dir(id), |acc, c| acc.join(c));
        let meta = tokio::fs::metadata(&full).await.map_err(|_| PluginError::NotFound(path.to_owned()))?;
        if !meta.is_file() || meta.len() > package::MAX_FILE_BYTES {
            return Err(PluginError::NotFound(path.to_owned()));
        }
        Ok(tokio::fs::read(&full).await?)
    }

    /// Entry HTML of a widget of an enabled `Widget` plugin, read from the
    /// verified, extracted package. `plugin` is the install id (UUID) or the
    /// package id. Capped at [`MAX_WIDGET_HTML_BYTES`]; refused in safe mode.
    ///
    /// # Errors
    /// Not found / not enabled / too large / not UTF-8 / safe mode.
    pub async fn widget_html(&self, plugin: &str, widget_id: &str) -> R<String> {
        self.init().await?;
        if self.safe_mode() {
            return Err(PluginError::SafeMode);
        }
        let p = {
            let map = self.plugins.read();
            map.values()
                .find(|p| p.plugin.id.to_string() == plugin || p.plugin.package_id == plugin)
                .cloned()
                .ok_or_else(|| PluginError::NotFound(format!("plugin {plugin}")))?
        };
        if p.plugin.state != PluginState::Enabled {
            return Err(PluginError::Conflict(format!("plugin {} is {:?}", p.plugin.package_id, p.plugin.state)));
        }
        let w = p
            .manifest
            .widgets
            .iter()
            .find(|w| w.id == widget_id)
            .ok_or_else(|| PluginError::NotFound(format!("widget `{widget_id}` of {}", p.plugin.package_id)))?;
        let bytes = self.package_file(p.plugin.id, &w.entry_html).await?;
        if bytes.len() > MAX_WIDGET_HTML_BYTES {
            return Err(PluginError::Invalid(format!("widget HTML larger than {MAX_WIDGET_HTML_BYTES} bytes")));
        }
        String::from_utf8(bytes).map_err(|_| PluginError::Invalid("widget HTML is not UTF-8".into()))
    }

    /// The enforcing host API for calls made in the context of `project`
    /// (grants of the calling plugin, audit, `PluginCallDenied`).
    #[must_use]
    pub fn host_for(&self, project: Option<ProjectId>) -> GrantedHost {
        GrantedHost::new(Arc::clone(&self.host), project)
    }

    /// Number of calls currently running.
    #[must_use]
    pub fn running_calls(&self) -> usize {
        self.running.lock().len()
    }

    /// Project-scoped convenience for agents: invokes by plugin + tool name.
    ///
    /// # Errors
    /// See [`Self::invoke`].
    pub async fn invoke_tool(
        &self,
        plugin_id: PluginId,
        tool: &str,
        input: Value,
        project_id: Option<ProjectId>,
        cancel: CancellationToken,
    ) -> R<InvokePluginResult> {
        self.invoke_with_cancel(InvokePluginParams { plugin_id, tool: tool.to_owned(), input, project_id }, cancel)
            .await
    }
}
