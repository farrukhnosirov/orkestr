//! `providers.*` — AI Provider Hub (§4, §5, §7, §55).
//!
//! API keys travel **only** inbound (`providers.create.apiKey`,
//! `providers.setCredential.apiKey`); they are stored in the secret vault and
//! never returned, logged or emitted. `Debug` of every param type redacts them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AuthMethod, ManagerConfig, Model, ModelCapabilities, Provider, ProviderId, ProviderKind, ProviderStatus,
    RoutingConfig, Timestamp,
};

use super::{method, Empty};

/// Where a provider runs (privacy routing, §56).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProviderLocality {
    /// On this machine.
    Local,
    /// A user-controlled host.
    SelfHosted,
    /// Vendor cloud.
    Cloud,
}

/// How a provider bills (§22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProviderCostModel {
    /// Per-token list prices (estimated).
    PerToken,
    /// Subscription (CLI session): usage tracked, $ not metered.
    Subscription,
    /// Local compute ($0).
    Local,
    /// Unknown (custom endpoint).
    Unknown,
}

/// Static info about a provider kind (cards in the provider UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderKindInfo {
    /// Kind.
    pub kind: ProviderKind,
    /// Display name.
    pub display_name: String,
    /// One-line description.
    pub description: String,
    /// Auth mechanism.
    pub auth_method: AuthMethod,
    /// Needs an API key.
    pub requires_api_key: bool,
    /// API key optional (OpenAI-compatible).
    pub api_key_optional: bool,
    /// Base URL editable.
    pub supports_base_url: bool,
    /// Default base URL.
    pub default_base_url: Option<String>,
    /// Custom headers editable.
    pub supports_custom_headers: bool,
    /// Runs locally.
    pub is_local: bool,
    /// Offers a coding-agent mode (official CLI).
    pub coding_agent: bool,
    /// Official CLI binary name.
    pub binary: Option<String>,
    /// Whether that binary was found on this machine (CLI kinds).
    pub detected: Option<bool>,
    /// Note on official sign-in availability.
    pub sign_in_note: Option<String>,
    /// How to log in (CLI kinds).
    pub login_instructions: Option<String>,
    /// Default model.
    pub default_model: Option<String>,
}

/// A configured provider with live status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderView {
    /// Config (non-secret).
    pub provider: Provider,
    /// Live status.
    pub status: ProviderStatus,
    /// An API key is stored in the vault.
    pub has_credential: bool,
    /// Aggregate capabilities.
    pub capabilities: ModelCapabilities,
    /// Largest context window.
    pub max_context_tokens: u32,
    /// Locality.
    pub locality: ProviderLocality,
    /// Cost model.
    pub cost_model: ProviderCostModel,
    /// Supports coding-agent runs.
    pub coding_agent: bool,
}

/// Params of methods addressing one provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderIdParams {
    /// Provider.
    pub provider_id: ProviderId,
}

/// Result of `providers.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListProvidersResult {
    /// Providers in creation order.
    pub providers: Vec<ProviderView>,
}

/// Result of methods returning one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderViewResult {
    /// The provider.
    pub provider: ProviderView,
}

/// Params of `providers.create`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateProviderParams {
    /// Kind.
    pub kind: ProviderKind,
    /// Display name (default: kind name, made unique).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Base URL (HTTP kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_url: Option<String>,
    /// Non-secret custom headers (OpenAI-compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_model: Option<String>,
    /// Enabled (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// API key — write-only, stored in the secret vault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub api_key: Option<String>,
    /// Absolute path of the CLI binary (CLI kinds; default: PATH lookup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub binary_path: Option<String>,
}

impl std::fmt::Debug for CreateProviderParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateProviderParams")
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("headers", &self.headers.as_ref().map(|h| h.keys().collect::<Vec<_>>()))
            .field("default_model", &self.default_model)
            .field("enabled", &self.enabled)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("binary_path", &self.binary_path)
            .finish()
    }
}

/// Partial provider update. Empty strings clear `baseUrl`, `defaultModel`, `binaryPath`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderConfigPatch {
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_url: Option<String>,
    /// Headers (replace all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_model: Option<String>,
    /// Enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// CLI binary path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub binary_path: Option<String>,
}

/// Params of `providers.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateProviderParams {
    /// Provider.
    pub provider_id: ProviderId,
    /// Changes.
    pub patch: ProviderConfigPatch,
}

/// Result of `providers.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteProviderResult {
    /// Deleted provider.
    pub provider_id: ProviderId,
}

/// Params of `providers.setCredential`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetCredentialParams {
    /// Provider.
    pub provider_id: ProviderId,
    /// API key — write-only, stored in the secret vault.
    pub api_key: String,
}

impl std::fmt::Debug for SetCredentialParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetCredentialParams")
            .field("provider_id", &self.provider_id)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// Result of `providers.health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderStatusResult {
    /// Provider.
    pub provider_id: ProviderId,
    /// Fresh status.
    pub status: ProviderStatus,
}

/// Params of `providers.models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderModelsParams {
    /// Provider.
    pub provider_id: ProviderId,
    /// Bypass the catalog cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `providers.models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderModelsResult {
    /// Provider.
    pub provider_id: ProviderId,
    /// Models (pricing is an ESTIMATE from a static table).
    pub models: Vec<Model>,
    /// Served from cache.
    pub cached: bool,
    /// Fetch time.
    pub fetched_at: Timestamp,
}

/// Params of `providers.test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TestProviderParams {
    /// Provider.
    pub provider_id: ProviderId,
    /// Model (default: provider default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
}

/// Result of `providers.test` (a tiny real request; may cost a few tokens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TestProviderResult {
    /// Success.
    pub ok: bool,
    /// Model used.
    pub model: String,
    /// Time to first event in ms.
    pub latency_ms: u64,
    /// Start of the reply.
    pub sample: Option<String>,
    /// Error class (`rate_limited`, `auth`, …).
    pub error_class: Option<String>,
    /// Error message (redacted).
    pub error_message: Option<String>,
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Cost in USD (estimated unless reported by the provider).
    pub cost_usd: Option<f64>,
}

/// Result of `providers.kinds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderKindsResult {
    /// Kinds in UI order.
    pub kinds: Vec<ProviderKindInfo>,
    /// Date of the (estimated) pricing table.
    pub pricing_table_date: String,
}

/// Params/result of the Manager settings methods.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerConfigPayload {
    /// AI Manager settings (§5).
    pub config: ManagerConfig,
}

/// Params/result of the routing methods.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoutingConfigPayload {
    /// Routing settings (§7).
    pub config: RoutingConfig,
}

method!(
    /// `providers.list`.
    List, "providers.list", Empty, ListProvidersResult
);
method!(
    /// `providers.get`.
    Get, "providers.get", ProviderIdParams, ProviderViewResult
);
method!(
    /// `providers.create` — emits `ProviderStatusChanged`.
    Create, "providers.create", CreateProviderParams, ProviderViewResult
);
method!(
    /// `providers.update` — hot-swaps the adapter, emits `ProviderStatusChanged`.
    Update, "providers.update", UpdateProviderParams, ProviderViewResult
);
method!(
    /// `providers.delete` — removes config, stored key and routing references.
    Delete, "providers.delete", ProviderIdParams, DeleteProviderResult
);
method!(
    /// `providers.setCredential` — stores the API key in the vault.
    SetCredential, "providers.setCredential", SetCredentialParams, ProviderViewResult
);
method!(
    /// `providers.clearCredential` — deletes the stored API key.
    ClearCredential, "providers.clearCredential", ProviderIdParams, ProviderViewResult
);
method!(
    /// `providers.health` — active auth + health probe (no tokens spent).
    Health, "providers.health", ProviderIdParams, ProviderStatusResult
);
method!(
    /// `providers.models` — model catalog (cached 10 min).
    Models, "providers.models", ProviderModelsParams, ProviderModelsResult
);
method!(
    /// `providers.test` — tiny real request to verify the connection.
    Test, "providers.test", TestProviderParams, TestProviderResult
);
method!(
    /// `providers.kinds` — supported kinds with auth/setup info.
    Kinds, "providers.kinds", Empty, ProviderKindsResult
);
method!(
    /// `providers.getManager` — AI Manager settings.
    GetManager, "providers.getManager", Empty, ManagerConfigPayload
);
method!(
    /// `providers.setManager` — validates and stores AI Manager settings.
    SetManager, "providers.setManager", ManagerConfigPayload, ManagerConfigPayload
);
method!(
    /// `providers.getRouting` — routing table / fallback settings.
    GetRouting, "providers.getRouting", Empty, RoutingConfigPayload
);
/// One automatic switch between AI sources (ADR-0051).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FailoverLogEntry {
    /// When.
    pub at: Timestamp,
    /// Failed model.
    pub from: orkestr_domain::ModelRef,
    /// Failed source name.
    pub from_name: String,
    /// Model switched to.
    pub to: orkestr_domain::ModelRef,
    /// Its source name.
    pub to_name: String,
    /// Failure slug (`usage_limit`, `rate_limited`, `auth`, `network`, …).
    pub reason: String,
}

/// `providers.sources` result: the effective AI source priority and the
/// recent failover log (ADR-0051). Edit the order via `providers.setRouting`
/// (`sourcePriority`, `purposeSources`, `failover`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AiSourcesResult {
    /// Every provider in try order, capabilities resolved.
    pub sources: Vec<orkestr_domain::AiSource>,
    /// Purpose groups that accept an override.
    pub purposes: Vec<String>,
    /// Recent switches, newest first.
    pub log: Vec<FailoverLogEntry>,
}

method!(
    /// `providers.sources` — effective AI source priority + failover log.
    Sources, "providers.sources", Empty, AiSourcesResult
);
method!(
    /// `providers.setRouting` — validates and stores routing settings.
    SetRouting, "providers.setRouting", RoutingConfigPayload, RoutingConfigPayload
);

/// Method names of this area (all implemented).
pub const PLANNED: &[&str] = &[
    "providers.list",
    "providers.get",
    "providers.create",
    "providers.update",
    "providers.delete",
    "providers.setCredential",
    "providers.clearCredential",
    "providers.health",
    "providers.models",
    "providers.test",
    "providers.kinds",
    "providers.getManager",
    "providers.setManager",
    "providers.getRouting",
    "providers.setRouting",
    "providers.sources",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_names_are_valid() {
        for name in PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }

    #[test]
    fn debug_redacts_keys() {
        let p: CreateProviderParams = serde_json::from_str(r#"{"kind":"OpenAi","apiKey":"sk-secret-1"}"#).unwrap();
        assert!(!format!("{p:?}").contains("sk-secret-1"));
        let s = SetCredentialParams { provider_id: ProviderId::new(), api_key: "sk-secret-2".into() };
        assert!(!format!("{s:?}").contains("sk-secret-2"));
    }
}
