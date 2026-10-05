//! Model router (spec §7.2): manual routing table / fallback chains, or
//! automatic scoring (capability fit, cost, latency, limits/circuit state),
//! always under the project privacy policy (§56). Pure and deterministic.

use std::collections::HashSet;

use orkestr_domain::{
    AiPolicy, ModelCapabilities, ModelPricing, ModelRef, ProviderHealth, ProviderKind, RoutingConfig, RoutingMode,
};
use serde::{Deserialize, Serialize};

use crate::pricing::preflight_estimate;

/// Everything the router knows about one routable model.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The model.
    pub model: ModelRef,
    /// Provider kind.
    pub kind: ProviderKind,
    /// Runs locally (privacy).
    pub is_local: bool,
    /// Provider enabled.
    pub enabled: bool,
    /// Auth usable (configured / installed / not invalid).
    pub usable: bool,
    /// Last health.
    pub health: ProviderHealth,
    /// Seconds until rate limit / quota window ends (None = usable now).
    pub blocked_for_secs: Option<u64>,
    /// Quota exhausted (hard filter).
    pub quota_exhausted: bool,
    /// Circuit breaker open (hard filter).
    pub circuit_open: bool,
    /// Model capabilities.
    pub capabilities: ModelCapabilities,
    /// Context window.
    pub context_window: u32,
    /// Estimated pricing.
    pub pricing: Option<ModelPricing>,
    /// Average latency (ms).
    pub latency_ms: Option<f64>,
    /// Historical benchmark statistics (auto-scoring input; `None` = unknown).
    pub history: Option<crate::history::ModelHistory>,
}

/// Capabilities a request needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCapabilities {
    /// Tool calling.
    pub tools: bool,
    /// Image input.
    pub vision: bool,
    /// Structured output.
    pub structured_output: bool,
    /// CLI coding agent.
    pub coding_agent: bool,
    /// Minimum context window.
    pub min_context: u32,
}

/// A routing request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteRequest {
    /// Role / task kind (`Backend`, `Classification`, `Manager`, …).
    pub purpose: Option<String>,
    /// Required capabilities.
    pub required: RequiredCapabilities,
    /// Explicit preferred model (template / manual pick).
    pub preferred: Option<ModelRef>,
    /// Caller fallback chain (template / Manager config).
    pub fallback_chain: Vec<ModelRef>,
    /// Project AI privacy policy.
    pub policy: AiPolicy,
    /// Remaining budget for this call (USD).
    pub max_cost_usd: Option<f64>,
    /// Estimated prompt tokens.
    pub est_input_tokens: u64,
    /// Max output tokens.
    pub max_output_tokens: u64,
    /// Models that already failed in this attempt chain.
    pub exclude: Vec<ModelRef>,
}

/// Why a candidate was filtered out (diagnostics / UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// Disabled provider.
    Disabled,
    /// Not authenticated / installed.
    NotUsable,
    /// Privacy policy.
    Privacy,
    /// Missing capability.
    Capability(&'static str),
    /// Context too small.
    Context,
    /// Circuit open.
    CircuitOpen,
    /// Quota exhausted.
    Quota,
    /// Rate limited longer than the configured wait.
    RateLimited,
    /// Over budget.
    Budget,
    /// Excluded by caller.
    Excluded,
}

/// Hard filters (spec §7.2 step 2).
pub fn check(c: &Candidate, req: &RouteRequest, cfg: &RoutingConfig) -> Result<(), Rejection> {
    if req.exclude.contains(&c.model) {
        return Err(Rejection::Excluded);
    }
    if !c.enabled {
        return Err(Rejection::Disabled);
    }
    if !c.usable {
        return Err(Rejection::NotUsable);
    }
    let privacy_ok = match &req.policy {
        AiPolicy::CloudAllowed => true,
        AiPolicy::LocalModelsOnly => c.is_local,
        AiPolicy::SelectedProvidersOnly { providers } => providers.contains(&c.model.provider_id),
    };
    if !privacy_ok {
        return Err(Rejection::Privacy);
    }
    let r = &req.required;
    let caps = &c.capabilities;
    if r.tools && !caps.tools {
        return Err(Rejection::Capability("tools"));
    }
    if r.vision && !caps.vision {
        return Err(Rejection::Capability("vision"));
    }
    if r.structured_output && !caps.structured_output {
        return Err(Rejection::Capability("structured output"));
    }
    if r.coding_agent && !caps.coding_agent {
        return Err(Rejection::Capability("coding agent"));
    }
    let needed = u64::from(r.min_context).max(req.est_input_tokens + req.max_output_tokens);
    if needed > 0 && u64::from(c.context_window) < needed {
        return Err(Rejection::Context);
    }
    if c.circuit_open {
        return Err(Rejection::CircuitOpen);
    }
    if c.quota_exhausted {
        return Err(Rejection::Quota);
    }
    if c.blocked_for_secs.is_some_and(|s| s > u64::from(cfg.max_rate_limit_wait_secs)) {
        return Err(Rejection::RateLimited);
    }
    if let (Some(max), Some(p)) = (req.max_cost_usd, c.pricing.as_ref()) {
        if preflight_estimate(p, req.est_input_tokens, req.max_output_tokens.max(1)) > max {
            return Err(Rejection::Budget);
        }
    }
    Ok(())
}

/// Auto-mode score (higher is better).
#[must_use]
pub fn score(c: &Candidate, req: &RouteRequest, cfg: &RoutingConfig) -> f64 {
    let cost = match (&c.pricing, c.is_local) {
        (_, true) => 1.0,
        (Some(p), false) => 1.0 / (1.0 + (p.input_per_mtok_usd + p.output_per_mtok_usd) / 2.0 / 5.0),
        (None, false) => 0.5,
    };
    let latency = c
        .latency_ms
        .or_else(|| c.history.and_then(|h| h.avg_latency_ms))
        .map_or(0.5, |ms| 1.0 / (1.0 + ms / 2000.0));
    let health = match c.health {
        ProviderHealth::Healthy => 1.0,
        ProviderHealth::Unknown => 0.6,
        ProviderHealth::Degraded => 0.4,
        ProviderHealth::RateLimited => 0.2,
        ProviderHealth::Down => 0.0,
    };
    let mut s = 0.35 * cost + 0.25 * latency + 0.4 * health;
    if c.blocked_for_secs.is_some() {
        s -= 0.5;
    }
    if cfg.prefer_local && c.is_local {
        s += 0.3;
    }
    if let Some(rule) = req.purpose.as_deref().and_then(|p| cfg.rule_for(p)) {
        if rule.model == c.model {
            s += 1.0;
        } else if rule.fallback.contains(&c.model) {
            s += 0.5;
        }
    }
    // Learned quality / latency / cost from past executions (§22).
    if let Some(h) = &c.history {
        s += h.adjustment();
    }
    if c.kind == ProviderKind::Mock {
        s -= 0.8; // the offline demo is a last resort when real providers exist
    }
    s
}

/// The ordered manual chain for a request (before filtering).
#[must_use]
pub fn manual_chain(req: &RouteRequest, cfg: &RoutingConfig) -> Vec<ModelRef> {
    let mut chain: Vec<ModelRef> = Vec::new();
    let mut seen: HashSet<ModelRef> = HashSet::new();
    let mut push = |m: &ModelRef, chain: &mut Vec<ModelRef>| {
        if seen.insert(m.clone()) {
            chain.push(m.clone());
        }
    };
    if let Some(p) = &req.preferred {
        push(p, &mut chain);
    }
    if let Some(rule) = req.purpose.as_deref().and_then(|p| cfg.rule_for(p)) {
        push(&rule.model, &mut chain);
        for m in &rule.fallback {
            push(m, &mut chain);
        }
    }
    for m in req.fallback_chain.iter().chain(cfg.default_fallback.iter()) {
        push(m, &mut chain);
    }
    chain
}

/// Routes a request: ordered list of models to try (head first). Empty when
/// nothing satisfies the hard filters.
#[must_use]
pub fn route(req: &RouteRequest, candidates: &[Candidate], cfg: &RoutingConfig) -> Vec<ModelRef> {
    let find = |m: &ModelRef| candidates.iter().find(|c| &c.model == m);
    if cfg.mode == RoutingMode::Manual {
        let chain = manual_chain(req, cfg);
        if !chain.is_empty() {
            return chain.into_iter().filter(|m| find(m).is_some_and(|c| check(c, req, cfg).is_ok())).collect();
        }
        // No explicit configuration for this purpose: fall through to scoring
        // so a request is never unroutable just because nothing was configured.
    }
    let mut scored: Vec<(f64, &Candidate)> =
        candidates.iter().filter(|c| check(c, req, cfg).is_ok()).map(|c| (score(c, req, cfg), c)).collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.model.provider_id.cmp(&b.1.model.provider_id))
            .then_with(|| a.1.model.model.cmp(&b.1.model.model))
    });
    scored.into_iter().map(|(_, c)| c.model.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{ProviderId, RoutingRule};

    fn cand(pid: ProviderId, model: &str, kind: ProviderKind, local: bool) -> Candidate {
        Candidate {
            model: ModelRef { provider_id: pid, model: model.into() },
            kind,
            is_local: local,
            enabled: true,
            usable: true,
            health: ProviderHealth::Healthy,
            blocked_for_secs: None,
            quota_exhausted: false,
            circuit_open: false,
            capabilities: ModelCapabilities {
                tools: true,
                vision: false,
                structured_output: true,
                streaming: true,
                reasoning: false,
                coding_agent: false,
            },
            context_window: 128_000,
            pricing: crate::pricing::pricing_for(model),
            latency_ms: Some(500.0),
            history: None,
        }
    }

    fn mr(c: &Candidate) -> ModelRef {
        c.model.clone()
    }

    #[test]
    fn local_only_policy_keeps_only_local() {
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let cands =
            vec![cand(a, "gpt-4o", ProviderKind::OpenAi, false), cand(b, "llama3.2", ProviderKind::Ollama, true)];
        let req = RouteRequest { policy: AiPolicy::LocalModelsOnly, ..RouteRequest::default() };
        let cfg = RoutingConfig { mode: RoutingMode::Auto, ..RoutingConfig::default() };
        assert_eq!(route(&req, &cands, &cfg), vec![mr(&cands[1])]);
    }

    #[test]
    fn selected_providers_allow_list() {
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let cands = vec![
            cand(a, "gpt-4o", ProviderKind::OpenAi, false),
            cand(b, "claude-sonnet-4-5", ProviderKind::Anthropic, false),
        ];
        let req =
            RouteRequest { policy: AiPolicy::SelectedProvidersOnly { providers: vec![b] }, ..RouteRequest::default() };
        assert_eq!(route(&req, &cands, &RoutingConfig::default()), vec![mr(&cands[1])]);
    }

    #[test]
    fn manual_chain_order_and_filters() {
        let (a, b, c) = (ProviderId::new(), ProviderId::new(), ProviderId::new());
        let mut cands = vec![
            cand(a, "claude-sonnet-4-5", ProviderKind::Anthropic, false),
            cand(b, "gpt-4o", ProviderKind::OpenAi, false),
            cand(c, "llama3.2", ProviderKind::Ollama, true),
        ];
        let cfg = RoutingConfig {
            rules: vec![RoutingRule { purpose: "Backend".into(), model: mr(&cands[1]), fallback: vec![mr(&cands[0])] }],
            default_fallback: vec![mr(&cands[2])],
            ..RoutingConfig::default()
        };
        let req = RouteRequest { purpose: Some("backend".into()), ..RouteRequest::default() };
        assert_eq!(route(&req, &cands, &cfg), vec![mr(&cands[1]), mr(&cands[0]), mr(&cands[2])]);
        // Circuit open and exclusions drop models but keep the order.
        cands[1].circuit_open = true;
        let req2 = RouteRequest { exclude: vec![mr(&cands[2])], ..req.clone() };
        assert_eq!(route(&req2, &cands, &cfg), vec![mr(&cands[0])]);
        // Missing capability.
        let req3 = RouteRequest { required: RequiredCapabilities { vision: true, ..Default::default() }, ..req };
        assert!(route(&req3, &cands, &cfg).is_empty());
    }

    #[test]
    fn rate_limit_wait_and_quota_and_budget() {
        let a = ProviderId::new();
        let mut c = cand(a, "gpt-4o", ProviderKind::OpenAi, false);
        let cfg = RoutingConfig { max_rate_limit_wait_secs: 10, ..RoutingConfig::default() };
        let req = RouteRequest::default();
        c.blocked_for_secs = Some(5);
        assert!(check(&c, &req, &cfg).is_ok());
        c.blocked_for_secs = Some(50);
        assert_eq!(check(&c, &req, &cfg), Err(Rejection::RateLimited));
        c.blocked_for_secs = None;
        c.quota_exhausted = true;
        assert_eq!(check(&c, &req, &cfg), Err(Rejection::Quota));
        c.quota_exhausted = false;
        let pricey = RouteRequest {
            max_cost_usd: Some(0.001),
            est_input_tokens: 100_000,
            max_output_tokens: 1000,
            ..RouteRequest::default()
        };
        assert_eq!(check(&c, &pricey, &cfg), Err(Rejection::Budget));
        let small = RouteRequest { est_input_tokens: 200_000, ..RouteRequest::default() };
        assert_eq!(check(&c, &small, &cfg), Err(Rejection::Context));
    }

    #[test]
    fn auto_scoring_prefers_healthy_cheap_real_providers() {
        let (a, b, m) = (ProviderId::new(), ProviderId::new(), ProviderId::new());
        let mut expensive = cand(a, "claude-opus-4-1", ProviderKind::Anthropic, false);
        let cheap = cand(b, "gpt-4o-mini", ProviderKind::OpenAi, false);
        let mock = cand(m, "mock-smart", ProviderKind::Mock, true);
        expensive.health = ProviderHealth::Degraded;
        let cfg = RoutingConfig { mode: RoutingMode::Auto, ..RoutingConfig::default() };
        let out = route(&RouteRequest::default(), &[expensive.clone(), mock.clone(), cheap.clone()], &cfg);
        assert_eq!(out, vec![mr(&cheap), mr(&expensive), mr(&mock)]);
        // A routing rule boosts its model.
        let cfg = RoutingConfig {
            mode: RoutingMode::Auto,
            rules: vec![RoutingRule { purpose: "Architect".into(), model: mr(&expensive), fallback: vec![] }],
            ..RoutingConfig::default()
        };
        let req = RouteRequest { purpose: Some("Architect".into()), ..RouteRequest::default() };
        assert_eq!(route(&req, &[cheap.clone(), expensive.clone()], &cfg)[0], mr(&expensive));
    }

    #[test]
    fn auto_scoring_uses_history_when_confident() {
        use crate::history::ModelHistory;
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let mut x = cand(a, "gpt-4o", ProviderKind::OpenAi, false);
        let mut y = cand(b, "claude-sonnet-4-5", ProviderKind::Anthropic, false);
        x.pricing = y.pricing.clone();
        let cfg = RoutingConfig { mode: RoutingMode::Auto, ..RoutingConfig::default() };
        // Same static profile → tie broken by provider id; history decides otherwise.
        let base = route(&RouteRequest::default(), &[x.clone(), y.clone()], &cfg);
        let loser = if base[0] == mr(&x) { &mut x } else { &mut y };
        loser.history = Some(ModelHistory {
            samples: 25,
            call_success_rate: 1.0,
            first_pass_rate: Some(0.95),
            avg_latency_ms: Some(400.0),
            avg_cost_usd: Some(0.01),
        });
        let favoured = mr(loser);
        assert_eq!(route(&RouteRequest::default(), &[x.clone(), y.clone()], &cfg)[0], favoured);
        // Too few samples: ignored.
        for c in [&mut x, &mut y] {
            if let Some(h) = c.history.as_mut() {
                h.samples = 2;
            }
        }
        assert_eq!(route(&RouteRequest::default(), &[x, y], &cfg), base);
    }

    #[test]
    fn manual_without_config_falls_back_to_scoring() {
        let a = ProviderId::new();
        let c = cand(a, "gpt-4o", ProviderKind::OpenAi, false);
        assert_eq!(route(&RouteRequest::default(), std::slice::from_ref(&c), &RoutingConfig::default()), vec![mr(&c)]);
    }
}
