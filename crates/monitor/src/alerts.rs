//! Alert rules, dedup and resolution. **Owner: WP5.**
//!
//! An alert is identified by its *dedup key* `rule|source`. While an alert
//! with that key is open (or acknowledged) a new trigger only bumps `count`;
//! `AlertRaised` is published once per opening and `AlertResolved` once when
//! the condition clears.
//!
//! Rule kinds:
//! * [`RuleKind::CheckDown`] — a health check went `Down` (resolved on `Up`);
//! * [`RuleKind::MetricThreshold`] — `metric > above` for `samples`
//!   consecutive samples; resolved after `samples` consecutive samples at or
//!   below the threshold (hysteresis);
//! * [`RuleKind::LogRate`] — at least `count` matching lines (level ≥
//!   `min_level`, optional case-insensitive `pattern`) within `window_secs`;
//!   resolved after a full window without matches.

use std::collections::{HashMap, VecDeque};

use chrono::Duration as ChronoDuration;
use orkestr_domain::{AlertId, NodeId, ProjectId, Timestamp};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, LogLevel};
use orkestr_protocol::methods::monitor::{Alert, AlertPayload, AlertSeverity, AlertSource, AlertState, LogLine};
use orkestr_protocol::NewEvent;
use parking_lot::Mutex;

use crate::logs::redact_line;
use crate::store::MonitorStore;
use crate::MonitorError;

/// Alert rules are wire types (`monitor.rules`, `monitor.ruleUpsert`); the
/// JSON shape stored in `monitor_alert_rules` is unchanged.
pub use orkestr_protocol::methods::monitor::{AlertRule, AlertRuleKind as RuleKind};

/// Largest `LogRate` window (one day).
pub const MAX_WINDOW_SECS: u32 = 86_400;
/// Largest `LogRate` count.
pub const MAX_LOG_COUNT: u32 = 100_000;
/// Largest `MetricThreshold` sample run.
pub const MAX_SAMPLES: u32 = 1000;

fn valid_metric(m: &str) -> bool {
    matches!(m, "cpu" | "mem" | "disk")
        || m.strip_prefix("gpu").is_some_and(|n| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Validates a rule (id charset, name, kind-specific thresholds and windows).
pub fn validate_rule(rule: &AlertRule) -> Result<(), MonitorError> {
    let bad = |m: &str| Err(MonitorError::Invalid(m.to_owned()));
    let id_ok = !rule.id.is_empty()
        && rule.id.len() <= 64
        && rule.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !id_ok {
        return bad("rule id must be 1..64 of [A-Za-z0-9._-]");
    }
    let name = rule.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return bad("rule name must be 1..=120 characters");
    }
    match &rule.kind {
        RuleKind::CheckDown { .. } => {}
        RuleKind::MetricThreshold { metric, above, samples, .. } => {
            if !valid_metric(metric) {
                return bad("metric must be cpu, mem, disk or gpuN");
            }
            if !(above.is_finite() && (0.0..100.0).contains(above)) {
                return bad("threshold (above) must be within [0, 100) percent");
            }
            if !(1..=MAX_SAMPLES).contains(samples) {
                return bad("samples must be within 1..=1000");
            }
        }
        RuleKind::LogRate { source_id, pattern, count, window_secs, .. } => {
            if !(1..=MAX_LOG_COUNT).contains(count) {
                return bad("count must be within 1..=100000");
            }
            if !(1..=MAX_WINDOW_SECS).contains(window_secs) {
                return bad("windowSecs must be within 1..=86400");
            }
            if pattern.as_deref().is_some_and(|p| p.trim().is_empty() || p.chars().count() > 200) {
                return bad("pattern must be 1..=200 characters when set");
            }
            if source_id.as_deref().is_some_and(|s| s.is_empty() || s.len() > 200) {
                return bad("sourceId must be 1..=200 characters when set");
            }
        }
    }
    Ok(())
}

/// Rules seeded on first start (editable through [`crate::MonitorService::upsert_rule`]).
#[must_use]
pub fn default_rules() -> Vec<AlertRule> {
    let r = |id: &str, name: &str, severity, kind| AlertRule {
        id: id.into(),
        name: name.into(),
        severity,
        enabled: true,
        project_id: None,
        kind,
    };
    vec![
        r("check-down", "Health check down", AlertSeverity::Critical, RuleKind::CheckDown { check_id: None }),
        r(
            "cpu-high",
            "CPU above 95%",
            AlertSeverity::Warning,
            RuleKind::MetricThreshold { metric: "cpu".into(), node_id: None, above: 95.0, samples: 12 },
        ),
        r(
            "mem-high",
            "Memory above 95%",
            AlertSeverity::Warning,
            RuleKind::MetricThreshold { metric: "mem".into(), node_id: None, above: 95.0, samples: 6 },
        ),
        r(
            "disk-high",
            "Disk above 95%",
            AlertSeverity::Critical,
            RuleKind::MetricThreshold { metric: "disk".into(), node_id: None, above: 95.0, samples: 2 },
        ),
        r(
            "log-errors",
            "Error burst in logs",
            AlertSeverity::Warning,
            RuleKind::LogRate {
                source_id: None,
                min_level: Some(LogLevel::Error),
                pattern: None,
                count: 20,
                window_secs: 60,
            },
        ),
    ]
}

/// A trigger produced by an evaluator.
#[derive(Debug, Clone)]
pub enum Signal {
    /// Raise (or bump) the alert.
    Raise {
        /// Rule.
        rule: Box<AlertRule>,
        /// Source.
        source: AlertSource,
        /// Dedup source part.
        source_key: String,
        /// Project.
        project_id: Option<ProjectId>,
        /// Title.
        title: String,
        /// Message.
        message: String,
    },
    /// Resolve the alert with this key if open.
    Resolve {
        /// `rule|source`.
        key: String,
    },
}

/// Dedup key.
#[must_use]
pub fn dedup_key(rule: &str, source_key: &str) -> String {
    format!("{rule}|{source_key}")
}

#[derive(Default)]
struct EvalState {
    /// (rule, node) → (consecutive above, consecutive below).
    metric: HashMap<(String, NodeId), (u32, u32)>,
    /// (rule, source) → recent match times + project.
    log: HashMap<(String, String), (VecDeque<Timestamp>, Option<ProjectId>)>,
}

/// Open alerts + rule evaluation state.
pub struct AlertEngine {
    store: MonitorStore,
    events: EventBus,
    rules: Mutex<Vec<AlertRule>>,
    open: tokio::sync::Mutex<HashMap<String, Alert>>,
    eval: Mutex<EvalState>,
}

impl std::fmt::Debug for AlertEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertEngine").finish_non_exhaustive()
    }
}

impl AlertEngine {
    /// Creates the engine with `rules` and previously open alerts.
    #[must_use]
    pub fn new(store: MonitorStore, events: EventBus, rules: Vec<AlertRule>, open: Vec<(String, Alert)>) -> Self {
        Self {
            store,
            events,
            rules: Mutex::new(rules),
            open: tokio::sync::Mutex::new(open.into_iter().collect()),
            eval: Mutex::new(EvalState::default()),
        }
    }

    /// Restores alerts that were open before a restart.
    pub async fn restore_open(&self, open: Vec<(String, Alert)>) {
        self.open.lock().await.extend(open);
    }

    /// Current rules.
    #[must_use]
    pub fn rules(&self) -> Vec<AlertRule> {
        self.rules.lock().clone()
    }

    /// Replaces the rule set (evaluation state of removed rules is dropped).
    pub fn set_rules(&self, rules: Vec<AlertRule>) {
        let ids: Vec<String> = rules.iter().map(|r| r.id.clone()).collect();
        *self.rules.lock() = rules;
        let mut e = self.eval.lock();
        e.metric.retain(|(r, _), _| ids.contains(r));
        e.log.retain(|(r, _), _| ids.contains(r));
    }

    /// Open (unresolved) alerts.
    pub async fn open_alerts(&self) -> Vec<Alert> {
        let mut v: Vec<Alert> = self.open.lock().await.values().cloned().collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.opened_at));
        v
    }

    /// Applies a signal; returns the alert when it was raised, bumped or resolved.
    pub async fn apply(&self, s: Signal) -> Result<Option<Alert>, MonitorError> {
        match s {
            Signal::Raise { rule, source, source_key, project_id, title, message } => {
                let key = dedup_key(&rule.id, &source_key);
                let mut open = self.open.lock().await;
                if let Some(a) = open.get_mut(&key) {
                    a.count = a.count.saturating_add(1);
                    a.message = redact_line(&message);
                    let a = a.clone();
                    drop(open);
                    self.store.save_alert(&a, &key).await?;
                    return Ok(Some(a));
                }
                let alert = Alert {
                    id: AlertId::new(),
                    project_id,
                    rule: rule.id.clone(),
                    severity: rule.severity,
                    title: redact_line(&title),
                    message: redact_line(&message),
                    source,
                    state: AlertState::Open,
                    count: 1,
                    opened_at: orkestr_domain::now(),
                    acked_at: None,
                    resolved_at: None,
                };
                open.insert(key.clone(), alert.clone());
                drop(open);
                self.store.save_alert(&alert, &key).await?;
                self.publish(EventKind::AlertRaised(AlertPayload { alert: alert.clone() })).await;
                Ok(Some(alert))
            }
            Signal::Resolve { key } => {
                let Some(mut a) = self.open.lock().await.remove(&key) else { return Ok(None) };
                a.state = AlertState::Resolved;
                a.resolved_at = Some(orkestr_domain::now());
                self.store.save_alert(&a, &key).await?;
                self.publish(EventKind::AlertResolved(AlertPayload { alert: a.clone() })).await;
                Ok(Some(a))
            }
        }
    }

    /// Acknowledges an open alert (it stays open until resolved).
    pub async fn ack(&self, id: AlertId) -> Result<Alert, MonitorError> {
        let mut open = self.open.lock().await;
        if let Some((key, a)) = open.iter_mut().find(|(_, a)| a.id == id) {
            if a.state == AlertState::Open {
                a.state = AlertState::Acknowledged;
                a.acked_at = Some(orkestr_domain::now());
            }
            let (key, a) = (key.clone(), a.clone());
            drop(open);
            self.store.save_alert(&a, &key).await?;
            return Ok(a);
        }
        drop(open);
        // Resolved alerts can still be read (ack is a no-op).
        self.store.get_alert(id).await?.ok_or_else(|| MonitorError::NotFound(format!("alert {id}")))
    }

    async fn publish(&self, kind: EventKind) {
        if let Err(e) = self.events.publish(NewEvent::new(kind)).await {
            tracing::warn!(error = %e, "monitor alert event publication failed");
        }
    }

    /// Signals for a check transition.
    #[must_use]
    pub fn on_check(
        &self,
        check: &orkestr_protocol::methods::monitor::HealthCheck,
        down: bool,
        error: Option<&str>,
    ) -> Vec<Signal> {
        let source_key = format!("check:{}", check.id);
        self.rules
            .lock()
            .iter()
            .filter(|r| r.enabled && scope_matches(r.project_id, check.project_id))
            .filter_map(|r| match &r.kind {
                RuleKind::CheckDown { check_id } if check_id.is_none_or(|c| c == check.id) => Some(if down {
                    Signal::Raise {
                        rule: Box::new(r.clone()),
                        source: AlertSource::Check { check_id: check.id },
                        source_key: source_key.clone(),
                        project_id: check.project_id,
                        title: format!("{}: {}", r.name, check.name),
                        message: error.unwrap_or("check failed").to_owned(),
                    }
                } else {
                    Signal::Resolve { key: dedup_key(&r.id, &source_key) }
                }),
                _ => None,
            })
            .collect()
    }

    /// Signals for a new metric sample of `node` (`value(metric)` reads it).
    pub fn on_metrics(&self, node: NodeId, node_name: &str, value: impl Fn(&str) -> Option<f64>) -> Vec<Signal> {
        let rules = self.rules.lock().clone();
        let mut eval = self.eval.lock();
        let mut out = Vec::new();
        for r in rules.iter().filter(|r| r.enabled) {
            let RuleKind::MetricThreshold { metric, node_id, above, samples } = &r.kind else { continue };
            if node_id.is_some_and(|n| n != node) {
                continue;
            }
            let Some(v) = value(metric) else { continue };
            let need = (*samples).max(1);
            let entry = eval.metric.entry((r.id.clone(), node)).or_insert((0, 0));
            let source_key = format!("metric:{node}:{metric}");
            if v > *above {
                entry.0 = entry.0.saturating_add(1);
                entry.1 = 0;
                if entry.0 >= need {
                    out.push(Signal::Raise {
                        rule: Box::new(r.clone()),
                        source: AlertSource::Metric { node_id: node, metric: metric.clone() },
                        source_key,
                        project_id: None,
                        title: format!("{}: {node_name}", r.name),
                        message: format!("{metric} at {v:.1}% (> {above:.1}% for {need} samples)"),
                    });
                }
            } else {
                entry.1 = entry.1.saturating_add(1);
                entry.0 = 0;
                if entry.1 == need {
                    out.push(Signal::Resolve { key: dedup_key(&r.id, &source_key) });
                }
            }
        }
        out
    }

    /// Signals for a stored log line.
    pub fn on_log(&self, line: &LogLine, project: Option<ProjectId>) -> Vec<Signal> {
        let rules = self.rules.lock().clone();
        let mut eval = self.eval.lock();
        let mut out = Vec::new();
        for r in rules.iter().filter(|r| r.enabled && scope_matches(r.project_id, project)) {
            let RuleKind::LogRate { source_id, min_level, pattern, count, window_secs } = &r.kind else { continue };
            if source_id.as_ref().is_some_and(|s| *s != line.source_id) {
                continue;
            }
            let level_ok = min_level.is_none_or(|m| line.level.is_some_and(|l| l >= m));
            let pattern_ok = pattern.as_deref().is_none_or(|p| line.message.to_lowercase().contains(&p.to_lowercase()));
            if !(level_ok && pattern_ok) {
                continue;
            }
            let (times, proj) = eval.log.entry((r.id.clone(), line.source_id.clone())).or_default();
            *proj = project;
            let cutoff = line.ts - ChronoDuration::seconds(i64::from(*window_secs));
            times.push_back(line.ts);
            while times.front().is_some_and(|t| *t < cutoff) {
                times.pop_front();
            }
            let cap = usize::try_from(*count).unwrap_or(usize::MAX).max(1);
            while times.len() > cap {
                times.pop_front();
            }
            if times.len() >= cap {
                out.push(Signal::Raise {
                    rule: Box::new(r.clone()),
                    source: AlertSource::Log { source_id: line.source_id.clone() },
                    source_key: format!("log:{}", line.source_id),
                    project_id: project,
                    title: format!("{}: {}", r.name, line.source_id),
                    message: format!("{} matching lines within {window_secs}s; last: {}", times.len(), line.message),
                });
            }
        }
        out
    }

    /// Resolves log-rate alerts whose window has been quiet; call periodically.
    pub fn tick(&self, now: Timestamp) -> Vec<Signal> {
        let rules = self.rules.lock().clone();
        let mut eval = self.eval.lock();
        let mut out = Vec::new();
        eval.log.retain(|(rule_id, source), (times, _)| {
            let Some(RuleKind::LogRate { window_secs, .. }) = rules.iter().find(|r| &r.id == rule_id).map(|r| &r.kind)
            else {
                return false;
            };
            let cutoff = now - ChronoDuration::seconds(i64::from(*window_secs));
            if times.back().is_none_or(|t| *t < cutoff) {
                out.push(Signal::Resolve { key: dedup_key(rule_id, &format!("log:{source}")) });
                false
            } else {
                true
            }
        });
        out
    }
}

fn scope_matches(rule: Option<ProjectId>, subject: Option<ProjectId>) -> bool {
    rule.is_none() || rule == subject
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::monitor::{CheckProbe, HealthCheck};

    async fn engine() -> (AlertEngine, orkestr_events::Subscription) {
        let db = orkestr_storage::Db::open_in_memory().unwrap();
        db.migrate(crate::store::migrations()).await.unwrap();
        let events = EventBus::in_memory(256);
        let sub = events.subscribe_all();
        (AlertEngine::new(MonitorStore::new(db), events, default_rules(), Vec::new()), sub)
    }

    async fn kinds(sub: &mut orkestr_events::Subscription) -> Vec<&'static str> {
        let mut v = Vec::new();
        while let Ok(Some(orkestr_events::Received::Event(e))) =
            tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv()).await
        {
            v.push(e.kind_name());
        }
        v
    }

    #[tokio::test]
    async fn check_alert_dedups_and_resolves() {
        let (eng, mut sub) = engine().await;
        let check = HealthCheck {
            id: orkestr_domain::HealthCheckId::new(),
            project_id: None,
            environment_id: None,
            name: "api".into(),
            probe: CheckProbe::Tcp { host: "h".into(), port: 1 },
            interval_secs: 30,
            timeout_ms: 100,
            failure_threshold: 1,
            enabled: true,
        };
        for _ in 0..3 {
            for s in eng.on_check(&check, true, Some("refused password=hunter2xyz")) {
                eng.apply(s).await.unwrap();
            }
        }
        let open = eng.open_alerts().await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].count, 3);
        assert!(!open[0].message.contains("hunter2xyz"));
        let acked = eng.ack(open[0].id).await.unwrap();
        assert_eq!(acked.state, AlertState::Acknowledged);
        for s in eng.on_check(&check, false, None) {
            eng.apply(s).await.unwrap();
        }
        assert!(eng.open_alerts().await.is_empty());
        // Resolving again is a no-op (no duplicate event).
        for s in eng.on_check(&check, false, None) {
            assert!(eng.apply(s).await.unwrap().is_none());
        }
        assert_eq!(kinds(&mut sub).await, ["AlertRaised", "AlertResolved"]);
        let stored = eng.store.list_alerts(None, true, 10).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, AlertState::Resolved);
        // A new failure opens a fresh alert.
        for s in eng.on_check(&check, true, None) {
            eng.apply(s).await.unwrap();
        }
        assert_eq!(eng.store.list_alerts(None, true, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn metric_threshold_needs_n_samples_and_hysteresis_to_resolve() {
        let (eng, mut sub) = engine().await;
        eng.set_rules(vec![AlertRule {
            id: "cpu".into(),
            name: "CPU".into(),
            severity: AlertSeverity::Warning,
            enabled: true,
            project_id: None,
            kind: RuleKind::MetricThreshold { metric: "cpu".into(), node_id: None, above: 90.0, samples: 3 },
        }]);
        let node = NodeId::new();
        let feed = |v: f64| eng.on_metrics(node, "local", move |_| Some(v));
        assert!(feed(95.0).is_empty());
        assert!(feed(95.0).is_empty());
        for s in feed(96.0) {
            eng.apply(s).await.unwrap();
        }
        for s in feed(97.0) {
            eng.apply(s).await.unwrap();
        }
        assert_eq!(eng.open_alerts().await[0].count, 2);
        assert!(feed(10.0).is_empty());
        assert!(feed(99.0).is_empty(), "above again resets the below counter but needs 3 samples to re-raise");
        assert!(feed(10.0).is_empty());
        assert!(feed(10.0).is_empty());
        let res = feed(10.0);
        assert_eq!(res.len(), 1);
        for s in res {
            eng.apply(s).await.unwrap();
        }
        assert!(eng.open_alerts().await.is_empty());
        assert_eq!(kinds(&mut sub).await, ["AlertRaised", "AlertResolved"]);
    }

    #[tokio::test]
    async fn log_rate_and_pattern_rules() {
        let (eng, _sub) = engine().await;
        eng.set_rules(vec![AlertRule {
            id: "oom".into(),
            name: "OOM".into(),
            severity: AlertSeverity::Critical,
            enabled: true,
            project_id: None,
            kind: RuleKind::LogRate {
                source_id: None,
                min_level: None,
                pattern: Some("out of memory".into()),
                count: 2,
                window_secs: 60,
            },
        }]);
        let t0 = orkestr_domain::now();
        let line = |msg: &str, secs: i64| LogLine {
            ts: t0 + ChronoDuration::seconds(secs),
            source_id: "app".into(),
            level: Some(LogLevel::Error),
            message: msg.into(),
        };
        assert!(eng.on_log(&line("Out Of Memory", 0), None).is_empty());
        assert!(eng.on_log(&line("all fine", 1), None).is_empty());
        let s = eng.on_log(&line("out of memory again", 2), None);
        assert_eq!(s.len(), 1);
        for s in s {
            eng.apply(s).await.unwrap();
        }
        assert!(eng.tick(t0 + ChronoDuration::seconds(30)).is_empty());
        let r = eng.tick(t0 + ChronoDuration::seconds(120));
        assert_eq!(r.len(), 1);
        for s in r {
            eng.apply(s).await.unwrap();
        }
        assert!(eng.open_alerts().await.is_empty());
    }
}
