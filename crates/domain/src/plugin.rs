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

/// An installed plugin. Permissions are explicit; widgets get no FS/secrets by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Plugin {
    /// Id.
    pub id: PluginId,
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
