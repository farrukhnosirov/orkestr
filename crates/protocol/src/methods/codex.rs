//! `codex.*` — the user's ChatGPT subscription through the official OpenAI
//! Codex CLI (ADR-0047), and `engines.*` — running Claude Code and Codex
//! side by side.
//!
//! Orkestr runs the official `codex` binary (on `PATH`, or installed by
//! Orkestr into `<ORKESTR_HOME>/tools/codex` with `npm`, or a pinned path).
//! The native panel, status, limits, models, threads and the ChatGPT sign-in
//! go through one long-lived `codex app-server` process (JSON-RPC over
//! stdio):
//!
//! ```text
//! codex app-server            initialize → thread/start | thread/resume | thread/fork
//!                             → turn/start … item/* notifications … turn/completed
//! ```
//!
//! Approval requests of the CLI (`item/commandExecution/requestApproval`,
//! `item/fileChange/requestApproval`) are classified by the
//! `DangerousActionGuard` (forbidden → declined automatically), audited and
//! shown as inline cards; the user answers with `codex.respondApproval`.
//! Chat items stream (never persisted) on the topic `codex:<sessionId>` as
//! `CodexEvent` events; state changes are `CodexStatus` events.
//!
//! Orkestr never reads Codex's credential files: the login state comes from
//! the CLI (`account/read`), the sign-in is the CLI's own ChatGPT browser or
//! device-code flow (`account/login/start`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use orkestr_domain::{CodexSessionId, ProjectId, ProviderId, Timestamp};

use super::claude_code::ClaudeRisk;
use super::method;
use super::terminal::TerminalSession;

// ---------------------------------------------------------------------------
// Installation, login, account
// ---------------------------------------------------------------------------

/// Where the Codex binary was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexBinarySource {
    /// A path pinned by the user (provider setting).
    Override,
    /// Installed by Orkestr (`<ORKESTR_HOME>/tools/codex`).
    Orkestr,
    /// Found on `PATH` (e.g. `npm i -g @openai/codex`).
    Path,
}

/// The Codex binary in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexBinaryInfo {
    /// Absolute path.
    pub path: String,
    /// Where it was found.
    pub source: CodexBinarySource,
    /// `codex --version` (`0.163.0`).
    pub version: Option<String>,
}

/// The signed-in account (from the CLI's `account/read`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexAccount {
    /// `chatgpt`, `apiKey`, …
    pub kind: String,
    /// E-mail of a ChatGPT account.
    pub email: Option<String>,
    /// ChatGPT plan (`plus`, `pro`, `team`, …).
    pub plan: Option<String>,
}

/// How the ChatGPT sign-in runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexLoginMode {
    /// The browser opens chatgpt.com; the CLI receives the callback locally.
    #[default]
    Browser,
    /// Device code: open a URL anywhere and enter a one-time code.
    DeviceCode,
}

/// Phase of a sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexLoginPhase {
    /// Waiting for the user in the browser.
    Pending,
    /// Signed in.
    Succeeded,
    /// Failed (see `error`).
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

/// A ChatGPT sign-in started by Orkestr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexLoginState {
    /// The CLI's login id.
    pub login_id: Option<String>,
    /// Mode.
    pub mode: CodexLoginMode,
    /// Phase.
    pub phase: CodexLoginPhase,
    /// Browser mode: the URL to open (Orkestr opens it too).
    pub auth_url: Option<String>,
    /// Device-code mode: where to enter the code.
    pub verification_url: Option<String>,
    /// Device-code mode: the one-time code.
    pub user_code: Option<String>,
    /// Why it failed.
    pub error: Option<String>,
    /// When it started.
    pub started_at: Timestamp,
}

/// `codex.status` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexStatusParams {
    /// Re-probe the binary, the account and the latest version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `codex.status` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexStatus {
    /// The binary (`None` = not installed).
    pub binary: Option<CodexBinaryInfo>,
    /// Latest published version (`npm view @openai/codex version`).
    pub latest_version: Option<String>,
    /// A newer version than the installed one exists.
    pub update_available: bool,
    /// `npm` is available (needed by the installer).
    pub npm_available: bool,
    /// Signed in (ChatGPT or API key).
    pub logged_in: bool,
    /// The account.
    pub account: Option<CodexAccount>,
    /// The sign-in in progress / last finished.
    pub login: Option<CodexLoginState>,
    /// The Codex provider (registered automatically once signed in).
    pub provider_id: Option<ProviderId>,
    /// Live panel sessions.
    pub sessions: Vec<CodexSessionInfo>,
    /// Why the status is incomplete.
    pub error: Option<String>,
}

/// `codex.install` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexInstallParams {
    /// Update an existing installation to the latest version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub update: Option<bool>,
}

/// `codex.install` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexInstallResult {
    /// Installed successfully.
    pub ok: bool,
    /// Last lines of npm's output.
    pub log: String,
    /// The status afterwards.
    pub status: CodexStatus,
}

/// `codex.login` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexLoginParams {
    /// Browser (default) or device code.
    #[serde(default)]
    pub mode: CodexLoginMode,
}

/// `codex.openInTerminal` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexOpenInTerminalParams {
    /// Project whose terminal list shows the terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Run `codex login --device-auth` instead of the interactive CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub login: Option<bool>,
    /// Resume this thread (`codex resume <id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thread_id: Option<String>,
}

/// `codex.openInTerminal` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTerminalResult {
    /// The terminal.
    pub terminal: TerminalSession,
}

// ---------------------------------------------------------------------------
// Limits, models, threads
// ---------------------------------------------------------------------------

/// One rate-limit window of the ChatGPT plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexLimitWindow {
    /// Used share in percent (0‥100).
    pub percent: f64,
    /// Window length in minutes (300 = 5 hours, 10080 = a week).
    pub window_minutes: Option<u64>,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
}

/// `codex.limits` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexLimitsParams {
    /// Ignore the cache (about a minute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `codex.limits` result: the plan's usage windows (`account/rateLimits/read`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexLimits {
    /// The CLI reported limits (false = unknown).
    pub available: bool,
    /// Plan (`plus`, `pro`, …).
    pub plan: Option<String>,
    /// Shorter window (usually 5 hours).
    pub primary: Option<CodexLimitWindow>,
    /// Longer window (usually weekly).
    pub secondary: Option<CodexLimitWindow>,
    /// Credits balance, when the plan has credits.
    pub credits: Option<String>,
    /// Which limit was reached (when one was).
    pub limit_reached: Option<String>,
    /// Whether ordinary included usage is allowed right now.
    pub usage_allowed: Option<bool>,
    /// When this was fetched.
    pub fetched_at: Timestamp,
    /// Why no data is available.
    pub error: Option<String>,
}

/// One model of the Codex model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexModelOption {
    /// Model id (`gpt-5-codex`, …).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: String,
    /// Supported reasoning efforts (`low`, `medium`, `high`, …).
    pub efforts: Vec<String>,
    /// Default effort.
    pub default_effort: Option<String>,
    /// The CLI's default model.
    pub is_default: bool,
}

/// `codex.models` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexModelsParams {
    /// Ignore the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `codex.models` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexModelsResult {
    /// Models (live from the CLI, or the built-in list).
    pub models: Vec<CodexModelOption>,
    /// The list is the built-in fallback (the CLI did not answer).
    pub fallback: bool,
    /// Why the live list is unavailable.
    pub error: Option<String>,
}

/// A Codex conversation (thread) saved by the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexThread {
    /// Thread id.
    pub id: String,
    /// Name or first message.
    pub title: String,
    /// Working directory.
    pub cwd: Option<String>,
    /// Model.
    pub model: Option<String>,
    /// Created.
    pub created_at: Option<Timestamp>,
    /// Last activity.
    pub updated_at: Option<Timestamp>,
    /// Source thread of a fork.
    pub forked_from: Option<String>,
}

/// `codex.threads` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexThreadsParams {
    /// Only threads of this project's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Page size (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `codex.threads` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexThreadsResult {
    /// Saved threads, newest first.
    pub threads: Vec<CodexThread>,
    /// Live panel sessions (of the project, when given).
    pub live: Vec<CodexSessionInfo>,
    /// Why the list is unavailable.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Panel sessions
// ---------------------------------------------------------------------------

/// How much a panel session may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexApprovalMode {
    /// Read-only sandbox; anything else is asked.
    ReadOnly,
    /// Workspace-write sandbox; every non-trivial command / edit is asked (`untrusted`).
    #[default]
    Ask,
    /// Workspace-write sandbox; Codex asks only to leave it (`on-request`).
    Auto,
}

impl CodexApprovalMode {
    /// The CLI's sandbox mode.
    #[must_use]
    pub fn sandbox(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Ask | Self::Auto => "workspace-write",
        }
    }

    /// The CLI's approval policy.
    #[must_use]
    pub fn approval_policy(self) -> &'static str {
        match self {
            Self::ReadOnly | Self::Auto => "on-request",
            Self::Ask => "untrusted",
        }
    }
}

/// State of a panel session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexSessionState {
    /// Starting the thread.
    Starting,
    /// Waiting for a message.
    Idle,
    /// A turn is running.
    Running,
    /// Waiting for an approval.
    WaitingApproval,
    /// Ended.
    Exited,
    /// Failed (see `error`).
    Failed,
}

impl CodexSessionState {
    /// The session has ended.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Exited | Self::Failed)
    }
}

/// A live panel session (one thread on the shared app-server process).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexSessionInfo {
    /// Session id.
    pub session_id: CodexSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// Working directory.
    pub cwd: String,
    /// The CLI's thread id.
    pub thread_id: Option<String>,
    /// Model.
    pub model: Option<String>,
    /// Reasoning effort.
    pub effort: Option<String>,
    /// Approval mode.
    pub mode: CodexApprovalMode,
    /// State.
    pub state: CodexSessionState,
    /// Title (first message / thread name).
    pub title: Option<String>,
    /// Started.
    pub started_at: Timestamp,
    /// Last error.
    pub error: Option<String>,
}

/// `codex.start` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexStartParams {
    /// Project (its folder is the working directory).
    pub project_id: ProjectId,
    /// Resume (or fork) this saved thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thread_id: Option<String>,
    /// Fork `threadId` into a new thread instead of resuming it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fork: Option<bool>,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<String>,
    /// Approval mode (default `ask`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<CodexApprovalMode>,
}

/// `codex.send` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexSendParams {
    /// Session.
    pub session_id: CodexSessionId,
    /// Message.
    pub text: String,
}

/// Params naming one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexSessionParams {
    /// Session.
    pub session_id: CodexSessionId,
}

/// `codex.setOptions` params (applied from the next turn).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexSetOptionsParams {
    /// Session.
    pub session_id: CodexSessionId,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<String>,
    /// Approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<CodexApprovalMode>,
}

/// Answer to an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexApprovalDecision {
    /// Allow once.
    Allow,
    /// Allow this and similar requests for the rest of the session.
    AllowSession,
    /// Deny; Codex continues the turn.
    Deny,
    /// Deny and stop the turn.
    Abort,
}

/// `codex.respondApproval` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexRespondApprovalParams {
    /// Session.
    pub session_id: CodexSessionId,
    /// The request.
    pub request_id: String,
    /// Decision.
    pub decision: CodexApprovalDecision,
}

/// `codex.transcript` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTranscriptParams {
    /// Thread.
    pub thread_id: String,
}

/// `codex.transcript` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTranscriptResult {
    /// The thread's items as events.
    pub events: Vec<CodexEvent>,
}

/// Token usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTokenUsage {
    /// Input tokens (cached included).
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning tokens.
    pub reasoning_tokens: u64,
}

/// What an approval request is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexApprovalKind {
    /// Run a command.
    Command,
    /// Apply file changes.
    FileChange,
}

/// One item of a Codex chat session's stream. Tool names follow the Claude
/// Code panel's vocabulary (`Bash`, `Edit`, `WebSearch`, `mcp__<server>__<tool>`)
/// so the same tool cards render both engines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum CodexEvent {
    /// The thread is ready.
    ThreadStarted {
        /// Thread id.
        thread_id: String,
        /// Model.
        model: Option<String>,
        /// Effort.
        effort: Option<String>,
    },
    /// A turn started.
    TurnStarted {
        /// Turn id.
        turn_id: String,
    },
    /// A user message.
    UserMessage {
        /// Text.
        text: String,
    },
    /// Streaming answer text.
    TextDelta {
        /// Item id.
        item_id: String,
        /// Text.
        text: String,
    },
    /// A complete answer message.
    Text {
        /// Item id.
        item_id: String,
        /// Text.
        text: String,
    },
    /// Reasoning summary (never the hidden chain of thought).
    Reasoning {
        /// Summary text.
        text: String,
    },
    /// The agent's plan.
    Plan {
        /// Plan text.
        text: String,
    },
    /// A tool started.
    ToolUse {
        /// Item id.
        id: String,
        /// Tool (`Bash`, `Edit`, `WebSearch`, `mcp__s__t`, …).
        name: String,
        /// Input (`{command}`, `{file_path, changes}`, `{query}`, …).
        input: Value,
    },
    /// A tool finished.
    ToolResult {
        /// Item id.
        tool_use_id: String,
        /// Output (shortened).
        content: String,
        /// Failed.
        is_error: bool,
    },
    /// A file was changed.
    FileChanged {
        /// Path.
        path: String,
    },
    /// Codex asks before running something.
    ApprovalRequest {
        /// Request id (answer with `codex.respondApproval`).
        request_id: String,
        /// Kind.
        kind: CodexApprovalKind,
        /// Tool (`Bash` / `Edit`).
        tool_name: String,
        /// Input (`{command, cwd}` / `{file_path, changes}`).
        input: Value,
        /// Codex's reason.
        reason: Option<String>,
        /// Orkestr's guard classification.
        risk: Option<ClaudeRisk>,
        /// "Allow for this session" is offered (never for dangerous ones).
        can_remember: bool,
    },
    /// An approval was answered.
    ApprovalResolved {
        /// Request id.
        request_id: String,
        /// Decision.
        decision: CodexApprovalDecision,
        /// Decided by Orkestr's policy (forbidden command).
        by_policy: bool,
        /// Note.
        message: Option<String>,
    },
    /// Token usage of the thread.
    TokenUsage {
        /// Last turn.
        last: CodexTokenUsage,
        /// Whole thread.
        total: CodexTokenUsage,
        /// Context window.
        context_window: Option<u64>,
    },
    /// A turn finished.
    TurnCompleted {
        /// Turn id.
        turn_id: String,
        /// `completed`, `interrupted`, `failed`.
        status: String,
        /// Error message of a failed turn.
        error: Option<String>,
        /// Duration.
        duration_ms: Option<u64>,
    },
    /// Plan limits changed.
    RateLimits {
        /// Limits.
        limits: CodexLimits,
    },
    /// The model / effort changed.
    ModelChanged {
        /// Model.
        model: Option<String>,
        /// Effort.
        effort: Option<String>,
    },
    /// An informational note (compaction, warnings).
    Info {
        /// Text.
        text: String,
    },
    /// An error.
    Error {
        /// Message.
        message: String,
        /// Sign-in needed.
        auth_required: bool,
        /// The plan's usage limit was reached.
        usage_limit: bool,
        /// Codex retries by itself.
        will_retry: bool,
    },
    /// The session ended.
    Exited {
        /// Exit code of the app-server process, when it died.
        code: Option<i32>,
    },
}

/// `CodexEvent` event payload (ephemeral; topic `codex:<sessionId>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexEventPayload {
    /// Session.
    pub session_id: CodexSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// The item.
    pub event: CodexEvent,
}

/// `CodexStatus` event payload (ephemeral; topics `codex:<sessionId>`, `project:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexStatusPayload {
    /// Session.
    pub session: CodexSessionInfo,
}

/// `CodexAccount` event payload (ephemeral; global): sign-in progress / account changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexAccountPayload {
    /// Signed in.
    pub logged_in: bool,
    /// Account.
    pub account: Option<CodexAccount>,
    /// Sign-in state.
    pub login: Option<CodexLoginState>,
}

// ---------------------------------------------------------------------------
// Agent templates
// ---------------------------------------------------------------------------

/// Where a Codex agent template comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodexTemplateOrigin {
    /// Shipped with Orkestr.
    BuiltIn,
    /// `~/.codex/prompts/*.md` (the user's custom prompts).
    Prompt,
    /// `AGENTS.md` of the project.
    AgentsMd,
}

/// A Codex agent mirrored as an Orkestr template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTemplateInfo {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Origin.
    pub origin: CodexTemplateOrigin,
    /// Source file.
    pub path: Option<String>,
    /// The mirrored template.
    pub template_id: Option<orkestr_domain::AgentTemplateId>,
}

/// `codex.templates` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTemplatesParams {
    /// Also the project's `AGENTS.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// `codex.templates` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodexTemplatesResult {
    /// Templates.
    pub templates: Vec<CodexTemplateInfo>,
}

// ---------------------------------------------------------------------------
// Parallel engines
// ---------------------------------------------------------------------------

/// A subscription engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EngineId {
    /// Claude Code (Claude subscription).
    #[default]
    ClaudeCode,
    /// Codex (ChatGPT subscription).
    Codex,
}

impl EngineId {
    /// The other engine.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::ClaudeCode => Self::Codex,
            Self::Codex => Self::ClaudeCode,
        }
    }
}

/// How work is spread over the two engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EnginesMode {
    /// Orkestr does not touch routing.
    #[default]
    Off,
    /// Everything on one engine (`primary`), the other as fallback.
    Single,
    /// Roles listed in `codexRoles` on Codex, the rest on Claude Code.
    Split,
    /// Each new agent goes to the engine with fewer running agents and limit headroom.
    Balanced,
    /// Reviews / consensus ask both engines; the first acceptable answer wins.
    Race,
    /// Consensus between both engines (Phase 2 consensus).
    Consensus,
}

/// `engines.*` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnginesConfig {
    /// Mode.
    pub mode: EnginesMode,
    /// The Manager's engine.
    pub manager: EngineId,
    /// `single`: the engine for everything.
    pub primary: EngineId,
    /// `split`: roles that run on Codex.
    pub codex_roles: Vec<String>,
    /// Claude Code model.
    pub claude_model: String,
    /// Codex model (`default` = the CLI's default).
    pub codex_model: String,
    /// The other engine is the first fallback on a usage limit.
    pub cross_fallback: bool,
}

impl Default for EnginesConfig {
    fn default() -> Self {
        Self {
            mode: EnginesMode::Off,
            manager: EngineId::ClaudeCode,
            primary: EngineId::ClaudeCode,
            codex_roles: vec!["Backend".into(), "QA".into(), "DevOps".into(), "Database".into()],
            claude_model: "sonnet".into(),
            codex_model: "default".into(),
            cross_fallback: true,
        }
    }
}

/// Status of one engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineStatus {
    /// Engine.
    pub engine: EngineId,
    /// CLI installed.
    pub installed: bool,
    /// Signed in.
    pub signed_in: bool,
    /// CLI version.
    pub version: Option<String>,
    /// Plan / subscription.
    pub plan: Option<String>,
    /// Provider id.
    pub provider_id: Option<ProviderId>,
    /// Highest used share of the plan windows (percent), when known.
    pub limit_percent: Option<f64>,
    /// When the fullest window resets.
    pub limit_resets_at: Option<Timestamp>,
    /// Agents running on it now.
    pub running_agents: u32,
    /// Why it is not usable.
    pub reason: Option<String>,
}

/// `engines.status` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnginesStatusParams {
    /// Re-probe both CLIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// `engines.status` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnginesStatus {
    /// Claude Code.
    pub claude: EngineStatus,
    /// Codex.
    pub codex: EngineStatus,
    /// Configuration.
    pub config: EnginesConfig,
}

method!(
    /// `codex.status` — binary, version, account, sign-in and live sessions.
    Status, "codex.status", CodexStatusParams, CodexStatus
);
method!(
    /// `codex.install` — installs / updates the official CLI with npm into Orkestr's tools folder.
    Install, "codex.install", CodexInstallParams, CodexInstallResult
);
method!(
    /// `codex.login` — starts the ChatGPT sign-in (browser or device code) through the CLI.
    Login, "codex.login", CodexLoginParams, CodexLoginState
);
method!(
    /// `codex.loginCancel` — cancels the sign-in in progress.
    LoginCancel, "codex.loginCancel", super::Empty, CodexStatus
);
method!(
    /// `codex.logout` — signs the CLI out (`account/logout`).
    Logout, "codex.logout", super::Empty, CodexStatus
);
method!(
    /// `codex.openInTerminal` — the interactive `codex` (or `codex login --device-auth`) in an Orkestr terminal.
    OpenInTerminal, "codex.openInTerminal", CodexOpenInTerminalParams, CodexTerminalResult
);
method!(
    /// `codex.limits` — ChatGPT plan usage windows.
    Limits, "codex.limits", CodexLimitsParams, CodexLimits
);
method!(
    /// `codex.models` — models and reasoning efforts.
    Models, "codex.models", CodexModelsParams, CodexModelsResult
);
method!(
    /// `codex.threads` — saved conversations + live sessions.
    Threads, "codex.threads", CodexThreadsParams, CodexThreadsResult
);
method!(
    /// `codex.transcript` — the items of a saved thread.
    Transcript, "codex.transcript", CodexTranscriptParams, CodexTranscriptResult
);
method!(
    /// `codex.start` — starts a panel session (new, resumed or forked thread).
    Start, "codex.start", CodexStartParams, CodexSessionInfo
);
method!(
    /// `codex.send` — sends a user message (starts a turn).
    Send, "codex.send", CodexSendParams, CodexSessionInfo
);
method!(
    /// `codex.interrupt` — interrupts the running turn.
    Interrupt, "codex.interrupt", CodexSessionParams, CodexSessionInfo
);
method!(
    /// `codex.stop` — ends the session.
    Stop, "codex.stop", CodexSessionParams, CodexSessionInfo
);
method!(
    /// `codex.compact` — compacts the thread's context (`/compact`).
    Compact, "codex.compact", CodexSessionParams, CodexSessionInfo
);
method!(
    /// `codex.setOptions` — model / effort / approval mode from the next turn.
    SetOptions, "codex.setOptions", CodexSetOptionsParams, CodexSessionInfo
);
method!(
    /// `codex.respondApproval` — answers an approval request.
    RespondApproval, "codex.respondApproval", CodexRespondApprovalParams, CodexSessionInfo
);
method!(
    /// `codex.templates` — Codex agents (built in + `~/.codex/prompts`) mirrored as templates.
    Templates, "codex.templates", CodexTemplatesParams, CodexTemplatesResult
);
method!(
    /// `engines.status` — both subscriptions and the parallel-engines configuration.
    EnginesStatusMethod, "engines.status", EnginesStatusParams, EnginesStatus
);
method!(
    /// `engines.setConfig` — saves the configuration and applies it to the Manager / routing.
    EnginesSetConfig, "engines.setConfig", EnginesConfig, EnginesStatus
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_to_cli_flags() {
        assert_eq!(CodexApprovalMode::Ask.approval_policy(), "untrusted");
        assert_eq!(CodexApprovalMode::Ask.sandbox(), "workspace-write");
        assert_eq!(CodexApprovalMode::ReadOnly.sandbox(), "read-only");
        assert_eq!(CodexApprovalMode::Auto.approval_policy(), "on-request");
        for m in [CodexApprovalMode::ReadOnly, CodexApprovalMode::Ask, CodexApprovalMode::Auto] {
            assert_ne!(m.sandbox(), "danger-full-access");
        }
        assert_eq!(EngineId::Codex.other(), EngineId::ClaudeCode);
    }

    #[test]
    fn events_are_type_tagged_camel_case() {
        let e = CodexEvent::ToolResult { tool_use_id: "i".into(), content: "ok".into(), is_error: false };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "toolResult");
        assert_eq!(v["toolUseId"], "i");
        let c: EnginesConfig = serde_json::from_value(serde_json::to_value(EnginesConfig::default()).unwrap()).unwrap();
        assert_eq!(c, EnginesConfig::default());
    }
}
