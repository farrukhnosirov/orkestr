//! Normalized, provider-independent types (provider-adapter-spec §3).
//!
//! The agent runtime, orchestrator and cost engine only ever see these types;
//! every adapter maps its wire format onto them (§60.5).

use futures::stream::BoxStream;
pub use orkestr_domain::ReasoningLevel;
use orkestr_domain::{ModelCapabilities, ModelRef, ProviderKind};
use serde::{Deserialize, Serialize};

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// System instructions.
    System,
    /// User turn.
    User,
    /// Assistant turn (may carry tool calls).
    Assistant,
    /// Tool result (answers `tool_call_id`).
    Tool,
}

/// A tool call made by the assistant (complete, arguments parsed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallPart {
    /// Call id (provider-issued or generated).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// JSON arguments.
    pub arguments: serde_json::Value,
}

/// An inline image (vision input).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePart {
    /// MIME type, e.g. `image/png`.
    pub media_type: String,
    /// Base64 data (no `data:` prefix).
    pub data_base64: String,
}

/// One normalized chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role.
    pub role: Role,
    /// Text content.
    pub content: String,
    /// Tool call id this message answers (role = Tool).
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// Tool calls requested by the assistant (role = Assistant).
    #[serde(default)]
    pub tool_calls: Vec<ToolCallPart>,
    /// Images attached to a user message.
    #[serde(default)]
    pub images: Vec<ImagePart>,
    /// Tool result is an error (role = Tool).
    #[serde(default)]
    pub is_error: bool,
}

impl ChatMessage {
    fn new(role: Role, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), tool_call_id: None, tool_calls: vec![], images: vec![], is_error: false }
    }

    /// System message.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    /// User message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// Assistant message.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    /// Assistant message carrying tool calls.
    #[must_use]
    pub fn assistant_tool_calls(content: impl Into<String>, calls: Vec<ToolCallPart>) -> Self {
        let mut m = Self::new(Role::Assistant, content);
        m.tool_calls = calls;
        m
    }

    /// Tool result.
    #[must_use]
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        let mut m = Self::new(Role::Tool, content);
        m.tool_call_id = Some(call_id.into());
        m.is_error = is_error;
        m
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// JSON schema of the arguments.
    pub parameters: serde_json::Value,
}

/// Tool selection mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolChoice {
    /// Model decides.
    #[default]
    Auto,
    /// Never call tools.
    None,
    /// Must call some tool.
    Required,
    /// Must call this tool.
    Specific(String),
}

/// Requested response format.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum ResponseFormat {
    /// Free text.
    #[default]
    Text,
    /// Any JSON object.
    JsonObject,
    /// JSON constrained by a schema.
    JsonSchema {
        /// Schema name (`[A-Za-z0-9_-]`).
        name: String,
        /// JSON schema.
        schema: serde_json::Value,
    },
}

/// A chat completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model id (provider-native).
    pub model: String,
    /// System prompt (merged with any `Role::System` messages).
    #[serde(default)]
    pub system: Option<String>,
    /// Messages.
    pub messages: Vec<ChatMessage>,
    /// Tools.
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Tool choice.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    /// Response format.
    #[serde(default)]
    pub response_format: ResponseFormat,
    /// Max output tokens.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Reasoning effort hint (ignored when unsupported).
    #[serde(default)]
    pub reasoning: Option<ReasoningLevel>,
    /// Stop sequences.
    #[serde(default)]
    pub stop: Vec<String>,
}

impl ChatRequest {
    /// Request with only a model and messages.
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages,
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            response_format: ResponseFormat::Text,
            max_output_tokens: None,
            temperature: None,
            reasoning: None,
            stop: vec![],
        }
    }

    /// Effective system prompt: `system` plus every `Role::System` message.
    #[must_use]
    pub fn system_prompt(&self) -> Option<String> {
        let mut parts: Vec<&str> = Vec::new();
        if let Some(s) = self.system.as_deref().filter(|s| !s.trim().is_empty()) {
            parts.push(s);
        }
        for m in &self.messages {
            if m.role == Role::System && !m.content.trim().is_empty() {
                parts.push(&m.content);
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// Messages without system messages.
    pub fn conversation(&self) -> impl Iterator<Item = &ChatMessage> {
        self.messages.iter().filter(|m| m.role != Role::System)
    }

    /// Rough character count of the prompt (for chars/4 estimates).
    #[must_use]
    pub fn prompt_chars(&self) -> usize {
        self.system.as_ref().map_or(0, String::len)
            + self
                .messages
                .iter()
                .map(|m| m.content.len() + m.tool_calls.iter().map(|t| t.arguments.to_string().len()).sum::<usize>())
                .sum::<usize>()
            + self.tools.iter().map(|t| t.description.len() + t.parameters.to_string().len()).sum::<usize>()
    }
}

/// Token usage of one call (cumulative; last value wins).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens (including cached).
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Cache write tokens.
    pub cache_write_tokens: u64,
    /// Reasoning tokens (count only; content is never kept).
    pub reasoning_tokens: u64,
    /// Cost reported by the provider/CLI directly.
    pub reported_cost_usd: Option<f64>,
    /// True when counts are a heuristic estimate.
    pub estimated: bool,
}

impl Usage {
    /// chars/4 heuristic estimate.
    #[must_use]
    pub fn estimate(prompt_chars: usize, output_chars: usize) -> Self {
        Self {
            input_tokens: (prompt_chars as u64).div_ceil(4),
            output_tokens: (output_chars as u64).div_ceil(4),
            estimated: true,
            ..Self::default()
        }
    }
}

/// Why generation stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Natural end of turn.
    EndTurn,
    /// The model wants tool results.
    ToolUse,
    /// Output token limit.
    MaxTokens,
    /// A stop sequence matched.
    StopSequence,
    /// Content filtered.
    ContentFilter,
    /// Anything else.
    Other(String),
}

/// Streamed chat output. Hidden reasoning has no variant by design (§1.14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChatEvent {
    /// Text delta (UTF-8 safe).
    TextDelta(String),
    /// Complete tool call (arguments accumulated and parsed by the adapter).
    ToolCall {
        /// Call id.
        id: String,
        /// Tool name.
        name: String,
        /// JSON arguments (`{"__invalid_json": raw}` when unparseable).
        arguments: serde_json::Value,
    },
    /// Usage report (may repeat; last wins).
    Usage(Usage),
    /// The fallback executor switched models mid-request (§7).
    Switched {
        /// Failed model.
        from: ModelRef,
        /// New model.
        to: ModelRef,
        /// Reason class (`rate_limited`, `quota_exceeded`, …).
        reason: String,
    },
    /// Finished (terminal).
    Done {
        /// Stop reason.
        stop_reason: StopReason,
    },
    /// Error (terminal).
    Error(ProviderError),
}

impl ChatEvent {
    /// Whether this ends the stream.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, ChatEvent::Done { .. } | ChatEvent::Error(_))
    }

    /// Whether this delivers model output to the caller.
    #[must_use]
    pub fn is_output(&self) -> bool {
        matches!(self, ChatEvent::TextDelta(_) | ChatEvent::ToolCall { .. })
    }
}

/// Process-unique id for a tool call the provider returned without one.
/// Ids must never repeat across turns: the history is handed to fallback
/// providers (Anthropic rejects duplicate `tool_use` ids) and Gemini keys
/// thought signatures by them.
#[must_use]
pub fn unique_call_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{prefix}_{:x}_{}", chrono::Utc::now().timestamp_millis(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Parses tool arguments, wrapping invalid JSON (§3.4 rule 1).
#[must_use]
pub fn parse_tool_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({ "__invalid_json": raw }))
}

/// Classified provider errors (contract §7, spec §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProviderError {
    /// Rate limited.
    #[error("rate limited (retry after {retry_after_secs:?}s)")]
    RateLimited {
        /// Seconds until retry.
        retry_after_secs: Option<u64>,
    },
    /// Quota / credits / subscription limit exhausted.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// Authentication failed / missing credential / CLI not logged in.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Network failure (connect, DNS, TLS, idle timeout, endpoint down).
    #[error("network: {0}")]
    Network(String),
    /// Bad request (400/404/413/422, context too long, schema rejected).
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Upstream server error (5xx, overloaded, malformed stream, CLI crash).
    #[error("server error: {0}")]
    Server(String),
    /// Cancelled by our token.
    #[error("cancelled")]
    Cancelled,
    /// The provider does not support this operation.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(String),
}

impl ProviderError {
    /// snake_case class name (wire `data.class`).
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            ProviderError::RateLimited { .. } => "rate_limited",
            ProviderError::QuotaExceeded(_) => "quota_exceeded",
            ProviderError::Auth(_) => "auth",
            ProviderError::Network(_) => "network",
            ProviderError::BadRequest(_) => "bad_request",
            ProviderError::Server(_) => "server",
            ProviderError::Cancelled => "cancelled",
            ProviderError::Unsupported(_) => "unsupported",
            ProviderError::Unimplemented(_) => "unimplemented",
        }
    }

    /// Whether the fallback chain should move to the next candidate (§4 table).
    #[must_use]
    pub fn is_fallback_eligible(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited { .. }
                | ProviderError::QuotaExceeded(_)
                | ProviderError::Auth(_)
                | ProviderError::Network(_)
                | ProviderError::Server(_)
                | ProviderError::Unsupported(_)
        )
    }

    /// Whether this failure counts against the provider's circuit breaker.
    #[must_use]
    pub fn trips_circuit(&self) -> bool {
        matches!(self, ProviderError::Network(_) | ProviderError::Server(_))
    }
}

/// Stream of chat events.
pub type ChatStream = BoxStream<'static, ChatEvent>;

/// Where a provider runs (privacy routing, §56).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locality {
    /// On this machine.
    Local,
    /// A user-controlled host (remote Ollama, company gateway).
    SelfHosted,
    /// Vendor cloud.
    Cloud,
}

/// How a provider bills (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostModelKind {
    /// Per-token list prices apply.
    PerToken,
    /// Subscription: marginal $ = 0, usage tracked, limits opaque.
    Subscription,
    /// Local compute, $0.
    Local,
    /// Unknown (custom endpoints).
    Unknown,
}

/// Static description of an adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    /// Family.
    pub kind: ProviderKind,
    /// Display name ("Mock (offline demo)" for mock).
    pub display_name: String,
    /// Runs locally (privacy routing).
    pub is_local: bool,
    /// Locality.
    pub locality: Locality,
    /// Aggregate capabilities.
    pub capabilities: ModelCapabilities,
    /// Largest context window offered.
    pub max_context_tokens: u32,
    /// Supports `run_coding_agent`.
    pub coding_agent: bool,
    /// Cost model.
    pub cost_model: CostModelKind,
    /// Suggested concurrency (cloud 4, Ollama 1, CLI 2).
    pub max_concurrency: u32,
}

/// Rich auth status (mapped onto the 4-variant wire `AuthState` + detail).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthStatus {
    /// Usable. `label` is user-visible, never a token.
    Ready {
        /// e.g. "API key", "cli session (managed by Claude Code)".
        label: String,
    },
    /// Credentials present but never verified.
    Unverified {
        /// Detail.
        label: String,
    },
    /// No key / endpoint configured.
    NotConfigured,
    /// CLI missing.
    NotInstalled {
        /// Binary name.
        binary: String,
    },
    /// Credentials rejected.
    Invalid {
        /// Reason.
        reason: String,
    },
    /// Local endpoint unreachable.
    Unreachable {
        /// Reason.
        reason: String,
    },
}

impl AuthStatus {
    /// Wire enum.
    #[must_use]
    pub fn wire(&self) -> orkestr_domain::AuthState {
        use orkestr_domain::AuthState as W;
        match self {
            AuthStatus::Ready { .. } => W::Authenticated,
            AuthStatus::Unverified { .. } => W::Unverified,
            AuthStatus::NotConfigured | AuthStatus::NotInstalled { .. } => W::Unconfigured,
            AuthStatus::Invalid { .. } | AuthStatus::Unreachable { .. } => W::Invalid,
        }
    }

    /// Human detail (never secret).
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            AuthStatus::Ready { label } | AuthStatus::Unverified { label } => label.clone(),
            AuthStatus::NotConfigured => "not configured".into(),
            AuthStatus::NotInstalled { binary } => format!("not installed (`{binary}` not found on PATH)"),
            AuthStatus::Invalid { reason } => format!("invalid: {reason}"),
            AuthStatus::Unreachable { reason } => format!("unreachable: {reason}"),
        }
    }
}

/// Result of a health probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    /// Health.
    pub health: orkestr_domain::ProviderHealth,
    /// Probe latency.
    pub latency_ms: Option<u64>,
    /// Detail (redacted, user-visible).
    pub detail: Option<String>,
    /// Detected version (CLI / Ollama).
    pub version: Option<String>,
}

impl HealthReport {
    /// Healthy report.
    #[must_use]
    pub fn healthy(latency_ms: Option<u64>, detail: Option<String>) -> Self {
        Self { health: orkestr_domain::ProviderHealth::Healthy, latency_ms, detail, version: None }
    }

    /// Down report.
    #[must_use]
    pub fn down(detail: impl Into<String>) -> Self {
        Self {
            health: orkestr_domain::ProviderHealth::Down,
            latency_ms: None,
            detail: Some(detail.into()),
            version: None,
        }
    }

    /// Maps a probe error onto a report.
    #[must_use]
    pub fn from_error(err: &ProviderError, latency_ms: Option<u64>) -> Self {
        use orkestr_domain::ProviderHealth as H;
        let health = match err {
            ProviderError::RateLimited { .. } => H::RateLimited,
            ProviderError::QuotaExceeded(_) | ProviderError::Auth(_) | ProviderError::BadRequest(_) => H::Degraded,
            _ => H::Down,
        };
        Self { health, latency_ms, detail: Some(err.to_string()), version: None }
    }
}

/// Static tool policy for CLI coding agents (derived by the caller from the
/// PermissionEngine grants; the CLI enforces it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliToolPolicy {
    /// Tools the CLI may use without asking (Claude Code `--allowedTools`).
    pub allowed_tools: Vec<String>,
    /// Tools the CLI must never use (Claude Code `--disallowedTools`).
    pub disallowed_tools: Vec<String>,
    /// Allow writes inside the worktree (Claude `acceptEdits`, Codex `workspace-write`).
    pub allow_writes: bool,
}

/// A coding-agent run request (CLI agents).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodingAgentRequest {
    /// Prompt / task description (+ handoff), built by the agent runtime.
    pub prompt: String,
    /// Extra system instructions appended to the CLI's own prompt.
    #[serde(default)]
    pub system_append: Option<String>,
    /// Working directory (agent worktree).
    pub cwd: String,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Max turns.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// CLI session id to resume.
    #[serde(default)]
    pub resume_session: Option<String>,
    /// Static tool policy.
    #[serde(default)]
    pub tool_policy: CliToolPolicy,
}

/// Kind of file change reported by a CLI agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeKind {
    /// Created.
    Added,
    /// Modified.
    Modified,
    /// Deleted.
    Deleted,
}

/// Events of a CLI coding-agent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CodingAgentEvent {
    /// Assistant text, usage, done/error.
    Chat(ChatEvent),
    /// The CLI started a tool (→ `ToolCallStarted`).
    ToolStarted {
        /// Call id.
        call_id: String,
        /// Tool name.
        tool: String,
        /// Short summary (command / path), redacted by the caller.
        summary: String,
    },
    /// The CLI finished a tool (→ `ToolCallFinished`).
    ToolFinished {
        /// Call id.
        call_id: String,
        /// Success.
        ok: bool,
        /// Short summary.
        summary: String,
    },
    /// A file changed (→ `FileChanged`).
    FileChanged {
        /// Path as reported by the CLI.
        path: String,
        /// Change kind.
        change: FileChangeKind,
    },
    /// CLI session id (persist on the agent for resume).
    Session {
        /// Session id.
        session_id: String,
    },
}

/// Stream of coding-agent events.
pub type CodingAgentStream = BoxStream<'static, CodingAgentEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_merges_sources() {
        let mut r = ChatRequest::new("m", vec![ChatMessage::system("b"), ChatMessage::user("hi")]);
        r.system = Some("a".into());
        assert_eq!(r.system_prompt().as_deref(), Some("a\n\nb"));
        assert_eq!(r.conversation().count(), 1);
    }

    #[test]
    fn invalid_tool_json_is_wrapped() {
        assert_eq!(parse_tool_arguments(r#"{"a":1}"#), serde_json::json!({"a":1}));
        assert_eq!(parse_tool_arguments(""), serde_json::json!({}));
        assert_eq!(parse_tool_arguments("{oops"), serde_json::json!({"__invalid_json":"{oops"}));
    }

    #[test]
    fn error_classes_and_eligibility() {
        assert_eq!(ProviderError::RateLimited { retry_after_secs: None }.class(), "rate_limited");
        assert!(ProviderError::QuotaExceeded("x".into()).is_fallback_eligible());
        assert!(!ProviderError::BadRequest("x".into()).is_fallback_eligible());
        assert!(!ProviderError::Cancelled.is_fallback_eligible());
        assert!(ProviderError::Server("x".into()).trips_circuit());
        assert!(!ProviderError::Auth("x".into()).trips_circuit());
    }

    #[test]
    fn auth_status_maps_to_wire() {
        use orkestr_domain::AuthState;
        assert_eq!(AuthStatus::NotInstalled { binary: "claude".into() }.wire(), AuthState::Unconfigured);
        assert_eq!(AuthStatus::Ready { label: "k".into() }.wire(), AuthState::Authenticated);
        assert!(AuthStatus::NotInstalled { binary: "claude".into() }.detail().contains("not installed"));
    }
}
