//! AI providers, credentials and models (§4, §7).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Timestamp;
use crate::ids::{CredentialRefId, ProviderId, SecretRefId};

/// Adapter family of a provider instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProviderKind {
    /// OpenAI API.
    OpenAi,
    /// Anthropic API.
    Anthropic,
    /// Google Gemini API.
    Gemini,
    /// DeepSeek API.
    DeepSeek,
    /// Local Ollama.
    Ollama,
    /// Any OpenAI-compatible HTTP endpoint.
    OpenAiCompatible,
    /// Claude Code official CLI (user's authenticated session).
    ClaudeCode,
    /// OpenAI Codex official CLI (user's authenticated session).
    Codex,
    /// Deterministic offline demo provider ("Mock (offline demo)").
    Mock,
    /// Custom HTTP provider / plugin.
    Custom,
}

impl ProviderKind {
    /// Whether the kind runs locally by default (no data leaves the machine).
    #[must_use]
    pub fn is_local_by_default(self) -> bool {
        matches!(self, ProviderKind::Ollama | ProviderKind::Mock)
    }
}

/// Authentication mechanism (§4). Only officially supported mechanisms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AuthMethod {
    /// No auth (local endpoints, mock).
    None,
    /// API key stored in the secret vault.
    ApiKey,
    /// Official OAuth / account sign-in.
    OAuth,
    /// Official CLI already authenticated by the user (Claude Code, Codex).
    CliSession,
    /// Cloud credentials (Bedrock, Vertex, Azure — future).
    CloudCredentials,
    /// Local endpoint without credentials.
    LocalEndpoint,
    /// Custom headers/tokens stored as secrets.
    CustomHeaders,
}

/// Current authentication state as reported by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AuthState {
    /// Not configured.
    Unconfigured,
    /// Credentials present, not yet validated.
    Unverified,
    /// Validated and working.
    Authenticated,
    /// Credentials rejected / expired.
    Invalid,
}

/// Live health of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProviderHealth {
    /// Unknown (never checked).
    Unknown,
    /// Healthy.
    Healthy,
    /// Slow or partially failing.
    Degraded,
    /// Rate-limited right now.
    RateLimited,
    /// Unreachable / erroring.
    Down,
}

/// A configured provider instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Provider {
    /// Id.
    pub id: ProviderId,
    /// Adapter family.
    pub kind: ProviderKind,
    /// Display name, e.g. "Anthropic (work)".
    pub name: String,
    /// Disabled providers are skipped by the router.
    pub enabled: bool,
    /// Base URL for HTTP providers.
    pub base_url: Option<String>,
    /// Auth mechanism.
    pub auth_method: AuthMethod,
    /// Credential reference (never the secret).
    pub credential_ref_id: Option<CredentialRefId>,
    /// Secret reference holding this provider's API key in the vault (never the
    /// secret itself). Resolved at call time by `orkestr-providers`.
    #[serde(default)]
    pub secret_ref_id: Option<SecretRefId>,
    /// Whether the endpoint is local (privacy policy §56).
    pub is_local: bool,
    /// Default model id.
    pub default_model: Option<String>,
    /// Adapter-specific non-secret settings.
    pub settings: serde_json::Value,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
}

/// Pointer from a provider to a secret in the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderCredentialReference {
    /// Id.
    pub id: CredentialRefId,
    /// Provider.
    pub provider_id: ProviderId,
    /// Mechanism.
    pub auth_method: AuthMethod,
    /// Secret reference holding the key/token, if any (CLI sessions have none).
    pub secret_ref_id: Option<SecretRefId>,
    /// Label ("personal key").
    pub label: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last successful validation.
    pub last_validated_at: Option<Timestamp>,
}

/// What a model can do (§4 adapter contract).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelCapabilities {
    /// Native tool/function calling.
    pub tools: bool,
    /// Image input.
    pub vision: bool,
    /// JSON schema constrained output.
    pub structured_output: bool,
    /// Token streaming.
    pub streaming: bool,
    /// Extended reasoning controls.
    pub reasoning: bool,
    /// Full coding-agent loop (CLI agents).
    pub coding_agent: bool,
}

/// Price per million tokens in USD.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelPricing {
    /// Input tokens.
    pub input_per_mtok_usd: f64,
    /// Output tokens.
    pub output_per_mtok_usd: f64,
    /// Cached input tokens, if discounted.
    pub cached_input_per_mtok_usd: Option<f64>,
}

impl ModelPricing {
    /// Cost for the given token counts.
    #[must_use]
    pub fn cost(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        (input_tokens as f64 * self.input_per_mtok_usd + output_tokens as f64 * self.output_per_mtok_usd) / 1_000_000.0
    }
}

/// A model offered by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Model {
    /// Provider instance.
    pub provider_id: ProviderId,
    /// Provider-specific id.
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Context window in tokens.
    pub context_window: u32,
    /// Max output tokens.
    pub max_output_tokens: Option<u32>,
    /// Capabilities.
    pub capabilities: ModelCapabilities,
    /// Pricing (None = unknown or free/local).
    pub pricing: Option<ModelPricing>,
    /// Runs locally.
    pub is_local: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_cost() {
        let p = ModelPricing { input_per_mtok_usd: 3.0, output_per_mtok_usd: 15.0, cached_input_per_mtok_usd: None };
        let c = p.cost(1_000_000, 100_000);
        assert!((c - 4.5).abs() < 1e-9);
    }

    #[test]
    fn local_kinds() {
        assert!(ProviderKind::Ollama.is_local_by_default());
        assert!(!ProviderKind::Anthropic.is_local_by_default());
    }
}
