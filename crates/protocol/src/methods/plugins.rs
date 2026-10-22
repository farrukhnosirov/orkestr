//! `plugins.*` — Plugin SDK host: signed `.orkp` packages, WASM sandbox,
//! permission grants, per-plugin disable, global kill switch and safe mode
//! (Phase 7, ADR-0034). **Owner: WP1** (handlers in `core/src/rpc/plugins.rs`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    Capability, PermissionRequest, Plugin, PluginId, PluginKind, PluginManifest, PluginState, Timestamp,
};

use super::{method, Empty};

/// A permission the user granted (or denied) to a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginGrant {
    /// Capability.
    pub capability: Capability,
    /// Scope (host, secret ref, method); `None` = the whole capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<String>,
    /// Granted (`true`) or denied (`false`).
    pub allowed: bool,
    /// Decision time.
    pub decided_at: Timestamp,
}

/// An installed plugin with its manifest, grants and runtime status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginInfo {
    /// The installed plugin.
    pub plugin: Plugin,
    /// Manifest.
    pub manifest: PluginManifest,
    /// Decided permissions.
    pub grants: Vec<PluginGrant>,
    /// Requested permissions not decided yet (the install prompt shows these).
    pub pending_grants: Vec<PermissionRequest>,
    /// Publisher key fingerprint the package was signed with (`None` = unsigned dev package).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signer_fingerprint: Option<String>,
    /// Package sha256 (hex).
    pub sha256: String,
    /// Last error (trap, timeout, quarantine reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
}

/// `plugins.list` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListPluginsParams {
    /// Only plugins contributing this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<PluginKind>,
}

/// `plugins.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListPluginsResult {
    /// Installed plugins.
    pub plugins: Vec<PluginInfo>,
    /// Safe mode: no plugin code is loaded (persisted across restarts).
    pub safe_mode: bool,
}

/// Params naming one plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginIdParams {
    /// Plugin.
    pub plugin_id: PluginId,
}

/// Result carrying one plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginResult {
    /// The plugin.
    pub plugin: PluginInfo,
}

/// `plugins.install` params: a local `.orkp` file **or** the package bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallPluginParams {
    /// Path of a `.orkp` file on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Package bytes (base64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub package_base64: Option<String>,
    /// Install source label (`marketplace:<sourceId>/<entryId>`, `file`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
}

/// `plugins.install` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallPluginResult {
    /// The installed plugin (state `Installed` until grants are decided).
    pub plugin: PluginInfo,
    /// Permissions the user must decide (`plugins.grant`).
    pub pending_grants: Vec<PermissionRequest>,
}

/// One permission decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrantDecision {
    /// Capability.
    pub capability: Capability,
    /// Scope (must match the request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<String>,
    /// Allow or deny.
    pub allow: bool,
}

/// `plugins.grant` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrantPluginParams {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Decisions.
    pub decisions: Vec<GrantDecision>,
    /// Enable the plugin once every request is decided.
    #[serde(default)]
    pub enable: bool,
}

/// `plugins.enable` / `plugins.disable` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginStateParams {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Reason (audited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// `plugins.killAll` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KillAllPluginsParams {
    /// Reason (audited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// `plugins.killAll` result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KillAllPluginsResult {
    /// Plugins moved to `Killed`.
    pub killed: u32,
    /// In-flight calls interrupted.
    pub calls_interrupted: u32,
}

/// `plugins.setSafeMode` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetSafeModeParams {
    /// Safe mode on/off.
    pub enabled: bool,
}

/// `plugins.setSafeMode` result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SafeModeResult {
    /// Current safe mode.
    pub safe_mode: bool,
}

/// `plugins.invoke` params: call one tool of a code plugin directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InvokePluginParams {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Tool name (from the manifest).
    pub tool: String,
    /// JSON input.
    #[ts(type = "unknown")]
    pub input: serde_json::Value,
    /// Project context (for RPC reads / audit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<orkestr_domain::ProjectId>,
}

/// `plugins.invoke` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InvokePluginResult {
    /// JSON output.
    #[ts(type = "unknown")]
    pub output: serde_json::Value,
    /// Fuel consumed.
    pub fuel_used: u64,
    /// Wall-clock duration.
    pub duration_ms: u64,
}

/// `plugins.logs` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginLogsParams {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Max lines (newest last; default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// One plugin log line (plugin `Log` host calls, host decisions, traps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginLogLine {
    /// Time.
    pub ts: Timestamp,
    /// `trace|debug|info|warn|error`.
    pub level: String,
    /// Message (redacted).
    pub message: String,
}

/// `plugins.logs` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginLogsResult {
    /// Lines.
    pub lines: Vec<PluginLogLine>,
}

// --- events ------------------------------------------------------------------------------

/// `PluginInstalled`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginInstalledPayload {
    /// The plugin.
    pub plugin: PluginInfo,
}

/// `PluginStateChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginStateChangedPayload {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Package id.
    pub package_id: String,
    /// New state.
    pub state: PluginState,
    /// Why (user, kill switch, emergency stop, quarantine reason, uninstalled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// `PluginCallDenied`: a host call was refused (capability not granted, scope mismatch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginCallDeniedPayload {
    /// Plugin.
    pub plugin_id: PluginId,
    /// Package id.
    pub package_id: String,
    /// Host op (`HttpFetch`, `SecretGet`, `RpcRead`, ...).
    pub op: String,
    /// Target (host, secret ref, method), redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// Reason.
    pub reason: String,
}

method!(/// Installed plugins.
    List, "plugins.list", ListPluginsParams, ListPluginsResult);
method!(/// One plugin.
    Get, "plugins.get", PluginIdParams, PluginResult);
method!(/// Installs a signed `.orkp` package (local only).
    Install, "plugins.install", InstallPluginParams, InstallPluginResult);
method!(/// Decides requested permissions (local only).
    Grant, "plugins.grant", GrantPluginParams, PluginResult);
method!(/// Uninstalls a plugin.
    Uninstall, "plugins.uninstall", PluginIdParams, Empty);
method!(/// Enables a plugin (local only).
    Enable, "plugins.enable", PluginStateParams, PluginResult);
method!(/// Disables a plugin (emergency class).
    Disable, "plugins.disable", PluginStateParams, PluginResult);
method!(/// Global kill switch: stops every running call and kills every plugin.
    KillAll, "plugins.killAll", KillAllPluginsParams, KillAllPluginsResult);
method!(/// Safe mode on/off (local only).
    SetSafeMode, "plugins.setSafeMode", SetSafeModeParams, SafeModeResult);
method!(/// Invokes a plugin tool directly.
    Invoke, "plugins.invoke", InvokePluginParams, InvokePluginResult);
method!(/// Plugin log lines.
    Logs, "plugins.logs", PluginLogsParams, PluginLogsResult);
