//! `cost.*`, `budgets.*` — Cost & Limit Center (§22).
//!
//! Handlers live in `crates/core/src/rpc/cost.rs`; the engine is
//! `orkestr-cost`. All money values are USD (`f64`); costs of subscription /
//! CLI usage are *estimated* from list prices and flagged as such.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, BudgetPeriod, ProjectId, ProviderId, TaskId, Timestamp, UsageRecord};

use super::method;

/// Who a budget applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum BudgetScope {
    /// All usage.
    Global,
    /// One project.
    Project {
        /// Project.
        project_id: ProjectId,
    },
    /// One task.
    Task {
        /// Task.
        task_id: TaskId,
    },
    /// One provider instance (e.g. "Ollama unrestricted").
    Provider {
        /// Provider.
        provider_id: ProviderId,
    },
    /// One agent instance.
    Agent {
        /// Agent.
        agent_id: AgentId,
    },
}

/// A configured budget (§22 Smart Budgeting).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetRule {
    /// Id (UUID v7 string).
    pub id: String,
    /// Optional display name.
    pub name: Option<String>,
    /// Scope.
    pub scope: BudgetScope,
    /// Period (`PerTask` = per individual task inside the scope).
    pub period: BudgetPeriod,
    /// Limit in USD; `None` only when `unrestricted`.
    pub limit_usd: Option<f64>,
    /// Explicitly unrestricted: exempts matching usage from broader-scope limits.
    pub unrestricted: bool,
    /// Fraction (0..1) at which a warning is raised (default 0.8).
    pub warn_at: f64,
    /// Disabled budgets are ignored.
    pub enabled: bool,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
}

/// Evaluation level of a budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum BudgetLevel {
    /// Below the warning threshold.
    Ok,
    /// At or above `warnAt`.
    Warn,
    /// At or above 100 %.
    Exceeded,
    /// No limit.
    Unrestricted,
    /// Disabled.
    Disabled,
}

/// A budget with its current spend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetStatus {
    /// The budget.
    pub budget: BudgetRule,
    /// Spent in the current period (for `PerTask`: the most expensive task).
    pub spent_usd: f64,
    /// spent / limit (absent when unrestricted).
    pub fraction: Option<f64>,
    /// Level.
    pub level: BudgetLevel,
    /// Current period key (`2026-10-02`, `2026-10`, `total`, `task:<id>`).
    pub period_key: String,
}

/// Token/cost totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsageTotals {
    /// Cost in USD (reported + estimated).
    pub cost_usd: f64,
    /// Part of `costUsd` that is estimated.
    pub estimated_cost_usd: f64,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Number of calls.
    pub requests: u64,
    /// Failed calls.
    pub failures: u64,
}

/// Params of `cost.summary`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostSummaryParams {
    /// Restrict to one project (absent ⇒ all usage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `cost.summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostSummaryResult {
    /// Today (core-local calendar day).
    pub today: UsageTotals,
    /// This calendar month.
    pub month: UsageTotals,
    /// All time.
    pub total: UsageTotals,
    /// Highest used fraction among enabled limited daily/monthly/total budgets that apply.
    pub budget_used: Option<f64>,
    /// Budget behind `budgetUsed`.
    pub budget_id: Option<String>,
    /// Current day key (`YYYY-MM-DD`).
    pub day: String,
    /// Current month key (`YYYY-MM`).
    pub month_key: String,
}

/// Breakdown dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CostDimension {
    /// By provider instance.
    Provider,
    /// By model (`providerId` + model).
    Model,
    /// By agent.
    Agent,
    /// By project.
    Project,
    /// By task.
    Task,
}

/// Time window of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CostWindow {
    /// Today.
    Today,
    /// This month.
    Month,
    /// Last 30 days incl. today.
    Last30Days,
    /// Everything.
    All,
}

/// Params of `cost.byDimension`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostByDimensionParams {
    /// Dimension.
    pub dimension: CostDimension,
    /// Window (default Month).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub window: Option<CostWindow>,
    /// Restrict to one project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Max rows (default 50, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// One breakdown row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostBreakdownRow {
    /// Group key (id, `providerId/model`, or `none` for unattributed usage).
    pub key: String,
    /// Model name for the Model dimension.
    pub model: Option<String>,
    /// Provider for the Provider/Model dimensions.
    pub provider_id: Option<ProviderId>,
    /// Totals.
    pub totals: UsageTotals,
    /// Average latency (ms) of calls that reported one.
    pub avg_latency_ms: Option<f64>,
}

/// Result of `cost.byDimension`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostByDimensionResult {
    /// Rows, most expensive first.
    pub rows: Vec<CostBreakdownRow>,
}

/// Time-series granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CostGranularity {
    /// Per day.
    Day,
    /// Per month.
    Month,
}

/// Params of `cost.timeseries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostTimeseriesParams {
    /// Granularity.
    pub granularity: CostGranularity,
    /// Number of buckets ending with the current one (default 30 days / 12 months, max 366).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub buckets: Option<u32>,
    /// Restrict to one project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// One bucket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostPoint {
    /// Bucket key (`YYYY-MM-DD` or `YYYY-MM`).
    pub bucket: String,
    /// Totals (zero-filled when no usage).
    pub totals: UsageTotals,
}

/// Result of `cost.timeseries`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostTimeseriesResult {
    /// Buckets, oldest first.
    pub points: Vec<CostPoint>,
}

/// Params of `cost.records`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostRecordsParams {
    /// Filter: project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Filter: task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Filter: provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_id: Option<ProviderId>,
    /// Filter: agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Only records strictly older than this time (paging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before: Option<Timestamp>,
    /// Max records (default 100, max 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `cost.records`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostRecordsResult {
    /// Records, newest first.
    pub records: Vec<UsageRecord>,
}

/// Params of `budgets.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListBudgetsParams {
    /// Only budgets relevant to this project (its own + global + its tasks are not known here: project + global).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `budgets.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetListResult {
    /// Budgets with their current spend.
    pub budgets: Vec<BudgetStatus>,
}

/// Params of `budgets.set` (create when `id` is absent, else replace).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetBudgetParams {
    /// Existing budget to replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Scope.
    pub scope: BudgetScope,
    /// Period.
    pub period: BudgetPeriod,
    /// Limit in USD (required unless `unrestricted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit_usd: Option<f64>,
    /// Explicitly unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unrestricted: Option<bool>,
    /// Warning fraction (0 < warnAt < 1, default 0.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warn_at: Option<f64>,
    /// Enabled (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
}

/// Result of `budgets.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetResult {
    /// The stored budget with its spend.
    pub budget: BudgetStatus,
}

/// Params of `budgets.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetIdParams {
    /// Budget id.
    pub id: String,
}

/// Result of `budgets.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteBudgetResult {
    /// Deleted id.
    pub id: String,
}

/// Params of `budgets.check`: would a call with this estimated cost fit?
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckBudgetParams {
    /// Project of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Task of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Provider of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_id: Option<ProviderId>,
    /// Agent of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Estimated cost of the call in USD (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub estimated_cost_usd: Option<f64>,
}

/// Overall outcome of a budget check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum BudgetOutcome {
    /// Within every applicable budget.
    Allow,
    /// Allowed, but at least one budget is at/above its warning threshold.
    Warn,
    /// At least one budget would be exceeded.
    Exceeded,
}

/// Evaluation of one applicable budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BudgetCheckEntry {
    /// Budget id.
    pub budget_id: String,
    /// Scope.
    pub scope: BudgetScope,
    /// Period.
    pub period: BudgetPeriod,
    /// Spent so far in the period.
    pub spent_usd: f64,
    /// Spent + estimate.
    pub projected_usd: f64,
    /// Limit (absent when unrestricted).
    pub limit_usd: Option<f64>,
    /// projected / limit.
    pub fraction: Option<f64>,
    /// Outcome for this budget.
    pub outcome: BudgetOutcome,
    /// True when an unrestricted narrower budget exempts this one.
    pub exempt: bool,
}

/// Result of `budgets.check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckBudgetResult {
    /// Worst outcome.
    pub outcome: BudgetOutcome,
    /// Highest projected fraction among non-exempt limited budgets.
    pub threshold: Option<f64>,
    /// Every applicable budget.
    pub entries: Vec<BudgetCheckEntry>,
}

method!(
    /// `cost.summary` — today / month / total + budget usage.
    Summary, "cost.summary", CostSummaryParams, CostSummaryResult
);
method!(
    /// `cost.byDimension` — breakdown by provider/model/agent/project/task.
    ByDimension, "cost.byDimension", CostByDimensionParams, CostByDimensionResult
);
method!(
    /// `cost.timeseries` — zero-filled daily/monthly spend.
    Timeseries, "cost.timeseries", CostTimeseriesParams, CostTimeseriesResult
);
method!(
    /// `cost.records` — raw usage records, newest first.
    Records, "cost.records", CostRecordsParams, CostRecordsResult
);
method!(
    /// `budgets.list`.
    BudgetsList, "budgets.list", ListBudgetsParams, BudgetListResult
);
method!(
    /// `budgets.set` — create or replace a budget.
    BudgetsSet, "budgets.set", SetBudgetParams, BudgetResult
);
method!(
    /// `budgets.delete`.
    BudgetsDelete, "budgets.delete", BudgetIdParams, DeleteBudgetResult
);
method!(
    /// `budgets.check` — Allow / Warn / Exceeded for an estimated call.
    BudgetsCheck, "budgets.check", CheckBudgetParams, CheckBudgetResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_json_shape() {
        let id = ProviderId::new();
        let v = serde_json::to_value(BudgetScope::Provider { provider_id: id }).unwrap();
        assert_eq!(v["scope"], "Provider");
        assert_eq!(v["providerId"], id.to_string());
        let g: BudgetScope = serde_json::from_str(r#"{"scope":"Global"}"#).unwrap();
        assert_eq!(g, BudgetScope::Global);
    }
}
