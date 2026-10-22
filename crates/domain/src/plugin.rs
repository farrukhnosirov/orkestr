//! Plugins (§48) and MCP servers (§19).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::Capability;
use crate::common::Timestamp;
use crate::ids::{McpServerId, PluginId, ProjectId, SecretRefId};

/// Extension points a plugin may contribute to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PluginKind {
    /// AI provider adapter.
    Provider,
    /// Agent tool.
    Tool,
    /// MCP bundle.
    Mcp,
    /// UI panel.
    Panel,
    /// Dashboard widget.
    Widget,
    /// Agent template pack.
    AgentTemplate,
    /// Deployment provider.
    DeploymentProvider,
    /// Platform integration.
    PlatformIntegration,
}

/// Lifecycle state of an installed plugin (Phase 7, ADR-0034).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PluginState {
    /// Installed; requested permissions not (all) decided yet: no code runs.
    #[default]
    Installed,
    /// Enabled: code may run within its grants.
    Enabled,
    /// Disabled by the user.
    Disabled,
    /// Stopped by the global kill switch / emergency stop; re-enable explicitly.
    Killed,
    /// Refused to load (bad signature, ABI mismatch, repeated traps, revoked).
    Quarantined,
}

/// ABI version of code plugins (wasm exports `orkestr_alloc`, `orkestr_call`; import `orkestr.host_call`).
pub const PLUGIN_ABI_VERSION: u32 = 1;

/// One permission a plugin asks for in its manifest (shown in the install prompt).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PermissionRequest {
    /// Capability (e.g. `Network`, `Secrets`).
    pub capability: Capability,
    /// Scope: host for `Network` (`api.example.com`), secret ref for `Secrets`, method for RPC reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<String>,
    /// Why the plugin needs it (publisher-provided, shown verbatim).
    pub reason: String,
}

/// A tool a code plugin exposes to agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolDecl {
    /// Tool name (`^[a-z][a-z0-9_]{0,63}$`).
    pub name: String,
    /// Description shown to the model.
    pub description: String,
    /// JSON schema of the input.
    #[ts(type = "unknown")]
    pub input_schema: serde_json::Value,
}

/// A widget grid size (columns x rows of the 12-column dashboard grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetSize {
    /// Columns (1-12).
    pub w: u32,
    /// Rows.
    pub h: u32,
}

/// Data a widget may read: Read-class, non-local-only RPC methods and event kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DataBinding {
    /// RPC methods (`monitor.overview`, ...).
    #[serde(default)]
    pub methods: Vec<String>,
    /// Event kinds (`AlertRaised`, ...).
    #[serde(default)]
    pub events: Vec<String>,
}

/// A dashboard widget contributed by a plugin (runs in a sandboxed iframe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetDecl {
    /// Widget id, unique within the package.
    pub id: String,
    /// Title.
    pub title: String,
    /// Path of the HTML entry inside the package.
    pub entry_html: String,
    /// Supported sizes (first = default).
    pub sizes: Vec<WidgetSize>,
    /// Data binding allowlist.
    #[serde(default)]
    pub data_binding: DataBinding,
    /// JSON schema of the per-instance config.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub config_schema: serde_json::Value,
}

/// Resource limits of one plugin call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PluginLimits {
    /// Linear memory cap (MiB).
    pub memory_mb: u32,
    /// Fuel per call (wasmtime fuel units).
    pub fuel_per_call: u64,
    /// Wall-clock timeout per call (epoch deadline).
    pub timeout_ms: u32,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self { memory_mb: 64, fuel_per_call: 1_000_000_000, timeout_ms: 5_000 }
    }
}

/// `orkestr-plugin.json`: the manifest at the root of a `.orkp` package (ADR-0034).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginManifest {
    /// Package id (`publisher.name`, lowercase).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version (semver).
    pub version: String,
    /// Publisher.
    pub publisher: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// ABI version (must equal [`PLUGIN_ABI_VERSION`]).
    pub abi: u32,
    /// Contributions.
    pub kinds: Vec<PluginKind>,
    /// Path of the wasm module inside the package (code plugins only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub entry: Option<String>,
    /// Requested permissions.
    #[serde(default)]
    pub permissions: Vec<PermissionRequest>,
    /// Agent tools (kind `Tool`).
    #[serde(default)]
    pub tools: Vec<ToolDecl>,
    /// Dashboard widgets (kind `Widget`).
    #[serde(default)]
    pub widgets: Vec<WidgetDecl>,
    /// Deployment provider ids (kind `DeploymentProvider`).
    #[serde(default)]
    pub deploy_providers: Vec<String>,
    /// Paths of agent template JSON files inside the package (kind `AgentTemplate`).
    #[serde(default)]
    pub agent_templates: Vec<String>,
    /// Event kinds delivered to the plugin (kind `PlatformIntegration`).
    #[serde(default)]
    pub subscribes: Vec<String>,
    /// Per-call limits.
    #[serde(default)]
    pub limits: PluginLimits,
}

/// An installed plugin. Permissions are explicit; widgets get no FS/secrets by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Plugin {
    /// Id (install instance).
    pub id: PluginId,
    /// Package id from the manifest (`publisher.name`; Phase 7, appended).
    #[serde(default)]
    pub package_id: String,
    /// Lifecycle state (Phase 7, appended; `enabled` mirrors `state == Enabled`).
    #[serde(default)]
    pub state: PluginState,
    /// Package name.
    pub name: String,
    /// Version (semver).
    pub version: String,
    /// Publisher.
    pub publisher: String,
    /// Contributions.
    pub kinds: Vec<PluginKind>,
    /// Capabilities declared in the manifest.
    pub permissions: Vec<Capability>,
    /// Enabled flag.
    pub enabled: bool,
    /// Install source (path/url/marketplace id).
    pub source: String,
    /// Install time.
    pub installed_at: Timestamp,
}

/// How to reach an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "transport", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum McpTransport {
    /// Child process over stdio.
    Stdio {
        /// Executable.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// Non-secret env vars.
        env: BTreeMap<String, String>,
    },
    /// Streamable HTTP / SSE endpoint.
    Http {
        /// URL.
        url: String,
    },
}

/// Connection status of an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpStatus {
    /// Stopped.
    Stopped,
    /// Starting.
    Starting,
    /// Running.
    Running,
    /// Error.
    Error,
}

/// A registered MCP server (§19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpServer {
    /// Id.
    pub id: McpServerId,
    /// Name.
    pub name: String,
    /// Transport.
    pub transport: McpTransport,
    /// Status.
    pub status: McpStatus,
    /// Tools discovered on last connect.
    pub tools: Vec<String>,
    /// Capabilities its tools require.
    pub permissions: Vec<Capability>,
    /// Secret references injected at launch.
    pub secret_refs: Vec<SecretRefId>,
    /// Linked projects (empty = global).
    pub project_ids: Vec<ProjectId>,
    /// Enabled flag.
    pub enabled: bool,
    /// Creation time.
    pub created_at: Timestamp,
}
