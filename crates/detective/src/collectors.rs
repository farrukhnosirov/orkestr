//! Built-in collectors: alert, logs + metrics (monitor reader, WP5),
//! deployments (WP4), VCS commits + diff since the last good deploy, quality
//! failures, and code snippets from stack-trace `file:line` references.
//! All content is redacted and capped by the [`EvidenceBudget`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{now, DeploymentStatus};
use orkestr_protocol::event::LogLevel;
use orkestr_protocol::methods::detective::{Evidence, EvidenceKind, Incident, IncidentTrigger};
use orkestr_protocol::methods::monitor::{AlertSource, LogQuery};

use crate::evidence::{EvidenceBudget, EvidenceCollector};
use crate::ports::{DeploymentsReader, MonitorReader, QualityReader, VcsOps};
use crate::{stacktrace, DetectiveError};

/// Truncates `s` to at most `max` bytes (char boundary) with a marker.
#[must_use]
pub fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    const MARK: &str = "\n…[truncated]";
    let mut end = max.saturating_sub(MARK.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &s[..end])
}

/// Keeps the *end* of `s` (logs: the newest lines matter most).
#[must_use]
pub fn cap_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    const MARK: &str = "…[truncated]\n";
    let mut start = s.len() - max.saturating_sub(MARK.len());
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("{MARK}{}", &s[start..])
}

/// Builds one evidence item (redacted; id assigned by the service).
#[must_use]
pub fn item(kind: EvidenceKind, title: impl Into<String>, content: &str, reference: Option<String>) -> Evidence {
    Evidence {
        id: String::new(),
        kind,
        title: orkestr_security::redact::redact(&title.into()),
        content: orkestr_security::redact::redact(content),
        reference: reference.map(|r| orkestr_security::redact::redact(&r)),
        collected_at: now(),
    }
}

/// Applies the per-collector budget: at most `max_items`, at most
/// `max_bytes` of content in total (later items are cut first).
#[must_use]
pub fn fit(mut items: Vec<Evidence>, budget: &EvidenceBudget) -> Vec<Evidence> {
    items.truncate(budget.max_items);
    let mut left = budget.max_bytes;
    let mut out = Vec::with_capacity(items.len());
    for mut e in items {
        if left < 64 {
            break;
        }
        e.content = cap(&e.content, left);
        left = left.saturating_sub(e.content.len());
        out.push(e);
    }
    out
}

fn since(budget: &EvidenceBudget) -> orkestr_domain::Timestamp {
    now() - chrono::Duration::from_std(budget.window).unwrap_or_else(|_| chrono::Duration::hours(1))
}

/// The triggering alert and other open alerts of the project.
pub struct AlertCollector(pub Arc<dyn MonitorReader>);

#[async_trait]
impl EvidenceCollector for AlertCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::Alert
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let mut out = Vec::new();
        let trigger_alert = match &inc.trigger {
            IncidentTrigger::Alert { alert_id } => self.0.alert(*alert_id).await?,
            _ => None,
        };
        if let Some(a) = &trigger_alert {
            out.push(item(
                EvidenceKind::Alert,
                format!("Alert: {}", a.title),
                &format!(
                    "{:?} · rule {} · raised {}× since {}\n{}",
                    a.severity, a.rule, a.count, a.opened_at, a.message
                ),
                Some(a.id.to_string()),
            ));
        }
        for a in self.0.active_alerts(inc.project_id).await? {
            if trigger_alert.as_ref().is_some_and(|t| t.id == a.id) {
                continue;
            }
            out.push(item(
                EvidenceKind::Alert,
                format!("Open alert: {}", a.title),
                &format!("{:?} · rule {} · since {}\n{}", a.severity, a.rule, a.opened_at, a.message),
                Some(a.id.to_string()),
            ));
        }
        Ok(fit(out, budget))
    }
}

/// Log lines in the window (the trigger's source first).
pub struct LogsCollector(pub Arc<dyn MonitorReader>);

#[async_trait]
impl EvidenceCollector for LogsCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::Logs
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let mut sources: Vec<Option<String>> = Vec::new();
        match &inc.trigger {
            IncidentTrigger::LogError { source_id, .. } => sources.push(Some(source_id.clone())),
            IncidentTrigger::Alert { alert_id } => {
                if let Some(a) = self.0.alert(*alert_id).await? {
                    if let AlertSource::Log { source_id } = a.source {
                        sources.push(Some(source_id));
                    }
                }
            }
            _ => {}
        }
        sources.push(None);
        let mut out = Vec::new();
        for source in sources {
            let q = LogQuery {
                source_id: source.clone(),
                project_id: Some(inc.project_id),
                contains: None,
                min_level: if source.is_some() { None } else { Some(LogLevel::Warn) },
                since: Some(since(budget)),
                limit: Some(200),
            };
            let lines = self.0.logs(&q).await?;
            if lines.is_empty() {
                continue;
            }
            let text: String = lines
                .iter()
                .map(|l| {
                    let lvl = l.level.map(|v| format!("{v:?}")).unwrap_or_default();
                    format!("{} {lvl:<5} [{}] {}\n", l.ts.format("%H:%M:%S"), l.source_id, l.message)
                })
                .collect();
            let title = match &source {
                Some(s) => format!("Logs of {s} ({} lines)", lines.len()),
                None => format!("Warnings and errors ({} lines)", lines.len()),
            };
            out.push(item(EvidenceKind::Logs, title, &cap_tail(&text, budget.max_bytes / 2), source));
        }
        Ok(fit(out, budget))
    }
}

/// Metric summaries (min / avg / max / last) in the window.
pub struct MetricsCollector(pub Arc<dyn MonitorReader>);

#[async_trait]
impl EvidenceCollector for MetricsCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::Metrics
    }
    async fn collect(&self, _inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let series = self.0.metrics(since(budget)).await?;
        let mut text = String::new();
        for s in series.iter().filter(|s| !s.points.is_empty()) {
            let vals: Vec<f64> = s.points.iter().map(|p| p.value).collect();
            let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
            let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            #[allow(clippy::cast_precision_loss)]
            let avg = vals.iter().sum::<f64>() / vals.len() as f64;
            let last = vals.last().copied().unwrap_or_default();
            text.push_str(&format!(
                "{} {}: last {last:.1} · min {min:.1} · avg {avg:.1} · max {max:.1} {} ({} points)\n",
                s.node_id,
                s.metric,
                s.unit,
                vals.len()
            ));
        }
        if text.is_empty() {
            return Ok(Vec::new());
        }
        Ok(fit(vec![item(EvidenceKind::Metrics, "Metrics in the incident window", &text, None)], budget))
    }
}

/// Current / previous deployed versions and the current deployment's log tail.
pub struct DeploymentsCollector(pub Arc<dyn DeploymentsReader>);

#[async_trait]
impl EvidenceCollector for DeploymentsCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::Deployments
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let deps = self.0.recent(inc.project_id, 10).await?;
        if deps.is_empty() {
            return Ok(Vec::new());
        }
        let mut text = String::new();
        for (i, d) in deps.iter().take(5).enumerate() {
            let label = match i {
                0 => "current ",
                1 => "previous",
                _ => "older   ",
            };
            text.push_str(&format!(
                "{label} {:?} version {} env {} at {}{}\n",
                d.status,
                d.version.as_deref().unwrap_or("?"),
                d.environment_id,
                d.finished_at.or(d.started_at).unwrap_or(d.created_at),
                d.url.as_deref().map(|u| format!(" · {u}")).unwrap_or_default()
            ));
        }
        let current = &deps[0];
        let mut out = vec![item(EvidenceKind::Deployments, "Recent deployments", &text, Some(current.id.to_string()))];
        let lines = self.0.logs(current.id, 80).await.unwrap_or_default();
        if !lines.is_empty() {
            out.push(item(
                EvidenceKind::Deployments,
                format!("Deploy log of {}", current.version.as_deref().unwrap_or("current deployment")),
                &cap_tail(&lines.join("\n"), budget.max_bytes / 2),
                Some(current.id.to_string()),
            ));
        }
        Ok(fit(out, budget))
    }
}

/// Recent commits plus the diff since the last good deployment (or the last
/// commit when no deployment is known).
pub struct VcsCollector {
    /// VCS.
    pub vcs: Arc<dyn VcsOps>,
    /// Project root.
    pub root: PathBuf,
    /// Deployments (for the last good version).
    pub deployments: Option<Arc<dyn DeploymentsReader>>,
}

impl VcsCollector {
    async fn last_good(&self, inc: &Incident) -> Option<String> {
        let deps = self.deployments.as_ref()?.recent(inc.project_id, 20).await.ok()?;
        // The newest deployment is the suspect; the one before it that
        // succeeded is the last known good version.
        deps.iter()
            .skip(1)
            .find(|d| d.status == DeploymentStatus::Succeeded && d.version.is_some())
            .and_then(|d| d.version.clone())
    }
}

#[async_trait]
impl EvidenceCollector for VcsCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::Commits
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let mut out = Vec::new();
        let commits = self.vcs.recent_commits(&self.root, 10).await?;
        if !commits.is_empty() {
            let text: String = commits
                .iter()
                .map(|c| format!("{} {} — {} ({})\n", &c.sha[..c.sha.len().min(10)], c.subject, c.author, c.date))
                .collect();
            out.push(item(EvidenceKind::Commits, "Recent commits", &text, commits.first().map(|c| c.sha.clone())));
        }
        let good = self.last_good(inc).await;
        let good = match good {
            Some(v) => self.vcs.rev_parse(&self.root, &v).await.ok().flatten(),
            None => None,
        };
        let diff = self.vcs.diff_since(&self.root, good.as_deref(), budget.max_bytes / 2).await?;
        if !diff.trim().is_empty() {
            let title = match &good {
                Some(g) => format!("Diff since last good deploy {}", &g[..g.len().min(10)]),
                None => "Diff of the last commit".to_owned(),
            };
            out.push(item(EvidenceKind::Diff, title, &diff, good));
        }
        Ok(fit(out, budget))
    }
}

/// Failing checks of the triggering (or latest failed) quality run.
pub struct QualityCollector(pub Arc<dyn QualityReader>);

#[async_trait]
impl EvidenceCollector for QualityCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::QualityFailures
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let run_id = match &inc.trigger {
            IncidentTrigger::TestFailure { run_id } => Some(run_id.as_str()),
            _ => None,
        };
        let Some(f) = self.0.failure(inc.project_id, run_id).await? else { return Ok(Vec::new()) };
        let title = if f.failed_checks.is_empty() {
            "Last test run".to_owned()
        } else {
            format!("Failing checks: {}", f.failed_checks.join(", "))
        };
        Ok(fit(
            vec![item(EvidenceKind::QualityFailures, title, &cap_tail(&f.output, budget.max_bytes), Some(f.run_id))],
            budget,
        ))
    }
}

/// Parses stack traces out of the trigger and the evidence collected so far
/// (`inc.evidence`) and attaches code around each in-project `file:line`.
pub struct CodeSnippetCollector {
    /// Project root.
    pub root: PathBuf,
}

/// The text a stack trace may hide in: trigger + logs / quality / alert evidence.
#[must_use]
pub fn trace_corpus(inc: &Incident) -> String {
    let mut text = match &inc.trigger {
        IncidentTrigger::LogError { line, .. } => line.clone(),
        IncidentTrigger::Manual { description } => description.clone(),
        _ => String::new(),
    };
    for e in &inc.evidence {
        if matches!(
            e.kind,
            EvidenceKind::Logs | EvidenceKind::QualityFailures | EvidenceKind::Alert | EvidenceKind::Deployments
        ) {
            text.push('\n');
            text.push_str(&e.content);
        }
    }
    text
}

#[async_trait]
impl EvidenceCollector for CodeSnippetCollector {
    fn kind(&self) -> EvidenceKind {
        EvidenceKind::CodeSnippet
    }
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError> {
        let frames = stacktrace::parse(&trace_corpus(inc), 40);
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let trace: String = frames
            .iter()
            .map(|f| {
                format!(
                    "{}:{}{}{}\n",
                    f.file,
                    f.line,
                    f.column.map(|c| format!(":{c}")).unwrap_or_default(),
                    f.function.as_deref().map(|n| format!(" in {n}")).unwrap_or_default()
                )
            })
            .collect();
        out.push(item(EvidenceKind::StackTrace, format!("Stack trace ({} frames)", frames.len()), &trace, None));
        let root = self.root.clone();
        let per = (budget.max_bytes / budget.max_items.max(1)).clamp(512, 4096);
        let snippets = tokio::task::spawn_blocking(move || {
            let mut seen = Vec::new();
            let mut v = Vec::new();
            for f in frames {
                let Some(rel) = stacktrace::resolve(&root, &f.file) else { continue };
                if seen.contains(&(rel.clone(), f.line)) {
                    continue;
                }
                if let Some(code) = stacktrace::snippet(&root, &rel, f.line, 6, per) {
                    let r = format!("{}:{}", rel.to_string_lossy().replace('\\', "/"), f.line);
                    v.push((r, f.function.clone(), code));
                    seen.push((rel, f.line));
                }
            }
            v
        })
        .await
        .map_err(|e| DetectiveError::Internal(e.to_string()))?;
        for (r, func, code) in snippets {
            let title = match func {
                Some(n) => format!("{r} ({n})"),
                None => r.clone(),
            };
            out.push(item(EvidenceKind::CodeSnippet, title, &code, Some(r)));
        }
        Ok(fit(out, budget))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_respect_char_boundaries_and_budget() {
        let s = "ж".repeat(100);
        let c = cap(&s, 51);
        assert!(c.len() <= 51 && c.ends_with("[truncated]"));
        let t = cap_tail(&s, 51);
        assert!(t.len() <= 51 && t.starts_with("…[truncated]"));
        let items: Vec<Evidence> =
            (0..10).map(|i| item(EvidenceKind::Logs, format!("t{i}"), &"x".repeat(500), None)).collect();
        let b = EvidenceBudget { max_items: 4, max_bytes: 1200, window: std::time::Duration::from_secs(60) };
        let f = fit(items, &b);
        assert!(f.len() <= 4);
        assert!(f.iter().map(|e| e.content.len()).sum::<usize>() <= 1200);
    }

    #[test]
    fn evidence_is_redacted() {
        let e = item(EvidenceKind::Logs, "x", "token=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", None);
        assert!(!e.content.contains("AAAAAAAAAAAAAAAAAAAA"), "{}", e.content);
    }
}
