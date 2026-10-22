//! `monitor.*` — infrastructure monitor: local + per-node CPU/RAM/GPU/disk
//! series, HTTP/TCP health checks, log sources (files, deployment logs,
//! `monitor.ingest` webhook) and alerts (Phase 7, ADR-0042). **Owner: WP5**
//! (crate `orkestr-monitor`, handlers `core/src/rpc/monitor.rs`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AlertId, DeploymentId, EnvironmentId, HealthCheckId, NodeId, ProjectId, Timestamp};

use super::{method, Empty};
use crate::event::LogLevel;

/// Latest metrics of one node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeMetricsSnapshot {
    /// Node.
    pub node_id: NodeId,
    /// Display name.
    pub name: String,
    /// This core's own machine.
    pub local: bool,
    /// Reachable.
    pub online: bool,
    /// CPU usage 0..100.
    pub cpu_percent: f32,
    /// Used RAM bytes.
    pub mem_used_bytes: u64,
    /// Total RAM bytes.
    pub mem_total_bytes: u64,
    /// GPU utilisation 0..100 per GPU (empty when unknown).
    pub gpu_percent: Vec<f32>,
    /// Used disk bytes (all volumes).
    pub disk_used_bytes: u64,
    /// Total disk bytes.
    pub disk_total_bytes: u64,
    /// Sample time.
    pub sampled_at: Timestamp,
}

/// Health check probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum CheckProbe {
    /// HTTP(S) request.
    Http {
        /// URL.
        url: String,
        /// Method (`GET` default, `HEAD`, `POST`, ...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        method: Option<String>,
        /// Expected status (default: any 2xx/3xx).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        expect_status: Option<u16>,
        /// Body must contain this text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        contains: Option<String>,
    },
    /// TCP connect.
    Tcp {
        /// Host.
        host: String,
        /// Port.
        port: u16,
    },
}

/// Check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CheckState {
    /// Not run yet.
    Unknown,
    /// Passing.
    Up,
    /// Failing (after `failureThreshold` consecutive failures).
    Down,
}

/// A configured health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheck {
    /// Id.
    pub id: HealthCheckId,
    /// Project (`None` = global).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Environment it watches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub environment_id: Option<EnvironmentId>,
    /// Name.
    pub name: String,
    /// Probe.
    pub probe: CheckProbe,
    /// Interval.
    pub interval_secs: u32,
    /// Timeout.
    pub timeout_ms: u32,
    /// Consecutive failures before `Down` (and successes before `Up`).
    pub failure_threshold: u32,
    /// Enabled.
    pub enabled: bool,
}

/// A check with its current status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheckStatus {
    /// Check.
    pub check: HealthCheck,
    /// State.
    pub state: CheckState,
    /// State since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<Timestamp>,
    /// Last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_run: Option<Timestamp>,
    /// Last latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_latency_ms: Option<u32>,
    /// Last error (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
    /// Recent runs (oldest first, bounded).
    #[serde(default)]
    pub history: Vec<CheckRunSample>,
}

/// One health check run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckRunSample {
    /// Run time.
    pub ts: Timestamp,
    /// Probe succeeded.
    pub ok: bool,
    /// Latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latency_ms: Option<u32>,
    /// Error (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// `monitor.overview` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorOverviewParams {
    /// Project filter for checks / alerts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// `monitor.overview` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorOverview {
    /// Nodes (local first).
    pub nodes: Vec<NodeMetricsSnapshot>,
    /// Checks.
    pub checks: Vec<HealthCheckStatus>,
    /// Open (unresolved) alerts.
    pub open_alerts: u32,
    /// Log sources.
    pub log_sources: u32,
}

/// `monitor.metrics` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsParams {
    /// Node (default: local).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub node_id: Option<NodeId>,
    /// Metrics (`cpu`, `mem`, `gpu`, `disk`; default all).
    #[serde(default)]
    pub metrics: Vec<String>,
    /// Window start (default: last 15 min).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<Timestamp>,
    /// Bucket width in seconds; points inside a bucket are averaged
    /// (default: raw samples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolution_secs: Option<u32>,
}

/// One point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricPoint {
    /// Time.
    pub ts: Timestamp,
    /// Value.
    pub value: f64,
}

/// One series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricSeries {
    /// Node.
    pub node_id: NodeId,
    /// Metric (`cpu`, `mem`, `gpu0`, `disk`).
    pub metric: String,
    /// Unit (`percent`, `bytes`).
    pub unit: String,
    /// Points (oldest first).
    pub points: Vec<MetricPoint>,
}

/// `monitor.metrics` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsResult {
    /// Series.
    pub series: Vec<MetricSeries>,
}

/// Params filtering by project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorProjectParams {
    /// Project (`None` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// `monitor.checksList` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChecksListResult {
    /// Checks.
    pub checks: Vec<HealthCheckStatus>,
}

/// `monitor.checkUpsert` params (create when `id` is absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpsertCheckParams {
    /// Existing check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<HealthCheckId>,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub environment_id: Option<EnvironmentId>,
    /// Name.
    pub name: String,
    /// Probe.
    pub probe: CheckProbe,
    /// Interval (default 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub interval_secs: Option<u32>,
    /// Timeout (default 5000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timeout_ms: Option<u32>,
    /// Failure threshold (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failure_threshold: Option<u32>,
    /// Enabled (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
}

/// Params naming one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckIdParams {
    /// Check.
    pub check_id: HealthCheckId,
}

/// Result carrying one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheckResult {
    /// The check.
    pub check: HealthCheckStatus,
}

/// Kind of a log source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum LogSourceKind {
    /// A tailed file.
    File {
        /// Path.
        path: String,
    },
    /// Logs of a deployment.
    Deployment {
        /// Deployment.
        deployment_id: DeploymentId,
    },
    /// Lines pushed through `monitor.ingest`.
    Ingest {
        /// Ingest stream name.
        stream: String,
    },
}

/// A log source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogSourceInfo {
    /// Id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Kind.
    pub kind: LogSourceKind,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// `monitor.logSources` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogSourcesResult {
    /// Sources.
    pub sources: Vec<LogSourceInfo>,
}

/// One log line (redacted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogLine {
    /// Time (ingest time when the line has none).
    pub ts: Timestamp,
    /// Source.
    pub source_id: String,
    /// Level, if detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub level: Option<LogLevel>,
    /// Text.
    pub message: String,
}

/// `monitor.logs` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogQuery {
    /// Source (default: all of the project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Substring filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub contains: Option<String>,
    /// Minimum level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_level: Option<LogLevel>,
    /// Since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<Timestamp>,
    /// Max lines (newest last; default 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `monitor.logs` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogsResult {
    /// Lines.
    pub lines: Vec<LogLine>,
    /// More lines matched than returned.
    pub truncated: bool,
}

/// One ingested line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestLine {
    /// Time (default: now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ts: Option<Timestamp>,
    /// Level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub level: Option<LogLevel>,
    /// Text.
    pub message: String,
    /// Structured attributes (appended to the stored line as `key=value`;
    /// values of sensitive keys are redacted).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attrs: std::collections::BTreeMap<String, String>,
}

/// `monitor.ingest` params (webhook: local or token-authenticated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestParams {
    /// Stream name (creates an `Ingest` source).
    pub stream: String,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Lines (max 1000 per call).
    pub lines: Vec<IngestLine>,
}

/// `monitor.ingest` result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IngestResult {
    /// Lines accepted.
    pub accepted: u32,
}

/// Alert severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AlertSeverity {
    /// Info.
    Info,
    /// Warning.
    Warning,
    /// Critical.
    Critical,
}

/// Alert state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AlertState {
    /// Firing.
    Open,
    /// Acknowledged, still firing.
    Acknowledged,
    /// Condition cleared.
    Resolved,
}

/// What raised an alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AlertSource {
    /// A health check went down.
    Check {
        /// Check.
        check_id: HealthCheckId,
    },
    /// A metric crossed a threshold.
    Metric {
        /// Node.
        node_id: NodeId,
        /// Metric.
        metric: String,
    },
    /// Error lines in a log source.
    Log {
        /// Source.
        source_id: String,
    },
}

/// An alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Alert {
    /// Id.
    pub id: AlertId,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Rule id (dedup key with the source).
    pub rule: String,
    /// Severity.
    pub severity: AlertSeverity,
    /// Title.
    pub title: String,
    /// Message (redacted).
    pub message: String,
    /// Source.
    pub source: AlertSource,
    /// State.
    pub state: AlertState,
    /// Times raised while open (dedup counter).
    pub count: u32,
    /// Opened.
    pub opened_at: Timestamp,
    /// Acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub acked_at: Option<Timestamp>,
    /// Resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolved_at: Option<Timestamp>,
}

/// `monitor.alerts` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertsParams {
    /// Project (`None` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Include resolved alerts.
    #[serde(default)]
    pub include_resolved: bool,
    /// Max rows (newest first; default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `monitor.alerts` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertsResult {
    /// Alerts.
    pub alerts: Vec<Alert>,
}

/// `monitor.ackAlert` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertIdParams {
    /// Alert.
    pub alert_id: AlertId,
}

/// Result carrying one alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertResult {
    /// The alert.
    pub alert: Alert,
}

/// `HealthCheckChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheckChangedPayload {
    /// Check.
    pub check_id: HealthCheckId,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Previous state.
    pub from: CheckState,
    /// New state.
    pub to: CheckState,
    /// Error (redacted) when down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// `AlertRaised` / `AlertResolved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertPayload {
    /// The alert.
    pub alert: Alert,
}

/// What an alert rule watches (`monitor.rules`, ADR-0042).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AlertRuleKind {
    /// A check (or any check when `check_id` is absent) is down; resolved on `Up`.
    CheckDown {
        /// Check filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        check_id: Option<HealthCheckId>,
    },
    /// `metric > above` for `samples` consecutive samples (hysteresis on resolve).
    MetricThreshold {
        /// `cpu`, `mem`, `disk`, `gpuN` (percent).
        metric: String,
        /// Node filter (absent = every node).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        node_id: Option<NodeId>,
        /// Threshold in percent, `[0, 100)`.
        above: f64,
        /// Consecutive samples, `1..=1000`.
        samples: u32,
    },
    /// At least `count` matching lines within `window_secs`.
    LogRate {
        /// Source filter (absent = every source).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        source_id: Option<String>,
        /// Minimum level (lines without a level only match through `pattern`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        min_level: Option<LogLevel>,
        /// Case-insensitive substring (≤ 200 chars).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pattern: Option<String>,
        /// Lines within the window, `1..=100000`.
        count: u32,
        /// Window, `1..=86400` seconds.
        window_secs: u32,
    },
}

/// An alert rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertRule {
    /// Stable id, `1..64` of `[A-Za-z0-9._-]` (part of the dedup key).
    pub id: String,
    /// Title prefix (1..=120 chars).
    pub name: String,
    /// Severity.
    pub severity: AlertSeverity,
    /// Enabled.
    pub enabled: bool,
    /// Project scope (absent = any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Condition.
    pub kind: AlertRuleKind,
}

/// `monitor.rules` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertRulesResult {
    /// Rules (default rules are seeded once; deleting them is permanent).
    pub rules: Vec<AlertRule>,
}

/// `monitor.ruleUpsert` params (creates or replaces the rule with that id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpsertRuleParams {
    /// The rule.
    pub rule: AlertRule,
}

/// Result carrying one rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlertRuleResult {
    /// The rule as stored.
    pub rule: AlertRule,
}

/// `monitor.ruleDelete` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuleIdParams {
    /// Rule id.
    pub rule_id: String,
}

/// `monitor.sourceAdd` params: a tailed log file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddFileSourceParams {
    /// Display name (unique among sources; default: the path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Absolute path of an existing, readable regular file.
    pub path: String,
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result carrying one log source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogSourceResult {
    /// The source.
    pub source: LogSourceInfo,
}

/// `monitor.sourceRemove` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogSourceIdParams {
    /// Source id (only `File` sources can be removed).
    pub source_id: String,
}

method!(/// Nodes, checks and alert counts.
    Overview, "monitor.overview", MonitorOverviewParams, MonitorOverview);
method!(/// Metric series.
    Metrics, "monitor.metrics", MetricsParams, MetricsResult);
method!(/// Health checks with status.
    ChecksList, "monitor.checksList", MonitorProjectParams, ChecksListResult);
method!(/// Creates / updates a health check.
    CheckUpsert, "monitor.checkUpsert", UpsertCheckParams, HealthCheckResult);
method!(/// Deletes a health check.
    CheckDelete, "monitor.checkDelete", CheckIdParams, Empty);
method!(/// Runs a check now.
    CheckRun, "monitor.checkRun", CheckIdParams, HealthCheckResult);
method!(/// Log sources.
    LogSources, "monitor.logSources", MonitorProjectParams, LogSourcesResult);
method!(/// Queries log lines (redacted).
    Logs, "monitor.logs", LogQuery, LogsResult);
method!(/// Ingests log lines (webhook).
    Ingest, "monitor.ingest", IngestParams, IngestResult);
method!(/// Alerts.
    Alerts, "monitor.alerts", AlertsParams, AlertsResult);
method!(/// Acknowledges an alert.
    AckAlert, "monitor.ackAlert", AlertIdParams, AlertResult);
method!(/// Alert rules.
    Rules, "monitor.rules", Empty, AlertRulesResult);
method!(/// Creates / replaces an alert rule (validated).
    RuleUpsert, "monitor.ruleUpsert", UpsertRuleParams, AlertRuleResult);
method!(/// Deletes an alert rule (default rules are not re-seeded).
    RuleDelete, "monitor.ruleDelete", RuleIdParams, Empty);
method!(/// Adds a tailed log file source (local only).
    SourceAdd, "monitor.sourceAdd", AddFileSourceParams, LogSourceResult);
method!(/// Removes a file log source (local only).
    SourceRemove, "monitor.sourceRemove", LogSourceIdParams, Empty);
