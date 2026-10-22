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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use orkestr_domain::{
    now, AuditOutcome, Capability, PermissionRequest, Plugin, PluginId, PluginKind, PluginState, ProjectId, ToolDecl,
    WidgetDecl,
};
use orkestr_events::EventBus;
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

use crate::host::{GrantedHost, HostEnv, RpcReader};
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
            tokio::task::spawn_blocking(move || sb.compile(&wasm))
                .await
                .map_err(|e| PluginError::Internal(e.to_string()))??;
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
        }
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
        if self.safe_mode() {
            return Err(PluginError::SafeMode);
        }
        let p = self.cached(params.plugin_id)?;
        match p.plugin.state {
            PluginState::Enabled => {}
            PluginState::Killed => return Err(PluginError::Killed),
            s => return Err(PluginError::Conflict(format!("plugin {} is {s:?}", p.plugin.package_id))),
        }
        if !p.manifest.tools.iter().any(|t| t.name == params.tool) {
            return Err(PluginError::NotFound(format!("tool `{}` of {}", params.tool, p.plugin.package_id)));
        }
        let module = self.module(&p).await?;
        let input = serde_json::to_vec(&json!({ "tool": params.tool, "input": params.input }))
            .map_err(|e| PluginError::Invalid(e.to_string()))?;
        let ctl = CallControl { killed: Arc::default(), cancel };
        let call_id = self.next_call.fetch_add(1, Ordering::Relaxed);
        self.running.lock().insert(call_id, Running { plugin: p.plugin.id, ctl: ctl.clone() });
        // A kill that raced the registration above.
        if self.cached(p.plugin.id).is_ok_and(|q| q.plugin.state != PluginState::Enabled) || self.safe_mode() {
            ctl.killed.store(true, Ordering::SeqCst);
        }
        let host = Arc::new(GrantedHost::new(Arc::clone(&self.host), params.project_id));
        let started = Instant::now();
        let result = self.sandbox()?.call(module, p.plugin.id, input, p.manifest.limits, host, ctl).await;
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
                self.record_trap(p.plugin.id, &params.tool, &msg, started).await;
                Err(PluginError::Trap(msg))
            }
            Err(e) => Err(e),
        }
    }

    async fn record_trap(&self, id: PluginId, tool: &str, msg: &str, started: Instant) {
        let _ = self
            .store
            .log(id, "error", &format!("tool `{tool}` trapped after {} ms: {msg}", started.elapsed().as_millis()))
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
