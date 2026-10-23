//! Parallel engines (`engines.*`, ADR-0047): the Claude Code and Codex
//! subscriptions side by side.
//!
//! The configuration (`<data_dir>/engines.json`) is applied to the existing
//! Manager settings and routing rules — nothing new in the router:
//!
//! * `single` — the Manager and every role on `primary`;
//! * `split` — roles in `codexRoles` on Codex, the others on Claude Code,
//!   the Manager on `manager`;
//! * `balanced` — like `split` at first; whenever an agent starts or stops,
//!   every role is re-pointed to the engine with fewer running agents and
//!   more plan headroom (new spawns follow the load);
//! * `race` — reviews on one engine and adversarial reviews on the other,
//!   consensus with both;
//! * `consensus` — the `Consensus` purpose carries both engines, so Phase 2
//!   consensus asks Claude Code and Codex for independent proposals.
//!
//! With `crossFallback` the other engine is always the first fallback, so a
//! usage limit on one subscription continues the work on the other (the
//! existing CLI usage-limit fallback with a structured handoff).

use std::path::Path;
use std::sync::Arc;

use orkestr_domain::{AgentState, ModelRef, ProviderId, ProviderKind, RoutingRule};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::agents::ListAgentsParams;
use orkestr_protocol::methods::codex::{EngineId, EngineStatus, EnginesConfig, EnginesMode, EnginesStatus};
use orkestr_protocol::RpcError;
use orkestr_security::NewAudit;

use crate::Services;

/// Configuration file (inside the data dir).
pub const CONFIG_FILE: &str = "engines.json";

/// Routing purposes the engines own (every role + the decision roles).
pub const ROLES: &[&str] = &[
    "Architect",
    "Backend",
    "Frontend",
    "Database",
    "QA",
    "Security",
    "DevOps",
    "UI/UX",
    "Product Manager",
    "Reviewer",
    "Researcher",
    "Coding",
    "Merge",
    crate::claude_engine::CLAUDE_ROLE,
    crate::codex::CODEX_ROLE,
];

/// Loads the configuration.
#[must_use]
pub fn load(data_dir: &Path) -> EnginesConfig {
    std::fs::read_to_string(data_dir.join(CONFIG_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, cfg: &EnginesConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let path = data_dir.join(CONFIG_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("cannot save the engines settings: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot save the engines settings: {e}"))
}

/// The engine of each routing purpose under `cfg` (pure; tested).
/// `load` = running agents `(claude, codex)` for `balanced`.
#[must_use]
pub fn plan(cfg: &EnginesConfig, load: (u32, u32), headroom: (f64, f64)) -> Vec<(&'static str, EngineId)> {
    let mut out: Vec<(&'static str, EngineId)> = Vec::new();
    let on_codex = |r: &str| cfg.codex_roles.iter().any(|c| c.eq_ignore_ascii_case(r));
    let lighter = {
        // Fewer running agents wins; a nearly exhausted plan counts as busy.
        let score = |n: u32, used: f64| f64::from(n) + if used >= 90.0 { 1000.0 } else { used / 50.0 };
        if score(load.1, headroom.1) < score(load.0, headroom.0) {
            EngineId::Codex
        } else {
            EngineId::ClaudeCode
        }
    };
    for role in ROLES {
        let e = match cfg.mode {
            EnginesMode::Off => continue,
            EnginesMode::Single => cfg.primary,
            EnginesMode::Split | EnginesMode::Race | EnginesMode::Consensus => {
                if *role == crate::codex::CODEX_ROLE || on_codex(role) {
                    EngineId::Codex
                } else if *role == crate::claude_engine::CLAUDE_ROLE {
                    EngineId::ClaudeCode
                } else {
                    cfg.primary_for_split()
                }
            }
            EnginesMode::Balanced => {
                if *role == crate::codex::CODEX_ROLE {
                    EngineId::Codex
                } else if *role == crate::claude_engine::CLAUDE_ROLE {
                    EngineId::ClaudeCode
                } else {
                    lighter
                }
            }
        };
        out.push((role, e));
    }
    match cfg.mode {
        EnginesMode::Off => {}
        EnginesMode::Race => {
            out.push(("Review", cfg.manager));
            out.push(("Adversarial Review", cfg.manager.other()));
            out.push(("Consensus", cfg.manager));
        }
        EnginesMode::Single => {
            for p in ["Review", "Adversarial Review", "Consensus"] {
                out.push((p, cfg.primary));
            }
        }
        _ => {
            out.push(("Review", cfg.manager));
            out.push(("Adversarial Review", cfg.manager.other()));
            out.push(("Consensus", cfg.manager));
        }
    }
    out
}

trait SplitDefault {
    fn primary_for_split(&self) -> EngineId;
}

impl SplitDefault for EnginesConfig {
    fn primary_for_split(&self) -> EngineId {
        if self.mode == EnginesMode::Split {
            EngineId::ClaudeCode
        } else {
            self.primary
        }
    }
}

/// Puts `first` at the head of a rule, `second` (when given) as its first
/// fallback, keeping the previous models after them (pure; tested).
pub fn point(current: &mut ModelRef, fallback: &mut Vec<ModelRef>, first: &ModelRef, second: Option<&ModelRef>) {
    crate::claude_engine::promote(current, fallback, first);
    if let Some(s) = second.filter(|s| *s != first) {
        fallback.retain(|m| m != s);
        fallback.insert(0, s.clone());
    }
    fallback.truncate(8);
}

/// Model of an engine.
fn model_of(cfg: &EnginesConfig, e: EngineId, ids: (Option<ProviderId>, Option<ProviderId>)) -> Option<ModelRef> {
    match e {
        EngineId::ClaudeCode => ids.0.map(|provider_id| ModelRef { provider_id, model: cfg.claude_model.clone() }),
        EngineId::Codex => ids.1.map(|provider_id| ModelRef { provider_id, model: cfg.codex_model.clone() }),
    }
}

fn kind_engine(kind: ProviderKind) -> Option<EngineId> {
    match kind {
        ProviderKind::ClaudeCode => Some(EngineId::ClaudeCode),
        ProviderKind::Codex => Some(EngineId::Codex),
        _ => None,
    }
}

async fn running(services: &Services) -> (u32, u32) {
    let agents = services
        .agents
        .list(ListAgentsParams { active_only: Some(true), ..ListAgentsParams::default() })
        .await
        .unwrap_or_default();
    let mut n = (0, 0);
    for a in agents.iter().filter(|a| !matches!(a.state, AgentState::Idle)) {
        let Some(m) = &a.model else { continue };
        let kind = services.providers.get(m.provider_id).ok().map(|v| v.provider.kind);
        match kind.and_then(kind_engine) {
            Some(EngineId::ClaudeCode) => n.0 += 1,
            Some(EngineId::Codex) => n.1 += 1,
            None => {}
        }
    }
    n
}

/// Applies `cfg` to the Manager settings and routing rules.
pub async fn apply(services: &Services, cfg: &EnginesConfig) -> Result<(), RpcError> {
    if cfg.mode == EnginesMode::Off {
        return Ok(());
    }
    let claude = crate::claude_engine::ensure_provider(services, false).await.ok();
    let codex_st = crate::codex::status(services, false).await;
    let codex = codex_st.provider_id;
    let ids = (claude, codex);
    let needed: Vec<EngineId> = plan(cfg, (0, 0), (0.0, 0.0)).iter().map(|(_, e)| *e).chain([cfg.manager]).collect();
    for e in [EngineId::ClaudeCode, EngineId::Codex] {
        if needed.contains(&e) && model_of(cfg, e, ids).is_none() {
            let what = if e == EngineId::Codex { "Codex (ChatGPT)" } else { "Claude Code" };
            return Err(RpcError::bad_request(format!(
                "{what} is not ready (install it and sign in first) — the {:?} mode needs it",
                cfg.mode
            )));
        }
    }
    let load = running(services).await;
    let codex_used = services.codex.cached_limits().and_then(|l| max_percent(&l)).unwrap_or(0.0);
    // Claude Code's plan windows need a probe process; only Codex's cached
    // limits weigh in here (the status shows both).
    let headroom = (0.0, codex_used);
    let to_rpc = crate::rpc::providers::to_rpc;
    let fb = |e: EngineId| if cfg.cross_fallback { model_of(cfg, e.other(), ids) } else { None };

    let mut mgr = services.providers.manager_config().await.map_err(to_rpc)?;
    let m_engine = if cfg.mode == EnginesMode::Single { cfg.primary } else { cfg.manager };
    if let Some(first) = model_of(cfg, m_engine, ids) {
        match mgr.model.as_mut() {
            Some(cur) => point(cur, &mut mgr.fallback_chain, &first, fb(m_engine).as_ref()),
            None => {
                mgr.model = Some(first.clone());
                mgr.fallback_chain.retain(|m| *m != first);
                if let Some(s) = fb(m_engine) {
                    mgr.fallback_chain.retain(|m| *m != s);
                    mgr.fallback_chain.insert(0, s);
                }
            }
        }
        services.providers.set_manager_config(mgr).await.map_err(to_rpc)?;
    }
    let mut routing = services.providers.routing_config().await.map_err(to_rpc)?;
    for (purpose, engine) in plan(cfg, load, headroom) {
        let Some(first) = model_of(cfg, engine, ids) else { continue };
        let second = fb(engine);
        match routing.rules.iter_mut().find(|r| r.purpose.eq_ignore_ascii_case(purpose)) {
            Some(rule) => point(&mut rule.model, &mut rule.fallback, &first, second.as_ref()),
            None => routing.rules.push(RoutingRule {
                purpose: purpose.to_owned(),
                model: first,
                fallback: second.into_iter().collect(),
            }),
        }
    }
    services.providers.set_routing_config(routing).await.map_err(to_rpc)?;
    Ok(())
}

fn max_percent(l: &orkestr_protocol::methods::codex::CodexLimits) -> Option<f64> {
    [l.primary.as_ref(), l.secondary.as_ref()].into_iter().flatten().map(|w| w.percent).reduce(f64::max)
}

/// `engines.status`.
pub async fn status(services: &Services, refresh: bool) -> EnginesStatus {
    let config = load(services.claude.data_dir());
    let c = crate::claude_engine::status(services, refresh).await;
    let x = crate::codex::status(services, refresh).await;
    let load = running(services).await;
    let usage = if c.installed && c.signed_in {
        let exts = services.extensions.enabled_dirs().await.unwrap_or_default();
        let program = services.claude.binary(exts).await;
        Some(services.claude.usage(None, program, false).await)
    } else {
        None
    };
    let claude_windows: Vec<&orkestr_protocol::methods::claude_code::ClaudeLimitWindow> =
        usage.iter().flat_map(|u| [u.session.as_ref(), u.weekly.as_ref()]).flatten().collect();
    let claude_max = claude_windows.iter().max_by(|a, b| a.percent.total_cmp(&b.percent));
    let claude = EngineStatus {
        engine: EngineId::ClaudeCode,
        installed: c.installed,
        signed_in: c.signed_in,
        version: None,
        plan: c.subscription.clone(),
        provider_id: c.provider_id,
        limit_percent: claude_max.map(|w| w.percent),
        limit_resets_at: claude_max.and_then(|w| w.resets_at),
        running_agents: load.0,
        reason: c.reason.clone(),
    };
    let limits = if x.logged_in { Some(services.codex.limits(refresh).await) } else { None };
    let fullest = limits.as_ref().and_then(|l| {
        [l.primary.as_ref(), l.secondary.as_ref()]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.percent.total_cmp(&b.percent))
            .cloned()
    });
    let codex = EngineStatus {
        engine: EngineId::Codex,
        installed: x.binary.is_some(),
        signed_in: x.logged_in,
        version: x.binary.as_ref().and_then(|b| b.version.clone()),
        plan: x.account.as_ref().and_then(|a| a.plan.clone()).or_else(|| limits.as_ref().and_then(|l| l.plan.clone())),
        provider_id: x.provider_id,
        limit_percent: fullest.as_ref().map(|w| w.percent),
        limit_resets_at: fullest.and_then(|w| w.resets_at),
        running_agents: load.1,
        reason: x.error.clone(),
    };
    EnginesStatus { claude, codex, config }
}

/// `engines.setConfig`.
pub async fn set_config(services: &Services, cfg: EnginesConfig) -> Result<EnginesStatus, RpcError> {
    let mut cfg = cfg;
    cfg.codex_roles.retain(|r| ROLES.iter().any(|x| x.eq_ignore_ascii_case(r)));
    cfg.codex_roles.dedup();
    let valid = |m: &str| orkestr_providers::codex_app::is_token(m);
    if !valid(&cfg.claude_model) || !valid(&cfg.codex_model) {
        return Err(RpcError::bad_request("invalid model name"));
    }
    apply(services, &cfg).await?;
    save(services.claude.data_dir(), &cfg).map_err(RpcError::internal)?;
    let sec = &services.security;
    sec.audit
        .record(
            NewAudit::new(
                sec.user(),
                "engines.setConfig",
                format!("{:?}", cfg.mode),
                orkestr_domain::AuditOutcome::Executed,
            )
            .details(serde_json::to_value(&cfg).unwrap_or_default()),
        )
        .await;
    Ok(status(services, false).await)
}

/// `balanced` mode: re-points the roles whenever an agent starts or stops.
pub fn spawn_balancer(services: Arc<Services>) {
    tokio::spawn(async move {
        let mut sub = services.events.subscribe_internal_all();
        while let Some(rcv) = sub.recv().await {
            let orkestr_events::Received::Event(ev) = rcv else { continue };
            if !matches!(ev.kind, EventKind::AgentStarted(_) | EventKind::AgentStopped(_)) {
                continue;
            }
            let cfg = load(services.claude.data_dir());
            if cfg.mode != EnginesMode::Balanced {
                continue;
            }
            if let Err(e) = apply(&services, &cfg).await {
                tracing::debug!(error = %e.message, "balanced engines: could not re-point the roles");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: EnginesMode) -> EnginesConfig {
        EnginesConfig { mode, ..EnginesConfig::default() }
    }

    fn engine(p: &[(&str, EngineId)], purpose: &str) -> EngineId {
        p.iter().find(|(r, _)| *r == purpose).unwrap().1
    }

    #[test]
    fn split_single_and_balanced_plans() {
        assert!(plan(&cfg(EnginesMode::Off), (0, 0), (0.0, 0.0)).is_empty());
        let s = plan(&cfg(EnginesMode::Split), (0, 0), (0.0, 0.0));
        assert_eq!(engine(&s, "Backend"), EngineId::Codex, "Backend is in the default codex roles");
        assert_eq!(engine(&s, "Frontend"), EngineId::ClaudeCode);
        assert_eq!(engine(&s, "Codex"), EngineId::Codex);
        assert_eq!(engine(&s, "Adversarial Review"), EngineId::Codex, "reviewed by the other engine");
        let one = EnginesConfig { primary: EngineId::Codex, ..cfg(EnginesMode::Single) };
        let s = plan(&one, (0, 0), (0.0, 0.0));
        assert!(s.iter().filter(|(r, _)| *r != "Claude Code").all(|(_, e)| *e == EngineId::Codex));
        // Balanced: the lighter engine takes new work; an exhausted plan never does.
        let b = plan(&cfg(EnginesMode::Balanced), (3, 1), (0.0, 0.0));
        assert_eq!(engine(&b, "Frontend"), EngineId::Codex);
        let b = plan(&cfg(EnginesMode::Balanced), (3, 1), (0.0, 95.0));
        assert_eq!(engine(&b, "Frontend"), EngineId::ClaudeCode);
        let b = plan(&cfg(EnginesMode::Balanced), (0, 2), (0.0, 0.0));
        assert_eq!(engine(&b, "Frontend"), EngineId::ClaudeCode);
    }

    #[test]
    fn point_puts_the_other_engine_first_in_the_fallbacks() {
        let a = ModelRef { provider_id: ProviderId::new(), model: "sonnet".into() };
        let b = ModelRef { provider_id: ProviderId::new(), model: "default".into() };
        let old = ModelRef { provider_id: ProviderId::new(), model: "gpt-5".into() };
        let mut cur = old.clone();
        let mut fb = vec![b.clone()];
        point(&mut cur, &mut fb, &a, Some(&b));
        assert_eq!(cur, a);
        assert_eq!(fb, vec![b, old], "the other engine first, the previous model kept");
    }
}
