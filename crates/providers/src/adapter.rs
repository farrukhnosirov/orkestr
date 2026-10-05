//! The provider adapter contract (provider-adapter-spec §2, contract §7).

use async_trait::async_trait;
use orkestr_domain::{Model, ProviderId};
use tokio_util::sync::CancellationToken;

use crate::audio::{SpeechAudio, SpeechRequest, Transcription, TranscriptionRequest};
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

    /// Speech-to-text (§33); default: unsupported. Callers enforce consent.
    async fn transcribe(&self, _request: TranscriptionRequest) -> Result<Transcription, ProviderError> {
        Err(ProviderError::Unsupported(format!("{:?} has no speech-to-text", self.descriptor().kind)))
    }

    /// Text-to-speech (§33); default: unsupported. Callers enforce consent.
    async fn synthesize_speech(&self, _request: SpeechRequest) -> Result<SpeechAudio, ProviderError> {
        Err(ProviderError::Unsupported(format!("{:?} has no text-to-speech", self.descriptor().kind)))
    }
}
