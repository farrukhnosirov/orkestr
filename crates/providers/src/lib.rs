//! # orkestr-providers — **SKELETON** (Phase 1 fills it in)
//!
//! Responsibility (MASTER_SPEC §4, §7; contract §7):
//!
//! * [`ProviderAdapter`] — object-safe async trait every provider implements:
//!   `id()`, `descriptor()`, `auth_state()`, `list_models()`, `health()`,
//!   `chat(ChatRequest, CancellationToken) -> stream of ChatEvent`, optional
//!   `run_coding_agent(...)` for official CLI agents (Claude Code
//!   `claude -p --output-format stream-json`, Codex `codex exec --json`) using
//!   the user's own authenticated CLI session — never reading their token files.
//! * Registry, router (quality/capability/latency/budget/limits/privacy §56)
//!   and fallback chain with explicit `ProviderFallback` events.
//! * Adapters (modules to add): `openai`, `anthropic`, `gemini`, `deepseek`,
//!   `ollama`, `openai_compatible`, `claude_code`, `codex`, `mock`
//!   ("Mock (offline demo)", deterministic).
//! * HTTP via `reqwest` (rustls). Keys only via `SecretStore` references.
//!
//! **No provider-specific logic may leak outside this crate (§60.5).**
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use futures::stream::BoxStream;
use orkestr_domain::{AuthState, Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Static description of an adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    /// Family.
    pub kind: ProviderKind,
    /// Display name.
    pub display_name: String,
    /// Runs locally.
    pub is_local: bool,
    /// Aggregate capabilities.
    pub capabilities: ModelCapabilities,
    /// Largest context window offered.
    pub max_context_tokens: u32,
    /// Supports `run_coding_agent`.
    pub coding_agent: bool,
}

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// System.
    System,
    /// User.
    User,
    /// Assistant.
    Assistant,
    /// Tool result.
    Tool,
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role.
    pub role: Role,
    /// Text content.
    pub content: String,
    /// Tool call id this message answers (role = Tool).
    pub tool_call_id: Option<String>,
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

/// A chat completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model id.
    pub model: String,
    /// Messages.
    pub messages: Vec<ChatMessage>,
    /// Tools.
    pub tools: Vec<ToolSpec>,
    /// Max output tokens.
    pub max_output_tokens: Option<u32>,
    /// Temperature.
    pub temperature: Option<f32>,
    /// Reasoning effort hint (`low|medium|high`).
    pub reasoning: Option<String>,
}

/// Streamed chat output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChatEvent {
    /// Text delta.
    TextDelta(String),
    /// Tool call request.
    ToolCall {
        /// Call id.
        id: String,
        /// Tool name.
        name: String,
        /// JSON arguments.
        arguments: serde_json::Value,
    },
    /// Usage report.
    Usage {
        /// Input tokens.
        input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
    },
    /// Finished.
    Done,
    /// Error (stream ends).
    Error(ProviderError),
}

/// Classified provider errors (contract §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProviderError {
    /// Rate limited.
    #[error("rate limited (retry after {retry_after_secs:?}s)")]
    RateLimited {
        /// Seconds until retry.
        retry_after_secs: Option<u64>,
    },
    /// Quota exhausted.
    #[error("quota exceeded")]
    QuotaExceeded,
    /// Authentication failed.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Network failure.
    #[error("network: {0}")]
    Network(String),
    /// Bad request.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Upstream server error.
    #[error("server error: {0}")]
    Server(String),
    /// Cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(String),
}

/// Stream of chat events.
pub type ChatStream = BoxStream<'static, ChatEvent>;

/// A coding-agent run request (CLI agents).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodingAgentRequest {
    /// Prompt / task description.
    pub prompt: String,
    /// Working directory (agent worktree).
    pub cwd: String,
    /// Model override.
    pub model: Option<String>,
    /// Max turns.
    pub max_turns: Option<u32>,
}

/// The provider adapter contract.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// Provider instance id.
    fn id(&self) -> ProviderId;
    /// Static descriptor.
    fn descriptor(&self) -> ProviderDescriptor;
    /// Current auth state.
    async fn auth_state(&self) -> AuthState;
    /// Available models.
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError>;
    /// Health probe.
    async fn health(&self) -> ProviderHealth;
    /// Streams a chat completion.
    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError>;
    /// Runs an official CLI coding agent; default: unsupported.
    async fn run_coding_agent(
        &self,
        _request: CodingAgentRequest,
        _cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        Err(ProviderError::Unimplemented(format!("{:?} has no coding-agent mode", self.descriptor().kind)))
    }
}
