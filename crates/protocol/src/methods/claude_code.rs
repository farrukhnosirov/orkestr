//! `claudeCode.*` — native Claude Code chat panel over the official `claude` CLI.
//!
//! Orkestr runs the user's own, official Claude Code binary (found in the
//! `anthropic.claude-code` VS Code extension Orkestr installed, in the native
//! extensions directory, on `PATH`, or at a path the user pinned) — one
//! long-lived process per chat tab:
//!
//! ```text
//! claude --print --input-format stream-json --output-format stream-json --verbose
//!        --include-partial-messages --permission-prompt-tool stdio
//!        [--model M] [--permission-mode M] [--resume ID]
//! ```
//!
//! User turns go to the CLI's stdin as stream-json lines; its output is parsed
//! into [`ClaudeCodeEvent`]s, streamed (never persisted) on the topic
//! `claude:<sessionId>` as `ClaudeCodeEvent` events. Session state changes are
//! `ClaudeCodeStatus` events (topics `claude:<sessionId>` and `project:<id>`).
//!
//! Permissions: the CLI asks Orkestr before a tool runs (`can_use_tool`
//! control requests). The core classifies shell commands with the
//! `DangerousActionGuard` (forbidden → denied automatically, dangerous → asked
//! every time, never remembered), records every decision in the audit log and
//! shows an inline card; the user answers with `claudeCode.respondPermission`.
//!
//! Orkestr never reads Claude Code's credential files: the login state comes
//! from the CLI itself (`claude auth status`, the session's `initialize`
//! answer, or an authentication error). `claudeCode.loginInTerminal` opens a
//! terminal running `claude auth login` so the user signs in themselves.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use orkestr_domain::{ClaudeSessionId, DangerCategory, ProjectId, Timestamp};

use super::method;
use super::terminal::TerminalSession;

/// Where the Claude Code binary was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudeBinarySource {
    /// The user pinned this path (`claudeCode.setBinaryPath`).
    Override,
    /// Bundled in the `anthropic.claude-code` extension of Orkestr's VS Code.
    VscodeExtension,
    /// Bundled in the extension installed in Orkestr's native extensions directory.
    NativeExtension,
    /// Found on `PATH`.
    Path,
}

/// The Claude Code binary Orkestr runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeBinaryInfo {
    /// Absolute path.
    pub path: String,
    /// Where it was found.
    pub source: ClaudeBinarySource,
    /// `claude --version` (e.g. `2.1.292`), when the probe answered.
    pub version: Option<String>,
}

/// Login state reported by the CLI itself (never read from credential files).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeAuthInfo {
    /// Whether the CLI has a usable login.
    pub logged_in: bool,
    /// `claude.ai`, `apiKey`, … as reported.
    pub auth_method: Option<String>,
    /// Subscription (`max`, `pro`, …) when signed in with claude.ai.
    pub subscription_type: Option<String>,
    /// Account e-mail, when reported.
    pub email: Option<String>,
}

/// Permission mode of a session (the CLI's `--permission-mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudePermissionMode {
    /// Ask before edits and commands.
    #[default]
    Default,
    /// Accept file edits automatically; ask for commands.
    AcceptEdits,
    /// Plan only: no edits, no commands.
    Plan,
    /// No permission prompts at all (explicit danger confirmation per session).
    BypassPermissions,
}

impl ClaudePermissionMode {
    /// The CLI's spelling.
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    /// Parses the CLI's spelling (`None` for modes Orkestr does not offer).
    #[must_use]
    pub fn from_cli(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "acceptEdits" => Some(Self::AcceptEdits),
            "plan" => Some(Self::Plan),
            "bypassPermissions" => Some(Self::BypassPermissions),
            _ => None,
        }
    }
}

/// Lifecycle of a chat session process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudeSessionState {
    /// The process is starting.
    Starting,
    /// Ready for the next message.
    Idle,
    /// Working on a turn.
    Running,
    /// Waiting for the user to answer a permission request.
    WaitingPermission,
    /// The process ended (stopped, emergency stop, or it exited).
    Exited,
    /// Could not start / crashed (see `error`).
    Failed,
}

impl ClaudeSessionState {
    /// Whether the process is gone.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Exited | Self::Failed)
    }
}

/// A live (or just finished) chat session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSessionInfo {
    /// Orkestr session id (subscribe to `claude:<sessionId>`).
    pub session_id: ClaudeSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// Working directory (project root or one of its worktrees).
    pub cwd: String,
    /// The CLI's own conversation id (resumable), once known.
    pub claude_session_id: Option<String>,
    /// Model alias or id requested / reported.
    pub model: Option<String>,
    /// Current permission mode.
    pub permission_mode: ClaudePermissionMode,
    /// Whether the process was launched allowing `bypassPermissions`.
    pub bypass_allowed: bool,
    /// State.
    pub state: ClaudeSessionState,
    /// Title (first prompt, shortened).
    pub title: Option<String>,
    /// Process id while running.
    pub pid: Option<u32>,
    /// Start time.
    pub started_at: Timestamp,
    /// Last error (spawn failure, exit status, stderr tail).
    pub error: Option<String>,
}

/// `claudeCode.status` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeStatusParams {
    /// Re-run discovery and `claude auth status` instead of using the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `claudeCode.status` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeCodeStatus {
    /// The binary in use (`null` = Claude Code not found).
    pub binary: Option<ClaudeBinaryInfo>,
    /// Path pinned by the user, if any.
    pub binary_override: Option<String>,
    /// Why the pinned path is not used (missing / not a file).
    pub override_error: Option<String>,
    /// Login state (`null` while unknown).
    pub auth: Option<ClaudeAuthInfo>,
    /// Why the login state could not be determined.
    pub auth_error: Option<String>,
    /// Live sessions (all projects).
    pub sessions: Vec<ClaudeSessionInfo>,
}

/// A past conversation of the project (from the CLI's own transcripts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeHistoryEntry {
    /// The CLI's conversation id (`--resume`).
    pub claude_session_id: String,
    /// Title the CLI generated, when present.
    pub title: Option<String>,
    /// First user prompt (shortened).
    pub first_prompt: Option<String>,
    /// First message time.
    pub created_at: Option<Timestamp>,
    /// Last modification time.
    pub updated_at: Timestamp,
    /// Number of user + assistant messages.
    pub messages: u32,
}

/// `claudeCode.sessions` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSessionsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Directory whose history to list (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
}

/// `claudeCode.sessions` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSessionsResult {
    /// Live sessions of the project.
    pub live: Vec<ClaudeSessionInfo>,
    /// Past conversations, newest first.
    pub history: Vec<ClaudeHistoryEntry>,
    /// Orkestr's own labels of conversations (rename / pin / hide), any folder.
    #[serde(default)]
    pub meta: Vec<ClaudeChatMeta>,
}

/// `claudeCode.start` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeStartParams {
    /// Project.
    pub project_id: ProjectId,
    /// Working directory: the project root (default), a folder inside it or one of its worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
    /// Model alias (`sonnet`, `opus`, `haiku`, …) or id; default = the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// Initial permission mode (default `default`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub permission_mode: Option<ClaudePermissionMode>,
    /// Required (`true`) to start in `bypassPermissions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirm_bypass: Option<bool>,
    /// Conversation to resume (`claudeSessionId` from `claudeCode.sessions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resume: Option<String>,
    /// Extra folders Claude may read and edit (`--add-dir`; inside the project or its worktrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub add_dirs: Option<Vec<String>>,
    /// Model used when the main one is overloaded (`--fallback-model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fallback_model: Option<String>,
}

/// `claudeCode.start` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeStartResult {
    /// The new session.
    pub session: ClaudeSessionInfo,
    /// Earlier messages of a resumed conversation (from its transcript).
    pub history: Vec<ClaudeCodeEvent>,
}

/// `claudeCode.send` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSendParams {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// The user's message (slash commands such as `/compact` pass through).
    pub text: String,
    /// Images sent with the message (pasted / attached).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub images: Option<Vec<ClaudeImage>>,
}

/// An image attached to a message (base64, at most 5 MB decoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeImage {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
    /// Base64 data (no `data:` prefix).
    pub data: String,
}

/// Params addressing one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSessionParams {
    /// Session.
    pub session_id: ClaudeSessionId,
}

/// `claudeCode.setMode` params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSetModeParams {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// New mode.
    pub mode: ClaudePermissionMode,
    /// Required (`true`) for `bypassPermissions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirm_bypass: Option<bool>,
}

/// `claudeCode.setModel` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSetModelParams {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// Model alias or id (`default` = the CLI's default).
    pub model: String,
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudePermissionDecision {
    /// Run this once.
    AllowOnce,
    /// Run it and stop asking for this kind of action in this session.
    AllowSession,
    /// Do not run it.
    Deny,
}

/// `claudeCode.respondPermission` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeRespondPermissionParams {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// `requestId` of the `permissionRequest` event.
    pub request_id: String,
    /// Decision.
    pub decision: ClaudePermissionDecision,
    /// Optional note sent to Claude with a denial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}

/// `claudeCode.loginInTerminal` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeLoginParams {
    /// Project whose terminal list shows the login terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// `claudeCode.loginInTerminal` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeLoginResult {
    /// The terminal running `claude auth login`.
    pub terminal: TerminalSession,
}

/// `claudeCode.setBinaryPath` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSetBinaryPathParams {
    /// Absolute path of a `claude` binary; `null` / empty = automatic discovery.
    #[serde(default)]
    pub path: Option<String>,
}

/// `claudeCode.transcript` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeTranscriptParams {
    /// Project.
    pub project_id: ProjectId,
    /// The CLI's conversation id.
    pub claude_session_id: String,
    /// Directory the conversation ran in (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
}

/// `claudeCode.transcript` result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeTranscriptResult {
    /// Messages, oldest first (capped; hidden reasoning is never included).
    pub events: Vec<ClaudeCodeEvent>,
    /// Whether older messages were left out.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Streamed events
// ---------------------------------------------------------------------------

/// A model the CLI offers (from its `initialize` answer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeModelOption {
    /// Value for `--model` / `setModel` (`default`, `opus`, `sonnet`, …).
    pub value: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: Option<String>,
    /// Effort levels the model supports (`low` … `max`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort_levels: Option<Vec<String>>,
}

/// A slash command the CLI offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSlashCommand {
    /// Name without the slash.
    pub name: String,
    /// Description.
    pub description: String,
    /// Argument hint.
    pub argument_hint: Option<String>,
    /// Built into Claude Code (`false` = custom command, skill, plugin or MCP prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub builtin: Option<bool>,
}

/// Severity of a command flagged by the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClaudeRiskLevel {
    /// Needs an explicit decision every time.
    Dangerous,
    /// Hard-denied by policy.
    Forbidden,
}

/// Orkestr's risk classification of a requested command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeRisk {
    /// Level.
    pub level: ClaudeRiskLevel,
    /// Category.
    pub category: DangerCategory,
    /// Reason (English, from the guard).
    pub reason: String,
}

/// Token usage of a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeUsage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write_tokens: u64,
}

/// One item of a chat session's stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ClaudeCodeEvent {
    /// Models, slash commands and account, from the CLI's `initialize` answer.
    Capabilities {
        /// Models.
        models: Vec<ClaudeModelOption>,
        /// Slash commands.
        commands: Vec<ClaudeSlashCommand>,
        /// Account (when signed in).
        account: Option<ClaudeAuthInfo>,
    },
    /// The conversation started (`system/init`).
    Init {
        /// The CLI's conversation id.
        claude_session_id: String,
        /// Model in use.
        model: Option<String>,
        /// Working directory.
        cwd: Option<String>,
        /// Tool names.
        tools: Vec<String>,
        /// Permission mode (CLI spelling).
        permission_mode: Option<String>,
    },
    /// A user message (sent from Orkestr, or from a resumed transcript).
    UserMessage {
        /// Text.
        text: String,
    },
    /// Streamed assistant text (partial).
    TextDelta {
        /// Text appended to the current block.
        text: String,
        /// Sub-agent tool call this belongs to.
        parent_tool_use_id: Option<String>,
    },
    /// A complete assistant text block (replaces the streamed partial text).
    Text {
        /// Text.
        text: String,
        /// Sub-agent tool call this belongs to.
        parent_tool_use_id: Option<String>,
        /// Written by the CLI itself (slash command output, API/auth errors).
        synthetic: bool,
    },
    /// Claude started thinking (content is never shown or stored).
    Thinking,
    /// Claude calls a tool.
    ToolUse {
        /// Tool call id.
        id: String,
        /// Tool name (`Read`, `Edit`, `Bash`, `mcp__…`).
        name: String,
        /// Tool input.
        input: Value,
        /// Sub-agent tool call this belongs to.
        parent_tool_use_id: Option<String>,
    },
    /// A tool finished.
    ToolResult {
        /// Tool call id.
        tool_use_id: String,
        /// Text output (truncated).
        content: String,
        /// Whether it failed or was denied.
        is_error: bool,
        /// Sub-agent tool call this belongs to.
        parent_tool_use_id: Option<String>,
    },
    /// Claude asks to run a tool.
    PermissionRequest {
        /// Id to answer with (`claudeCode.respondPermission`).
        request_id: String,
        /// Tool name.
        tool_name: String,
        /// Tool call id.
        tool_use_id: Option<String>,
        /// Tool input.
        input: Value,
        /// The CLI's short description.
        description: Option<String>,
        /// Path outside the allowed directories that triggered the request.
        blocked_path: Option<String>,
        /// Orkestr's risk classification (shell commands).
        risk: Option<ClaudeRisk>,
        /// Whether "allow for this session" is offered (never for dangerous commands).
        can_remember: bool,
    },
    /// A permission request was answered.
    PermissionResolved {
        /// Request id.
        request_id: String,
        /// Decision.
        decision: ClaudePermissionDecision,
        /// Decided by Orkestr's policy (forbidden command) rather than the user.
        by_policy: bool,
        /// Note sent with a denial.
        message: Option<String>,
    },
    /// The permission mode changed (CLI spelling).
    ModeChanged {
        /// Mode.
        mode: String,
    },
    /// The model changed.
    ModelChanged {
        /// Model.
        model: String,
    },
    /// Output of a local slash command (`/model`, …).
    CommandOutput {
        /// Text.
        text: String,
    },
    /// A turn finished.
    Result {
        /// Whether it failed / was interrupted.
        is_error: bool,
        /// `success`, `error_during_execution`, `error_max_turns`, …
        subtype: String,
        /// Final text / error.
        result: Option<String>,
        /// Wall time.
        duration_ms: Option<u64>,
        /// Turns.
        num_turns: Option<u32>,
        /// API-equivalent cost reported by the CLI (subscriptions may show 0).
        total_cost_usd: Option<f64>,
        /// Tokens.
        usage: ClaudeUsage,
    },
    /// Rate-limit information from the CLI.
    RateLimit {
        /// `allowed`, `allowed_warning`, `rejected`.
        status: String,
        /// Reset time (unix seconds).
        resets_at: Option<i64>,
        /// Window (`five_hour`, `seven_day`, …).
        window: Option<String>,
        /// Used share of the window (0..1), when reported.
        utilization: Option<f64>,
    },
    /// Claude wrote a file (absolute path).
    FileChanged {
        /// Path.
        path: String,
    },
    /// An error (process, protocol or account).
    Error {
        /// Message.
        message: String,
        /// The CLI needs a login (`claudeCode.loginInTerminal`).
        auth_required: bool,
    },
    /// The current turn was interrupted.
    Interrupted,
    /// The process ended.
    Exited {
        /// Exit code.
        code: Option<i32>,
    },
    /// The CLI's command list changed (plugins / skills loaded).
    CommandsChanged {
        /// Slash commands.
        commands: Vec<ClaudeSlashCommand>,
    },
}

/// `ClaudeCodeEvent` event payload (ephemeral; topic `claude:<sessionId>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeCodeEventPayload {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// The item.
    pub event: ClaudeCodeEvent,
}

/// `ClaudeCodeStatus` event payload (ephemeral; topics `claude:<sessionId>`, `project:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeCodeStatusPayload {
    /// Session.
    pub session: ClaudeSessionInfo,
}

// ---------------------------------------------------------------------------
// Plan usage, context, MCP, chat labels
// ---------------------------------------------------------------------------

/// One plan limit window (from the CLI's `get_usage` control request).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeLimitWindow {
    /// Used share in percent (0‥100).
    pub percent: f64,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
    /// Model / scope label (`Opus`, `Fable`, …) of a per-model weekly limit.
    pub label: Option<String>,
    /// `normal`, `warning`, `critical`, … as reported.
    pub severity: Option<String>,
}

/// `claudeCode.usage` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeUsageParams {
    /// Ask this live session's process (cheaper than a probe process).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<ClaudeSessionId>,
    /// Ignore the cache (about a minute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `claudeCode.usage` result: the subscription's limits (the CLI's `/usage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudePlanUsage {
    /// Whether the CLI reported plan limits (false for API keys / errors).
    pub available: bool,
    /// Subscription (`max`, `pro`, …).
    pub subscription_type: Option<String>,
    /// Current session (5-hour) window.
    pub session: Option<ClaudeLimitWindow>,
    /// Weekly window (all models).
    pub weekly: Option<ClaudeLimitWindow>,
    /// Weekly windows of single models (Opus, Sonnet, …).
    pub weekly_models: Vec<ClaudeLimitWindow>,
    /// Extra usage (credits) enabled.
    pub extra_usage_enabled: Option<bool>,
    /// When this was fetched.
    pub fetched_at: Timestamp,
    /// Why no data is available.
    pub error: Option<String>,
}

/// One category of the context window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeContextCategory {
    /// Name as reported (`System prompt`, `Messages`, `Free space`, …).
    pub name: String,
    /// Tokens.
    pub tokens: u64,
    /// `used`, `deferred` or `free`.
    pub kind: String,
}

/// `claudeCode.contextUsage` result (the CLI's `/context`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeContextUsage {
    /// Tokens in use.
    pub total_tokens: u64,
    /// Context window size.
    pub max_tokens: u64,
    /// Used share in percent.
    pub percentage: f64,
    /// Breakdown.
    pub categories: Vec<ClaudeContextCategory>,
}

/// An MCP server of the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeMcpServer {
    /// Name.
    pub name: String,
    /// `connected`, `needs-auth`, `failed`, `pending`, `disabled`.
    pub status: String,
    /// Error text.
    pub error: Option<String>,
    /// `user`, `project`, `local`, `claudeai`, `dynamic`, …
    pub scope: Option<String>,
    /// Number of tools.
    pub tools: u32,
}

/// `claudeCode.mcpStatus` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeMcpStatusResult {
    /// Servers.
    pub servers: Vec<ClaudeMcpServer>,
}

/// `claudeCode.setThinking` params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSetThinkingParams {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// Extended thinking on / off.
    pub enabled: bool,
}

/// Orkestr's label of a conversation (never written to Claude's files).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeChatMeta {
    /// The CLI's conversation id.
    pub claude_session_id: String,
    /// Custom title.
    pub title: Option<String>,
    /// Pinned to the top of the list.
    pub pinned: bool,
    /// Removed from the list (the transcript stays on disk).
    pub hidden: bool,
}

/// `claudeCode.setChatMeta` params (fields left out stay unchanged).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSetChatMetaParams {
    /// The CLI's conversation id.
    pub claude_session_id: String,
    /// New title (empty = back to the automatic one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// Pin / unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned: Option<bool>,
    /// Hide / show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hidden: Option<bool>,
}

/// `claudeCode.capabilities` result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeCapabilitiesResult {
    /// The session's last `capabilities` event (models, commands, account), once the CLI answered.
    pub event: Option<ClaudeCodeEvent>,
}

/// `claudeCode.openInTerminal` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeOpenInTerminalParams {
    /// Project.
    pub project_id: ProjectId,
    /// Conversation to continue (`claude --resume <id>`); none = a new interactive session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claude_session_id: Option<String>,
    /// Folder (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
}

method!(
    /// `claudeCode.status` — binary, login state and live sessions.
    Status, "claudeCode.status", ClaudeStatusParams, ClaudeCodeStatus
);
method!(
    /// `claudeCode.sessions` — live sessions + past conversations of a project.
    Sessions, "claudeCode.sessions", ClaudeSessionsParams, ClaudeSessionsResult
);
method!(
    /// `claudeCode.start` — starts a chat session (new or resumed).
    Start, "claudeCode.start", ClaudeStartParams, ClaudeStartResult
);
method!(
    /// `claudeCode.send` — sends a user message.
    Send, "claudeCode.send", ClaudeSendParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.interrupt` — interrupts the current turn (Esc).
    Interrupt, "claudeCode.interrupt", ClaudeSessionParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.setMode` — changes the permission mode.
    SetMode, "claudeCode.setMode", ClaudeSetModeParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.setModel` — changes the model.
    SetModel, "claudeCode.setModel", ClaudeSetModelParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.respondPermission` — answers a permission request.
    RespondPermission, "claudeCode.respondPermission", ClaudeRespondPermissionParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.stop` — ends the session (kills the process tree).
    Stop, "claudeCode.stop", ClaudeSessionParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.loginInTerminal` — opens a terminal running `claude auth login`.
    LoginInTerminal, "claudeCode.loginInTerminal", ClaudeLoginParams, ClaudeLoginResult
);
method!(
    /// `claudeCode.setBinaryPath` — pins (or clears) the binary path.
    SetBinaryPath, "claudeCode.setBinaryPath", ClaudeSetBinaryPathParams, ClaudeCodeStatus
);
method!(
    /// `claudeCode.transcript` — messages of a past conversation.
    Transcript, "claudeCode.transcript", ClaudeTranscriptParams, ClaudeTranscriptResult
);

method!(
    /// `claudeCode.capabilities` — models / commands / account of a live session (UI re-attach).
    Capabilities, "claudeCode.capabilities", ClaudeSessionParams, ClaudeCapabilitiesResult
);
method!(
    /// `claudeCode.usage` — plan limits: session (5 h) and weekly windows.
    Usage, "claudeCode.usage", ClaudeUsageParams, ClaudePlanUsage
);
method!(
    /// `claudeCode.contextUsage` — context window usage of a session.
    ContextUsage, "claudeCode.contextUsage", ClaudeSessionParams, ClaudeContextUsage
);
method!(
    /// `claudeCode.mcpStatus` — MCP servers of a session.
    McpStatus, "claudeCode.mcpStatus", ClaudeSessionParams, ClaudeMcpStatusResult
);
method!(
    /// `claudeCode.setThinking` — extended thinking on / off.
    SetThinking, "claudeCode.setThinking", ClaudeSetThinkingParams, ClaudeSessionInfo
);
method!(
    /// `claudeCode.setChatMeta` — renames / pins / hides a conversation in Orkestr's list.
    SetChatMeta, "claudeCode.setChatMeta", ClaudeSetChatMetaParams, ClaudeChatMeta
);
method!(
    /// `claudeCode.openInTerminal` — the interactive `claude` (optionally `--resume <id>`) in an Orkestr terminal.
    OpenInTerminal, "claudeCode.openInTerminal", ClaudeOpenInTerminalParams, ClaudeLoginResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_cli_spelling() {
        for m in [
            ClaudePermissionMode::Default,
            ClaudePermissionMode::AcceptEdits,
            ClaudePermissionMode::Plan,
            ClaudePermissionMode::BypassPermissions,
        ] {
            assert_eq!(ClaudePermissionMode::from_cli(m.as_cli()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), serde_json::json!(m.as_cli()));
        }
        assert_eq!(ClaudePermissionMode::from_cli("dontAsk"), None);
    }

    #[test]
    fn events_are_type_tagged_camel_case() {
        let e = ClaudeCodeEvent::ToolResult {
            tool_use_id: "t".into(),
            content: "ok".into(),
            is_error: false,
            parent_tool_use_id: None,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "toolResult");
        assert_eq!(v["toolUseId"], "t");
        assert_eq!(serde_json::to_value(ClaudeCodeEvent::Thinking).unwrap(), serde_json::json!({"type": "thinking"}));
    }
}
