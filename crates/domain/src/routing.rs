//! AI Manager configuration (§5) and model routing configuration (§7).
//!
//! Both are persisted as settings JSON by `orkestr-providers` and edited from
//! the "AI Manager" settings section. Every field has a serde default so old
//! stored documents keep deserializing when fields are added.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::ApprovalMode;
use crate::common::{ModelRef, Timestamp};
use crate::provider::{AuthState, ProviderHealth};

/// Reasoning effort requested from a model (mapped per provider; ignored when
/// unsupported). Hidden reasoning content is never surfaced (§1.14).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReasoningLevel {
    /// Minimal thinking, fastest.
    Low,
    /// Balanced (default).
    #[default]
    Medium,
    /// Maximum thinking budget.
    High,
}

impl ReasoningLevel {
    /// Lowercase wire form used by most provider APIs (`low|medium|high`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningLevel::Low => "low",
            ReasoningLevel::Medium => "medium",
            ReasoningLevel::High => "high",
        }
    }
}

/// How much context the Manager gathers for planning (§5 "context policy").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ContextPolicy {
    /// Only the task and directly referenced files.
    Minimal,
    /// Task, relevant files and project memory (default).
    #[default]
    Balanced,
    /// Everything that fits in the context window.
    Full,
}

fn default_parallel() -> u32 {
    4
}

/// AI Manager settings (§5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerConfig {
    /// Provider/model used for orchestration. `None` = not chosen yet.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Reasoning level.
    #[serde(default)]
    pub reasoning: ReasoningLevel,
    /// Extra system instructions appended to the Manager prompt.
    #[serde(default)]
    pub system_instructions: String,
    /// Maximum agents running in parallel.
    #[serde(default = "default_parallel")]
    pub max_parallel_agents: u32,
    /// Budget per Manager run in USD (`None` = unrestricted).
    #[serde(default)]
    pub budget_usd: Option<f64>,
    /// Wall-clock limit per Manager run in minutes (`None` = unlimited).
    #[serde(default)]
    pub time_limit_mins: Option<u32>,
    /// Context policy.
    #[serde(default)]
    pub context_policy: ContextPolicy,
    /// Ordered fallback chain used when the main model fails (§7).
    #[serde(default)]
    pub fallback_chain: Vec<ModelRef>,
    /// Default approval policy for agents spawned by the Manager.
    #[serde(default)]
    pub approval_policy: ApprovalMode,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            model: None,
            reasoning: ReasoningLevel::default(),
            system_instructions: String::new(),
            max_parallel_agents: default_parallel(),
            budget_usd: None,
            time_limit_mins: None,
            context_policy: ContextPolicy::default(),
            fallback_chain: Vec::new(),
            approval_policy: ApprovalMode::default(),
        }
    }
}

/// Manual vs automatic routing (§7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RoutingMode {
    /// Use the routing table and fallback chains exactly as configured.
    #[default]
    Manual,
    /// Score every ready model (capability fit, cost, latency, limits, privacy).
    Auto,
}

/// One routing table row: purpose → model (+ its own fallback chain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoutingRule {
    /// Agent role or task kind, e.g. `Architect`, `Backend`, `Frontend`,
    /// `Classification`, `Review`, `Manager`.
    pub purpose: String,
    /// Model to use.
    pub model: ModelRef,
    /// Fallbacks for this purpose (tried in order after `model`).
    #[serde(default)]
    pub fallback: Vec<ModelRef>,
}

fn default_rate_limit_wait() -> u32 {
    60
}

/// Model routing configuration (§7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoutingConfig {
    /// Manual or automatic.
    #[serde(default)]
    pub mode: RoutingMode,
    /// Routing table.
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
    /// Global fallback chain appended after rule-specific fallbacks.
    #[serde(default)]
    pub default_fallback: Vec<ModelRef>,
    /// Auto mode: prefer local models when they fit.
    #[serde(default)]
    pub prefer_local: bool,
    /// Max seconds to wait for a rate limit before falling back.
    #[serde(default = "default_rate_limit_wait")]
    pub max_rate_limit_wait_secs: u32,
    /// Global, user-ordered priority of AI sources (subscriptions and API
    /// keys / provider instances, ADR-0051). Empty = the default order
    /// (subscriptions, cloud API keys, local models, Mock last). Providers
    /// missing from the list are appended in that default order.
    #[serde(default)]
    pub source_priority: Vec<AiSource>,
    /// Optional per-purpose orders (`manager`, `agents`, `reviews`, `voice`,
    /// `jarvis`, `cursor`, `detective`, `productManager`). A purpose without
    /// an override uses [`Self::source_priority`] filtered by capability.
    #[serde(default)]
    pub purpose_sources: Vec<PurposeSources>,
    /// Automatic failover to the next source by priority (default on).
    #[serde(default = "default_true")]
    pub failover: bool,
}

fn default_true() -> bool {
    true
}

/// What an AI source can be used for (ADR-0051). Defaults come from the
/// provider kind; the user may switch a capability off for a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceCapabilities {
    /// Plain chat / text completion.
    pub chat: bool,
    /// Tool calling / coding agents.
    pub tools: bool,
    /// Image input.
    pub vision: bool,
    /// Screen understanding for computer use (cursor mode).
    pub computer_use: bool,
    /// Text embeddings.
    pub embeddings: bool,
}

/// One entry of the AI source priority list (ADR-0051).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AiSource {
    /// Provider instance (one subscription or one API key).
    pub provider_id: crate::ProviderId,
    /// Used for automatic routing and failover.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Model used when this source is picked by priority (`None` = the
    /// provider's default model, else the best routed one).
    #[serde(default)]
    pub model: Option<String>,
    /// Capability overrides (`None` = defaults of the provider kind).
    #[serde(default)]
    pub capabilities: Option<SourceCapabilities>,
}

/// A per-purpose source order (ADR-0051).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PurposeSources {
    /// Purpose group id (`manager`, `agents`, `reviews`, `voice`, `jarvis`,
    /// `cursor`, `detective`, `productManager`).
    pub purpose: String,
    /// Ordered providers for this purpose (others are not used for it).
    pub sources: Vec<crate::ProviderId>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            mode: RoutingMode::default(),
            rules: Vec::new(),
            default_fallback: Vec::new(),
            prefer_local: false,
            max_rate_limit_wait_secs: default_rate_limit_wait(),
            source_priority: Vec::new(),
            purpose_sources: Vec::new(),
            failover: true,
        }
    }
}

impl RoutingConfig {
    /// The rule for a purpose (case-insensitive).
    #[must_use]
    pub fn rule_for(&self, purpose: &str) -> Option<&RoutingRule> {
        self.rules.iter().find(|r| r.purpose.eq_ignore_ascii_case(purpose))
    }

    /// The per-purpose source override for a purpose group.
    #[must_use]
    pub fn purpose_override(&self, group: &str) -> Option<&PurposeSources> {
        self.purpose_sources.iter().find(|p| p.purpose.eq_ignore_ascii_case(group))
    }
}

/// Live runtime status of a provider instance (health, auth, limits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderStatus {
    /// Health from the last probe / call.
    pub health: ProviderHealth,
    /// Auth state.
    pub auth_state: AuthState,
    /// Human auth detail ("API key", "cli session (managed by Claude Code)", "not installed"). Never secret.
    pub auth_detail: Option<String>,
    /// Human health detail (redacted).
    pub health_detail: Option<String>,
    /// Latency of the last probe in ms.
    pub latency_ms: Option<u64>,
    /// Average call latency (time to first token) in ms.
    pub avg_latency_ms: Option<u64>,
    /// Detected version (CLI / Ollama).
    pub version: Option<String>,
    /// Last probe time.
    pub checked_at: Option<Timestamp>,
    /// Circuit breaker open.
    pub circuit_open: bool,
    /// Rate limited until.
    pub rate_limited_until: Option<Timestamp>,
    /// Quota exhausted until (estimated when the provider gives no reset time).
    pub quota_exhausted_until: Option<Timestamp>,
    /// Failover cooldown until (auth / network / 5xx / CLI failures, exponential; ADR-0051).
    #[serde(default)]
    #[ts(optional)]
    pub cooldown_until: Option<Timestamp>,
}

impl Default for ProviderStatus {
    fn default() -> Self {
        Self {
            health: ProviderHealth::Unknown,
            auth_state: AuthState::Unverified,
            auth_detail: None,
            health_detail: None,
            latency_ms: None,
            avg_latency_ms: None,
            version: None,
            checked_at: None,
            circuit_open: false,
            rate_limited_until: None,
            quota_exhausted_until: None,
            cooldown_until: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_config_defaults_from_empty_json() {
        let c: ManagerConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, ManagerConfig::default());
        assert_eq!(c.max_parallel_agents, 4);
    }

    #[test]
    fn routing_rule_lookup_is_case_insensitive() {
        let pid = crate::ProviderId::new();
        let cfg = RoutingConfig {
            rules: vec![RoutingRule {
                purpose: "Backend".into(),
                model: ModelRef { provider_id: pid, model: "m".into() },
                fallback: vec![],
            }],
            ..RoutingConfig::default()
        };
        assert!(cfg.rule_for("backend").is_some());
        assert!(cfg.rule_for("frontend").is_none());
        let back: RoutingConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }
}
