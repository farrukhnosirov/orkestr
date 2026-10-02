//! The provider adapter contract (provider-adapter-spec §2, contract §7).

use async_trait::async_trait;
use orkestr_domain::{Model, ProviderId};
use tokio_util::sync::CancellationToken;

use crate::types::{
    AuthStatus, ChatRequest, ChatStream, CodingAgentRequest, CodingAgentStream, HealthReport, ProviderDescriptor,
    ProviderError,
};

/// Every provider implements this object-safe trait.
///
/// Rules: adapters are stateless with respect to conversations, never touch
/// SQLite/the event bus, resolve secrets at call time and never log them.
/// Streams end with exactly one terminal item (`Done` or `Error`).
#[async_trait]
pub trait ProviderAdapter: Send + Sync + 'static {
    /// Configured instance id.
    fn id(&self) -> ProviderId;

    /// Static descriptor (no I/O).
    fn descriptor(&self) -> ProviderDescriptor;

    /// Current auth state. May do cheap I/O (secret lookup, `--version`),
    /// never spends tokens.
    async fn auth_state(&self) -> AuthStatus;

    /// Models available to this credential.
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError>;

    /// Liveness/latency probe without spending tokens.
    async fn health(&self) -> HealthReport;

    /// Streaming chat completion. Pre-flight failures (missing key, bad config)
    /// are returned as `Err`; failures after the request started arrive as the
    /// terminal `ChatEvent::Error`.
    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError>;

    /// Runs an official CLI coding agent; default: unsupported.
    async fn run_coding_agent(
        &self,
        _request: CodingAgentRequest,
        _cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        Err(ProviderError::Unsupported(format!("{:?} has no coding-agent mode", self.descriptor().kind)))
    }
}
