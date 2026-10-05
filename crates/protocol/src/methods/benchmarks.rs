//! `benchmarks.*` — model benchmarks and agent performance analytics (§22,
//! §68.38–39).
//!
//! Every agent run is an *execution record* collected from events (spawn,
//! stop, task transitions, reviews, builds, tests) and joined with usage
//! records (tokens, cost, latency). Quality signals are never fabricated:
//! rates are `null` until there is data behind them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, ProviderId, TaskId, Timestamp};

use super::method;

/// Outcome of an execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ExecutionOutcome {
    /// Still running.
    Running,
    /// Finished its work.
    Succeeded,
    /// Failed (error / limit).
    Failed,
    /// Stopped by the user / emergency stop / shutdown.
    Cancelled,
}

/// One execution (agent run) with its quality signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BenchmarkRecord {
    /// Agent instance (the execution key).
    pub agent_id: AgentId,
    /// Agent display name.
    pub agent_name: String,
    /// Template id.
    pub template_id: Option<String>,
    /// Role / task kind (`Backend`, `Reviewer`, …).
    pub role: String,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Provider of the (last) model.
    pub provider_id: Option<ProviderId>,
    /// Model.
    pub model: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub ended_at: Option<Timestamp>,
    /// Outcome.
    pub outcome: ExecutionOutcome,
    /// Why it stopped (`Completed`, `Error`, …).
    pub stop_reason: Option<String>,
    /// Final status of the task (`Completed`, `Failed`, …) when terminal.
    pub task_status: Option<String>,
    /// Completed without changes requested / fix loops (`null` until the task is terminal).
    pub first_pass: Option<bool>,
    /// Successful builds attributed.
    pub builds_ok: u32,
    /// Failed builds attributed.
    pub builds_failed: u32,
    /// Failing tests attributed.
    pub tests_failed: u32,
    /// Reviews that requested changes / rejected.
    pub review_rejections: u32,
    /// Review / test fix loops (Review→Running, Testing→Running).
    pub fix_loops: u32,
    /// Task retries (Failed→Ready / Recovering).
    pub retries: u32,
    /// Defects found (review findings + failing tests).
    pub defects_found: u32,
    /// Bug tasks linked to this (completed) task.
    pub post_merge_bugs: u32,
    /// Model calls.
    pub calls: u64,
    /// Failed calls.
    pub failed_calls: u64,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cost (USD).
    pub cost_usd: f64,
    /// Average call latency (ms).
    pub latency_avg_ms: Option<f64>,
    /// Wall-clock duration (ms) when ended.
    pub duration_ms: Option<u64>,
}

/// Per-model benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelBenchmarkRow {
    /// Provider.
    pub provider_id: ProviderId,
    /// Model.
    pub model: String,
    /// Roles / task kinds it ran (sorted).
    pub roles: Vec<String>,
    /// Executions.
    pub executions: u64,
    /// Executions whose task reached a terminal state.
    pub finished: u64,
    /// Model calls (usage records).
    pub calls: u64,
    /// Successful calls / calls.
    pub call_success_rate: Option<f64>,
    /// First-pass success rate of finished executions.
    pub first_pass_success_rate: Option<f64>,
    /// Successful builds / builds.
    pub build_success_rate: Option<f64>,
    /// Average defects found per finished execution.
    pub defects_found_avg: Option<f64>,
    /// Average retries per execution.
    pub retries_avg: Option<f64>,
    /// Average call latency (ms).
    pub latency_avg_ms: Option<f64>,
    /// Median call latency.
    pub latency_p50_ms: Option<u64>,
    /// 95th percentile call latency.
    pub latency_p95_ms: Option<u64>,
    /// Average execution duration (ms).
    pub duration_avg_ms: Option<f64>,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Average tokens per execution.
    pub tokens_avg: Option<f64>,
    /// Total cost (USD).
    pub cost_total_usd: f64,
    /// Average cost per execution (USD).
    pub cost_avg_usd: Option<f64>,
    /// Historical routing score 0..1 used by the auto router (`null` below the sample minimum).
    pub routing_score: Option<f64>,
}

/// Grouping of agent analytics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentGroupBy {
    /// Per template.
    #[default]
    Template,
    /// Per agent instance.
    Instance,
}

/// Agent performance (per template or instance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentPerformanceRow {
    /// Group key (template id, `name:<label>` or agent id).
    pub key: String,
    /// Display name (template label or agent name).
    pub name: String,
    /// Role.
    pub role: String,
    /// Template id.
    pub template_id: Option<String>,
    /// Agent id (instance grouping).
    pub agent_id: Option<AgentId>,
    /// Executions.
    pub executions: u64,
    /// Succeeded executions.
    pub completed: u64,
    /// Failed executions.
    pub failed: u64,
    /// Cancelled executions.
    pub cancelled: u64,
    /// Running executions.
    pub running: u64,
    /// First-pass success rate of finished executions.
    pub first_pass_success_rate: Option<f64>,
    /// Executions with ≥ 1 review rejection / reviewed executions.
    pub review_rejection_rate: Option<f64>,
    /// Review rejections.
    pub review_rejections: u64,
    /// Bug tasks linked to completed tasks.
    pub post_merge_bugs: u64,
    /// Retries.
    pub retries: u64,
    /// Average duration (ms).
    pub duration_avg_ms: Option<f64>,
    /// Total cost (USD).
    pub cost_usd: f64,
    /// Total tokens.
    pub tokens: u64,
    /// Models used (`provider/model` ids are not shown; model names, sorted).
    pub models: Vec<String>,
}

/// Params of `benchmarks.models`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelBenchmarksParams {
    /// Restrict to a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Restrict to a role / task kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub role: Option<String>,
    /// Window in days (default 30, max 365).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub days: Option<u32>,
}

/// Result of `benchmarks.models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelBenchmarksResult {
    /// Rows, most executions first.
    pub rows: Vec<ModelBenchmarkRow>,
    /// Window used.
    pub days: u32,
}

/// Params of `benchmarks.agents`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentPerformanceParams {
    /// Restrict to a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Grouping (default Template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub group_by: Option<AgentGroupBy>,
    /// Window in days (default 30, max 365).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub days: Option<u32>,
}

/// Result of `benchmarks.agents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentPerformanceResult {
    /// Rows, most executions first.
    pub rows: Vec<AgentPerformanceRow>,
    /// Window used.
    pub days: u32,
}

/// Params of `benchmarks.records`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BenchmarkRecordsParams {
    /// Filter: project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Filter: task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Filter: agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Filter: provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_id: Option<ProviderId>,
    /// Filter: model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// Max records (default 100, max 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `benchmarks.records`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BenchmarkRecordsResult {
    /// Records, newest first.
    pub records: Vec<BenchmarkRecord>,
}

method!(
    /// `benchmarks.models` — per-model quality / latency / cost table.
    Models, "benchmarks.models", ModelBenchmarksParams, ModelBenchmarksResult
);
method!(
    /// `benchmarks.agents` — agent performance per template or instance.
    Agents, "benchmarks.agents", AgentPerformanceParams, AgentPerformanceResult
);
method!(
    /// `benchmarks.records` — raw execution records, newest first.
    Records, "benchmarks.records", BenchmarkRecordsParams, BenchmarkRecordsResult
);
