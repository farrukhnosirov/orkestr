//! `lsp.*` — language servers for the Code panel (ADR-0025).
//!
//! The core spawns a language server per editor client and project as a
//! child process (stdio JSON-RPC with `Content-Length` framing) and proxies
//! raw LSP messages:
//!
//! * client → server: `lsp.send { sessionId, message }` (one JSON-RPC message);
//! * server → client: ephemeral `LspMessage` events on the topic `lsp:<sessionId>`.
//!
//! Running a language server executes third-party code, so the first start
//! of a server (per resolved program + arguments + binary hash) needs a user
//! approval: `lsp.start` fails with `approval_required` and an `approvalId`;
//! after the user approved it (`approvals.resolve`), `lsp.start` is called
//! again with that `approvalId` and the decision is remembered. A crashed
//! server is restarted with exponential backoff (`LspStatus` events, the
//! client re-initializes on `running` after `restarting`). Servers are
//! killed on `lsp.stop`, project deletion, emergency stop, core shutdown and
//! after 10 minutes without client traffic.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ApprovalId, LspSessionId, ProjectId};

use super::method;

/// Where a language server binary was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum LspServerSource {
    /// Bundled in an installed extension.
    Extension {
        /// Extension id.
        #[serde(rename = "extensionId")]
        extension_id: String,
    },
    /// Found on `PATH`.
    Path,
}

/// A language server Orkestr knows how to launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspServerInfo {
    /// Recipe id (`rust-analyzer`, `pyright`, `typescript`, …).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Monaco / VS Code language ids it serves.
    pub languages: Vec<String>,
    /// Whether a runnable binary was found.
    pub available: bool,
    /// Where it was found.
    pub source: Option<LspServerSource>,
    /// Resolved command line (display only).
    pub command: Option<String>,
    /// Whether the user already approved running exactly this command.
    pub trusted: bool,
    /// How to install it (shell command or extension id), for the UI.
    pub install_hint: String,
}

/// Lifecycle of a language server session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LspState {
    /// Process starting.
    Starting,
    /// Running; the client may (re-)initialize.
    Running,
    /// Crashed; waiting for the backoff before a restart.
    Restarting,
    /// Stopped by request.
    Stopped,
    /// Gave up (too many crashes or spawn failure).
    Failed,
}

/// A running (or recently finished) language server session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspSessionInfo {
    /// Session id (subscribe to `lsp:<sessionId>`).
    pub session_id: LspSessionId,
    /// Recipe id.
    pub server_id: String,
    /// Project.
    pub project_id: ProjectId,
    /// `file://` URI of the project root (the workspace folder to initialize with).
    pub root_uri: String,
    /// State.
    pub state: LspState,
    /// Restarts after crashes so far.
    pub restarts: u32,
    /// Process id while running.
    pub pid: Option<u32>,
    /// Last error (spawn failure, exit status, stderr tail).
    pub error: Option<String>,
}

/// `lsp.servers` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspServersParams {
    /// Re-scan `PATH` and extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `lsp.servers` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspServersResult {
    /// Known servers.
    pub servers: Vec<LspServerInfo>,
    /// Live sessions.
    pub sessions: Vec<LspSessionInfo>,
}

/// `lsp.start` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspStartParams {
    /// Project (its root folder is the server's workspace and cwd).
    pub project_id: ProjectId,
    /// Recipe id.
    pub server_id: String,
    /// The approved approval from a previous `approval_required` answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
}

/// `lsp.start` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspStartResult {
    /// The new session.
    pub session: LspSessionInfo,
}

/// `lsp.send` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspSendParams {
    /// Session.
    pub session_id: LspSessionId,
    /// One JSON-RPC 2.0 message (request, response or notification), serialized.
    pub message: String,
}

/// `lsp.stop` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspSessionParams {
    /// Session.
    pub session_id: LspSessionId,
}

/// `lsp.revoke` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspRevokeParams {
    /// Recipe id whose remembered approvals are forgotten.
    pub server_id: String,
}

/// `LspMessage` event payload (ephemeral, topic `lsp:<sessionId>` only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspMessagePayload {
    /// Session.
    pub session_id: LspSessionId,
    /// One JSON-RPC message from the server, serialized.
    pub message: String,
}

/// `LspStatus` event payload (ephemeral).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LspStatusPayload {
    /// Session state.
    pub session: LspSessionInfo,
}

method!(
    /// `lsp.servers` — known servers and live sessions.
    Servers, "lsp.servers", LspServersParams, LspServersResult
);
method!(
    /// `lsp.start` — start a server for a project (approval on first use).
    Start, "lsp.start", LspStartParams, LspStartResult
);
method!(
    /// `lsp.send` — forward one message to the server.
    Send, "lsp.send", LspSendParams, super::Empty
);
method!(
    /// `lsp.stop` — stop a session.
    Stop, "lsp.stop", LspSessionParams, super::Empty
);
method!(
    /// `lsp.revoke` — forget the approvals of a server.
    Revoke, "lsp.revoke", LspRevokeParams, super::Empty
);
