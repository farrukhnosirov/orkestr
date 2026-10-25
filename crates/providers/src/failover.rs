//! System-wide AI source failover (ADR-0051).
//!
//! * [`classify`] вЂ” one table that turns any [`ProviderError`] (HTTP adapters,
//!   official CLIs, app-server pools) into a [`FailureKind`] and decides
//!   whether the request moves on to the next AI source.
//! * [`effective_sources`] / [`prioritize`] вЂ” the global, user-ordered list
//!   of AI sources (each subscription and each API key / provider instance)
//!   with per-source capabilities and per-purpose overrides, applied to every
//!   routed chain by [`crate::ProviderRegistry::route`].
//! * [`ProviderChain`] вЂ” the generic "try sources by priority" runner for
//!   calls that are not a plain streamed chat (vision backends, custom
//!   adapter calls). Streamed chats use [`crate::chat_with_fallback`], which
//!   applies the same classification.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use orkestr_domain::{AiSource, ModelRef, ProviderId, ProviderKind, RoutingConfig, SourceCapabilities, Timestamp};
use serde::{Deserialize, Serialize};

use crate::adapter::ProviderAdapter;
use crate::fallback::{AdapterSource, AttemptOutcome, ProviderEvent};
use crate::router::{check, Candidate, RouteRequest};
use crate::types::ProviderError;

/// Why an AI source failed, as far as failover is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// API key / login invalid or expired.
    AuthInvalid,
    /// Subscription usage limit (Claude Code / ChatGPT plan window).
    UsageLimit,
    /// 429 / throttled.
    RateLimit,
    /// API credits / quota exhausted.
    QuotaExhausted,
    /// Connection / DNS / TLS failure.
    Network,
    /// The source did not answer in time.
    Timeout,
    /// 5xx / overloaded / malformed stream.
    Server,
    /// The requested model does not exist or is not available to this key.
    ModelUnavailable,
    /// Official CLI not installed.
    CliMissing,
    /// Official CLI crashed / exited unexpectedly.
    CliCrashed,
    /// The source cannot do this request (capability, disabled, recovering).
    Unsupported,
    /// The request itself is invalid (would fail everywhere).
    BadRequest,
    /// Refused by a content policy (must not be laundered through another source).
    ContentPolicy,
    /// Cancelled by the user.
    Cancelled,
}

impl FailureKind {
    /// Whether the request moves on to the next source.
    #[must_use]
    pub fn fails_over(self) -> bool {
        !matches!(self, FailureKind::BadRequest | FailureKind::ContentPolicy | FailureKind::Cancelled)
    }

    /// Whether the source gets an exponential cooldown (rate limits and quota
    /// have their own windows).
    #[must_use]
    pub fn cools_down(self) -> bool {
        matches!(
            self,
            FailureKind::AuthInvalid
                | FailureKind::Network
                | FailureKind::Timeout
                | FailureKind::Server
                | FailureKind::ModelUnavailable
                | FailureKind::CliMissing
                | FailureKind::CliCrashed
        )
    }

    /// Wire slug (`ProviderFallback.reason`, i18n key `fallbackReason.<slug>`).
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            FailureKind::AuthInvalid => "auth",
            FailureKind::UsageLimit => "usage_limit",
            FailureKind::RateLimit => "rate_limited",
            FailureKind::QuotaExhausted => "quota_exceeded",
            FailureKind::Network => "network",
            FailureKind::Timeout => "timeout",
            FailureKind::Server => "server",
            FailureKind::ModelUnavailable => "model_unavailable",
            FailureKind::CliMissing => "cli_missing",
            FailureKind::CliCrashed => "cli_crashed",
            FailureKind::Unsupported => "unsupported",
            FailureKind::BadRequest => "bad_request",
            FailureKind::ContentPolicy => "content_policy",
            FailureKind::Cancelled => "cancelled",
        }
    }
}

fn has(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

const CONTENT_POLICY: &[&str] =
    &["content policy", "content_policy", "safety system", "flagged", "content filter", "content_filter"];

/// Classifies a provider failure (the single failover table, ADR-0051).
#[must_use]
pub fn classify(error: &ProviderError) -> FailureKind {
    match error {
        ProviderError::Cancelled => FailureKind::Cancelled,
        ProviderError::RateLimited { .. } => FailureKind::RateLimit,
        ProviderError::QuotaExceeded(m) => {
            let m = m.to_lowercase();
            if has(&m, &["usage limit", "subscription", "plan limit", "5-hour", "weekly limit", "hit your limit"]) {
                FailureKind::UsageLimit
            } else {
                FailureKind::QuotaExhausted
            }
        }
        ProviderError::Auth(m) => {
            let m = m.to_lowercase();
            if has(&m, &["not installed", "not found вЂ” install", "not found - install", "install the official cli"])
            {
                FailureKind::CliMissing
            } else {
                FailureKind::AuthInvalid
            }
        }
        ProviderError::Network(m) => {
            if has(&m.to_lowercase(), &["timeout", "timed out", "deadline"]) {
                FailureKind::Timeout
            } else {
                FailureKind::Network
            }
        }
        ProviderError::Server(m) => {
            let m = m.to_lowercase();
            if has(&m, CONTENT_POLICY) {
                FailureKind::ContentPolicy
            } else if has(&m, &["failed to start", "failed to run", "no such file", "cannot find the file"]) {
                FailureKind::CliMissing
            } else if has(&m, &["exited", "exit code", "exit status", "crash", "no output for", "stdout unavailable"]) {
                if m.contains("no output for") {
                    FailureKind::Timeout
                } else {
                    FailureKind::CliCrashed
                }
            } else {
                FailureKind::Server
            }
        }
        ProviderError::BadRequest(m) => {
            let m = m.to_lowercase();
            if has(&m, CONTENT_POLICY) {
                FailureKind::ContentPolicy
            } else if has(
                &m,
                &[
                    "model_not_found",
                    "model not found",
                    "unknown model",
                    "does not exist",
                    "no such model",
                    "not have access to model",
                    "model is not available",
                    "model not available",
                    "unsupported model",
                    "invalid model",
                ],
            ) {
                FailureKind::ModelUnavailable
            } else {
                FailureKind::BadRequest
            }
        }
        ProviderError::Unsupported(_) | ProviderError::Unimplemented(_) => FailureKind::Unsupported,
    }
}

/// Time until a subscription usage limit resets when the message carries
/// it (Claude Code: `Claude AI usage limit reached|<unix seconds>`).
#[must_use]
pub fn limit_reset_in(message: &str) -> Option<std::time::Duration> {
    let tail = message.rsplit('|').next()?.trim();
    let at: i64 = tail.parse().ok()?;
    let now = chrono::Utc::now().timestamp();
    (at > now && at - now <= 8 * 24 * 3600).then(|| std::time::Duration::from_secs((at - now).unsigned_abs()))
}

/// Purpose groups that may carry their own source order (UI expanders).
pub const PURPOSE_GROUPS: &[&str] =
    &["manager", "agents", "reviews", "voice", "jarvis", "cursor", "detective", "productManager"];

/// Maps a routing purpose (`Manager`, `Backend`, `Review`, `VoicePhrase`, вЂ¦)
/// onto its purpose group. Unknown purposes are agent roles.
#[must_use]
pub fn purpose_group(purpose: Option<&str>) -> &'static str {
    let Some(p) = purpose else { return "manager" };
    let p: String = p.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    match p.as_str() {
        "manager" | "merge" | "summarize" | "classification" | "plan" | "planner" => "manager",
        "review" | "reviews" | "reviewer" | "consensus" | "adversarial" => "reviews",
        "voice" | "voicephrase" | "phrase" => "voice",
        "jarvis" | "jarvischat" | "assistant" | "conversation" => "jarvis",
        "cursor" | "cursormode" | "computer" | "computeruse" | "vision" => "cursor",
        "detective" | "bugdetective" => "detective",
        "productmanager" | "pm" => "productManager",
        _ => "agents",
    }
}

/// Default capabilities of a provider kind.
#[must_use]
pub fn default_capabilities(kind: ProviderKind) -> SourceCapabilities {
    let c = |chat, tools, vision, computer_use, embeddings| SourceCapabilities {
        chat,
        tools,
        vision,
        computer_use,
        embeddings,
    };
    match kind {
        ProviderKind::Anthropic => c(true, true, true, true, false),
        ProviderKind::OpenAi | ProviderKind::Gemini | ProviderKind::Mock => c(true, true, true, true, true),
        ProviderKind::DeepSeek => c(true, true, false, false, false),
        ProviderKind::Ollama | ProviderKind::OpenAiCompatible | ProviderKind::Custom => {
            c(true, true, true, false, true)
        }
        ProviderKind::ClaudeCode | ProviderKind::Codex => c(true, true, true, false, false),
    }
}

/// Default priority rank of a kind: subscriptions, cloud API keys, local
/// models, the offline Mock last.
#[must_use]
pub fn default_rank(kind: ProviderKind) -> u8 {
    match kind {
        ProviderKind::ClaudeCode => 0,
        ProviderKind::Codex => 1,
        ProviderKind::Anthropic => 2,
        ProviderKind::OpenAi => 3,
        ProviderKind::Gemini => 4,
        ProviderKind::DeepSeek => 5,
        ProviderKind::OpenAiCompatible => 6,
        ProviderKind::Custom => 7,
        ProviderKind::Ollama => 8,
        ProviderKind::Mock => 9,
    }
}

/// What prioritisation needs to know about a provider instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    /// Instance.
    pub provider_id: ProviderId,
    /// Kind.
    pub kind: ProviderKind,
    /// Configured default model.
    pub default_model: Option<String>,
    /// In an exponential cooldown right now (demoted to the end).
    pub cooling: bool,
}

/// The full, ordered source list: the stored order (known providers only),
/// then every other provider in the default order. Capabilities resolved.
#[must_use]
pub fn effective_sources(cfg: &RoutingConfig, providers: &[SourceInfo]) -> Vec<AiSource> {
    let mut out: Vec<AiSource> = Vec::new();
    let mut seen: HashSet<ProviderId> = HashSet::new();
    for s in &cfg.source_priority {
        if let Some(p) = providers.iter().find(|p| p.provider_id == s.provider_id) {
            if seen.insert(s.provider_id) {
                let mut s = s.clone();
                s.capabilities.get_or_insert_with(|| default_capabilities(p.kind));
                out.push(s);
            }
        }
    }
    let mut rest: Vec<(usize, &SourceInfo)> =
        providers.iter().enumerate().filter(|(_, p)| !seen.contains(&p.provider_id)).collect();
    rest.sort_by_key(|(i, p)| (default_rank(p.kind), *i));
    for (_, p) in rest {
        out.push(AiSource {
            provider_id: p.provider_id,
            enabled: true,
            model: None,
            capabilities: Some(default_capabilities(p.kind)),
        });
    }
    out
}

/// Whether a source's capabilities cover a request.
#[must_use]
pub fn covers(caps: &SourceCapabilities, req: &crate::router::RequiredCapabilities) -> bool {
    let needs_chat = !req.tools && !req.coding_agent && !req.embeddings;
    (!needs_chat || caps.chat)
        && (!req.tools || caps.tools)
        && (!req.coding_agent || caps.tools)
        && (!req.vision || caps.vision)
        && (!req.computer_use || caps.computer_use)
        && (!req.embeddings || caps.embeddings)
}

/// Applies the AI source priority to a routed chain (pure; ADR-0051):
///
/// 1. models the caller / routing table named explicitly (`preferred`, the
///    purpose rule, caller and default fallbacks) stay at the head;
/// 2. then one model of every enabled source in priority order (the
///    purpose override's order when set), filtered by capability;
/// 3. then the remaining routed models;
/// 4. sources in a cooldown are demoted to the end (tried last, never lost).
///
/// With `cfg.failover == false` the routed chain is returned unchanged.
#[must_use]
pub fn prioritize(
    routed: Vec<ModelRef>,
    req: &RouteRequest,
    candidates: &[Candidate],
    cfg: &RoutingConfig,
    providers: &[SourceInfo],
) -> Vec<ModelRef> {
    if !cfg.failover {
        return routed;
    }
    let sources = effective_sources(cfg, providers);
    let group = purpose_group(req.purpose.as_deref());
    let order: Vec<&AiSource> = match cfg.purpose_override(group) {
        Some(o) => o.sources.iter().filter_map(|id| sources.iter().find(|s| s.provider_id == *id)).collect(),
        None => sources.iter().filter(|s| s.enabled).collect(),
    };
    let allowed = |pid: &ProviderId| order.iter().any(|s| s.provider_id == *pid);
    let caps_ok = |pid: &ProviderId| {
        sources
            .iter()
            .find(|s| s.provider_id == *pid)
            .is_none_or(|s| s.capabilities.as_ref().is_none_or(|c| covers(c, &req.required)))
    };
    let explicit: HashSet<ModelRef> = crate::router::manual_chain(req, cfg).into_iter().collect();
    let mut out: Vec<ModelRef> = Vec::new();
    let push = |m: ModelRef, out: &mut Vec<ModelRef>| {
        if !out.contains(&m) {
            out.push(m);
        }
    };
    for m in routed.iter().filter(|m| explicit.contains(*m)) {
        let pinned = req.preferred.as_ref() == Some(m);
        if caps_ok(&m.provider_id) && (pinned || allowed(&m.provider_id)) {
            push((*m).clone(), &mut out);
        }
    }
    let passes = |c: &&Candidate| check(c, req, cfg).is_ok();
    for s in &order {
        if !caps_ok(&s.provider_id) || out.iter().any(|m| m.provider_id == s.provider_id) {
            continue;
        }
        let info = providers.iter().find(|p| p.provider_id == s.provider_id);
        let wanted = s.model.clone().or_else(|| info.and_then(|i| i.default_model.clone()));
        let pick = wanted
            .and_then(|w| {
                candidates
                    .iter()
                    .filter(passes)
                    .find(|c| c.model.provider_id == s.provider_id && c.model.model == w)
                    .map(|c| c.model.clone())
            })
            .or_else(|| routed.iter().find(|m| m.provider_id == s.provider_id).cloned())
            .or_else(|| {
                candidates.iter().filter(passes).find(|c| c.model.provider_id == s.provider_id).map(|c| c.model.clone())
            });
        if let Some(m) = pick {
            push(m, &mut out);
        }
    }
    for m in routed {
        if caps_ok(&m.provider_id) && allowed(&m.provider_id) {
            push(m, &mut out);
        }
    }
    let cooling = |m: &ModelRef| providers.iter().any(|p| p.provider_id == m.provider_id && p.cooling);
    let (mut ready, cold): (Vec<ModelRef>, Vec<ModelRef>) = out.into_iter().partition(|m| !cooling(m));
    ready.extend(cold);
    ready
}

/// One recorded automatic switch (failover log, Settings в†’ AI sources).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverRecord {
    /// When.
    pub at: Timestamp,
    /// Failed model.
    pub from: ModelRef,
    /// Its provider's display name.
    pub from_name: String,
    /// Next model.
    pub to: ModelRef,
    /// Its provider's display name.
    pub to_name: String,
    /// Failure slug ([`FailureKind::slug`] or the executor's error class).
    pub reason: String,
}

/// One attempt handed to a [`ProviderChain::run`] closure.
#[derive(Clone)]
pub struct SourceAttempt {
    /// The model to use (its provider is the AI source).
    pub model: ModelRef,
    /// The source's adapter.
    pub adapter: Arc<dyn ProviderAdapter>,
    /// 1-based attempt number.
    pub attempt: u32,
}

impl std::fmt::Debug for SourceAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceAttempt").field("model", &self.model).field("attempt", &self.attempt).finish()
    }
}

/// An automatic switch made by a [`ProviderChain`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChainSwitch {
    /// Failed model.
    pub from: ModelRef,
    /// Next model.
    pub to: ModelRef,
    /// Why.
    pub reason: FailureKind,
}

/// Result of a [`ProviderChain::run`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChainOutcome<T> {
    /// The closure's value.
    pub value: T,
    /// The model that answered.
    pub model: ModelRef,
    /// Switches made before it answered.
    pub switches: Vec<ChainSwitch>,
}

/// Tries AI sources by priority until one succeeds (ADR-0051). Every
/// attempt is reported to the registry (cooldown / limits / health), every
/// switch emits `ProviderFallback`. Non-failover errors (bad request,
/// content policy, cancel) end the run at once.
#[derive(Clone)]
pub struct ProviderChain {
    source: Arc<dyn AdapterSource>,
    chain: Vec<ModelRef>,
}

impl std::fmt::Debug for ProviderChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderChain").field("chain", &self.chain).finish()
    }
}

impl ProviderChain {
    /// A chain over `chain` (already routed + prioritised).
    #[must_use]
    pub fn new(source: Arc<dyn AdapterSource>, chain: Vec<ModelRef>) -> Self {
        Self { source, chain }
    }

    /// The models in try order.
    #[must_use]
    pub fn models(&self) -> &[ModelRef] {
        &self.chain
    }

    /// Runs `f` on each source in order until one returns `Ok`.
    ///
    /// # Errors
    /// The first non-failover error, else the last error (or `Unsupported`
    /// when no source was usable).
    pub async fn run<T, F, Fut>(&self, mut f: F) -> Result<ChainOutcome<T>, ProviderError>
    where
        F: FnMut(SourceAttempt) -> Fut,
        Fut: Future<Output = Result<T, ProviderError>>,
    {
        let mut switches = Vec::new();
        let mut failed: Option<(ModelRef, FailureKind)> = None;
        let mut last: Option<ProviderError> = None;
        let mut attempt = 0u32;
        for model in &self.chain {
            if !self.source.admit(model) {
                continue;
            }
            let Some(adapter) = self.source.adapter(&model.provider_id) else {
                continue;
            };
            if let Some((from, reason)) = failed.take() {
                self.source.emit(ProviderEvent::Fallback {
                    from: from.clone(),
                    to: model.clone(),
                    reason: reason.slug().to_owned(),
                    from_name: None,
                    to_name: None,
                });
                switches.push(ChainSwitch { from, to: model.clone(), reason });
            }
            attempt += 1;
            let started = Instant::now();
            match f(SourceAttempt { model: model.clone(), adapter, attempt }).await {
                Ok(value) => {
                    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    self.source.observe(model, &AttemptOutcome::Success { latency_ms });
                    return Ok(ChainOutcome { value, model: model.clone(), switches });
                }
                Err(error) => {
                    let kind = classify(&error);
                    if kind != FailureKind::Cancelled {
                        self.source.observe(model, &AttemptOutcome::Failure(error.clone()));
                    }
                    tracing::warn!(provider = %model.provider_id, model = %model.model, reason = kind.slug(), "AI source failed");
                    if !kind.fails_over() {
                        return Err(error);
                    }
                    failed = Some((model.clone(), kind));
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            ProviderError::Unsupported(
                "no AI source is available for this request (check Settings в†’ AI source priority)".into(),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::RequiredCapabilities;
    use orkestr_domain::{ModelCapabilities, ProviderHealth, PurposeSources};

    #[test]
    fn classification_table() {
        use FailureKind as K;
        use ProviderError as E;
        let rows: Vec<(E, K, bool)> = vec![
            (E::Auth("401 invalid x-api-key".into()), K::AuthInvalid, true),
            (E::Auth("Claude Code CLI (`claude`) is not installed".into()), K::CliMissing, true),
            (E::Auth("`codex` not found вЂ” install the official CLI".into()), K::CliMissing, true),
            (E::QuotaExceeded("Claude AI usage limit reached|1760000000".into()), K::UsageLimit, true),
            (E::QuotaExceeded("You've hit your usage limit".into()), K::UsageLimit, true),
            (E::QuotaExceeded("insufficient_quota".into()), K::QuotaExhausted, true),
            (E::RateLimited { retry_after_secs: Some(3) }, K::RateLimit, true),
            (E::Network("connection refused".into()), K::Network, true),
            (E::Network("request timed out".into()), K::Timeout, true),
            (E::Server("503 overloaded".into()), K::Server, true),
            (E::Server("claude exited with code 1".into()), K::CliCrashed, true),
            (E::Server("CLI produced no output for 600s".into()), K::Timeout, true),
            (E::Server("failed to start `claude`: access denied".into()), K::CliMissing, true),
            (E::BadRequest("model_not_found: gpt-9".into()), K::ModelUnavailable, true),
            (E::BadRequest("The model `x` does not exist".into()), K::ModelUnavailable, true),
            (E::Unsupported("provider is not available".into()), K::Unsupported, true),
            (E::BadRequest("messages: field required".into()), K::BadRequest, false),
            (E::BadRequest("Your request was flagged by our content policy".into()), K::ContentPolicy, false),
            (E::Server("blocked by the safety system".into()), K::ContentPolicy, false),
            (E::Cancelled, K::Cancelled, false),
        ];
        for (e, k, over) in rows {
            assert_eq!(classify(&e), k, "{e}");
            assert_eq!(k.fails_over(), over, "{e}");
        }
        assert!(K::AuthInvalid.cools_down() && !K::RateLimit.cools_down() && !K::UsageLimit.cools_down());
    }

    #[test]
    fn purpose_groups() {
        assert_eq!(purpose_group(Some("Manager")), "manager");
        assert_eq!(purpose_group(Some("Backend")), "agents");
        assert_eq!(purpose_group(Some("Product Manager")), "productManager");
        assert_eq!(purpose_group(Some("VoicePhrase")), "voice");
        assert_eq!(purpose_group(Some("Review")), "reviews");
        assert_eq!(purpose_group(Some("Cursor")), "cursor");
        assert_eq!(purpose_group(Some("Jarvis")), "jarvis");
        assert_eq!(purpose_group(Some("Detective")), "detective");
        assert_eq!(purpose_group(None), "manager");
    }

    fn info(kind: ProviderKind, model: &str) -> SourceInfo {
        SourceInfo { provider_id: ProviderId::new(), kind, default_model: Some(model.into()), cooling: false }
    }

    fn cand(s: &SourceInfo, model: &str, vision: bool) -> Candidate {
        Candidate {
            model: ModelRef { provider_id: s.provider_id, model: model.into() },
            kind: s.kind,
            is_local: false,
            enabled: true,
            usable: true,
            health: ProviderHealth::Healthy,
            blocked_for_secs: None,
            quota_exhausted: false,
            circuit_open: false,
            capabilities: ModelCapabilities {
                tools: true,
                vision,
                structured_output: true,
                streaming: true,
                reasoning: false,
                coding_agent: false,
            },
            context_window: 200_000,
            pricing: None,
            latency_ms: None,
            history: None,
        }
    }

    fn mr(s: &SourceInfo, m: &str) -> ModelRef {
        ModelRef { provider_id: s.provider_id, model: m.into() }
    }

    #[test]
    fn default_order_puts_subscriptions_first_and_mock_last() {
        let mock = info(ProviderKind::Mock, "mock-smart");
        let ollama = info(ProviderKind::Ollama, "llama3.2");
        let openai = info(ProviderKind::OpenAi, "gpt-4o");
        let claude = info(ProviderKind::ClaudeCode, "sonnet");
        let codex = info(ProviderKind::Codex, "gpt-5-codex");
        let all = vec![mock.clone(), ollama.clone(), openai.clone(), claude.clone(), codex.clone()];
        let order: Vec<ProviderId> =
            effective_sources(&RoutingConfig::default(), &all).into_iter().map(|s| s.provider_id).collect();
        assert_eq!(
            order,
            vec![claude.provider_id, codex.provider_id, openai.provider_id, ollama.provider_id, mock.provider_id]
        );
        // A stored order wins; unknown ids are dropped, new providers appended.
        let cfg = RoutingConfig {
            source_priority: vec![
                AiSource { provider_id: openai.provider_id, enabled: true, model: None, capabilities: None },
                AiSource { provider_id: ProviderId::new(), enabled: true, model: None, capabilities: None },
            ],
            ..RoutingConfig::default()
        };
        let order: Vec<ProviderId> = effective_sources(&cfg, &all).into_iter().map(|s| s.provider_id).collect();
        assert_eq!(order[0], openai.provider_id);
        assert_eq!(order.len(), 5);
    }

    #[test]
    fn chain_follows_priority_capabilities_overrides_and_cooldown() {
        let a = info(ProviderKind::Anthropic, "claude-sonnet-4-5");
        let o = info(ProviderKind::OpenAi, "gpt-4o");
        let l = info(ProviderKind::Ollama, "llama3.2");
        let providers = vec![a.clone(), o.clone(), l.clone()];
        let cands = vec![
            cand(&a, "claude-sonnet-4-5", true),
            cand(&o, "gpt-4o", true),
            cand(&o, "gpt-4o-mini", true),
            cand(&l, "llama3.2", false),
        ];
        let src = |p: &SourceInfo, enabled: bool| AiSource {
            provider_id: p.provider_id,
            enabled,
            model: None,
            capabilities: None,
        };
        let mut cfg = RoutingConfig {
            source_priority: vec![src(&l, true), src(&o, true), src(&a, true)],
            ..RoutingConfig::default()
        };
        let req = RouteRequest { purpose: Some("Manager".into()), ..RouteRequest::default() };
        let routed = crate::router::route(&req, &cands, &cfg);
        let chain = prioritize(routed.clone(), &req, &cands, &cfg, &providers);
        assert_eq!(&chain[..3], &[mr(&l, "llama3.2"), mr(&o, "gpt-4o"), mr(&a, "claude-sonnet-4-5")]);

        // Explicit preferred model stays at the head.
        let pinned = RouteRequest { preferred: Some(mr(&a, "claude-sonnet-4-5")), ..req.clone() };
        let r = crate::router::route(&pinned, &cands, &cfg);
        assert_eq!(prioritize(r, &pinned, &cands, &cfg, &providers)[0], mr(&a, "claude-sonnet-4-5"));

        // Disabled source is skipped; vision requirement drops the non-vision source.
        cfg.source_priority[0].enabled = false;
        let chain = prioritize(routed.clone(), &req, &cands, &cfg, &providers);
        assert!(!chain.iter().any(|m| m.provider_id == l.provider_id));
        cfg.source_priority[0].enabled = true;
        cfg.source_priority[1].capabilities =
            Some(SourceCapabilities { computer_use: false, ..default_capabilities(ProviderKind::OpenAi) });
        let cu = RouteRequest {
            purpose: Some("Cursor".into()),
            required: RequiredCapabilities { vision: true, computer_use: true, ..Default::default() },
            ..RouteRequest::default()
        };
        let r = crate::router::route(&cu, &cands, &cfg);
        let chain = prioritize(r, &cu, &cands, &cfg, &providers);
        assert_eq!(chain, vec![mr(&a, "claude-sonnet-4-5")], "only anthropic sees + drives the screen");

        // Per-purpose override replaces the global order.
        cfg.purpose_sources = vec![PurposeSources { purpose: "voice".into(), sources: vec![a.provider_id] }];
        let v = RouteRequest { purpose: Some("VoicePhrase".into()), ..RouteRequest::default() };
        let r = crate::router::route(&v, &cands, &cfg);
        assert_eq!(prioritize(r, &v, &cands, &cfg, &providers), vec![mr(&a, "claude-sonnet-4-5")]);

        // A cooling source is demoted to the end, never dropped.
        let mut cold = providers.clone();
        cold[2].cooling = true; // ollama, first by priority
        let chain = prioritize(routed.clone(), &req, &cands, &cfg, &cold);
        assert_eq!(chain[0], mr(&o, "gpt-4o"));
        assert_eq!(chain.last().unwrap().provider_id, l.provider_id);

        // Failover off: the routed chain is untouched.
        cfg.failover = false;
        assert_eq!(prioritize(routed.clone(), &req, &cands, &cfg, &providers), routed);
    }

    mod chain {
        use super::*;
        use crate::adapters::mock::{MockAdapter, MockOptions};
        use parking_lot::Mutex;
        use std::collections::HashMap;
        use std::time::Duration;

        #[derive(Default)]
        struct Src {
            adapters: HashMap<ProviderId, Arc<dyn ProviderAdapter>>,
            outcomes: Mutex<Vec<(ModelRef, AttemptOutcome)>>,
            events: Mutex<Vec<ProviderEvent>>,
        }
        impl AdapterSource for Src {
            fn adapter(&self, p: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
                self.adapters.get(p).cloned()
            }
            fn observe(&self, m: &ModelRef, o: &AttemptOutcome) {
                self.outcomes.lock().push((m.clone(), o.clone()));
            }
            fn emit(&self, e: ProviderEvent) {
                self.events.lock().push(e);
            }
        }

        fn two() -> (Arc<Src>, ModelRef, ModelRef) {
            let (a, b) = (ProviderId::new(), ProviderId::new());
            let mut s = Src::default();
            for id in [a, b] {
                s.adapters.insert(id, Arc::new(MockAdapter::new(id, "m", MockOptions { chunk_delay: Duration::ZERO })));
            }
            (
                Arc::new(s),
                ModelRef { provider_id: a, model: "x".into() },
                ModelRef { provider_id: b, model: "y".into() },
            )
        }

        #[tokio::test]
        async fn switches_on_usage_limit_and_reports() {
            let (src, a, b) = two();
            let chain = ProviderChain::new(src.clone(), vec![a.clone(), b.clone()]);
            let out = chain
                .run(|at| async move {
                    if at.attempt == 1 {
                        Err(ProviderError::QuotaExceeded("Claude AI usage limit reached".into()))
                    } else {
                        Ok(format!("answer from {}", at.model.model))
                    }
                })
                .await
                .unwrap();
            assert_eq!(out.value, "answer from y");
            assert_eq!(out.model, b);
            assert_eq!(
                out.switches,
                vec![ChainSwitch { from: a.clone(), to: b.clone(), reason: FailureKind::UsageLimit }]
            );
            let ev = src.events.lock().clone();
            assert!(matches!(&ev[0], ProviderEvent::Fallback { reason, .. } if reason == "usage_limit"));
            let outs = src.outcomes.lock().clone();
            assert!(matches!(&outs[0], (m, AttemptOutcome::Failure(_)) if *m == a));
            assert!(matches!(&outs[1], (m, AttemptOutcome::Success { .. }) if *m == b));
        }

        #[tokio::test]
        async fn non_retryable_errors_do_not_fail_over() {
            for e in [
                ProviderError::BadRequest("bad schema".into()),
                ProviderError::BadRequest("flagged by content policy".into()),
                ProviderError::Cancelled,
            ] {
                let (src, a, b) = two();
                let chain = ProviderChain::new(src.clone(), vec![a, b]);
                let calls = Arc::new(Mutex::new(0));
                let c2 = calls.clone();
                let err = chain
                    .run(move |_| {
                        let e = e.clone();
                        let c = c2.clone();
                        async move {
                            *c.lock() += 1;
                            Err::<(), _>(e)
                        }
                    })
                    .await
                    .unwrap_err();
                assert_eq!(*calls.lock(), 1, "{err}");
                assert!(src.events.lock().is_empty());
            }
        }

        #[tokio::test]
        async fn exhausted_chain_returns_the_last_error() {
            let (src, a, b) = two();
            let chain = ProviderChain::new(src, vec![a, b]);
            let err = chain.run(|_| async { Err::<(), _>(ProviderError::Network("down".into())) }).await.unwrap_err();
            assert!(matches!(err, ProviderError::Network(_)));
            let empty = ProviderChain::new(Arc::new(Src::default()), vec![]);
            assert!(matches!(empty.run(|_| async { Ok(()) }).await, Err(ProviderError::Unsupported(_))));
        }
    }
}
