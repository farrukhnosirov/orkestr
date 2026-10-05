//! `dashboard.*` — aggregated read models for Mission Control and the
//! Monitoring panel (§27 local part, §32, §68.22, §68.45 lite).
//!
//! `dashboard.snapshot` folds projects, agents, tasks, approvals, spend,
//! provider health and runs into one compact answer so the Mission Control
//! screen (desktop, and later Web/Mobile) needs a single round trip; live
//! deltas still arrive as ordinary events. `dashboard.health` reports the
//! core's own services (storage sizes, event log position) and the
//! processes Orkestr spawned (terminals, CLI agents) with CPU/RAM.
//!
//! Both are read-only and never expose secrets or file contents.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AuthState, ProjectId, ProviderHealth, ProviderId, Timestamp};

use super::method;
use super::nodes::ProcessUsage;

/// Coarse activity of a project in Mission Control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProjectActivity {
    /// Nothing running.
    Idle,
    /// Agents or tasks are running.
    Active,
    /// Something needs the user: failed tasks or pending approvals.
    Attention,
}

/// One project row of the snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectOverview {
    /// Project.
    pub project_id: ProjectId,
    /// Display name.
    pub name: String,
    /// Accent colour chosen by the user (`#rrggbb`), if any.
    pub color: Option<String>,
    /// Derived activity.
    pub activity: ProjectActivity,
    /// Agents whose run is still going.
    pub active_agents: u32,
    /// All agent instances of the project.
    pub total_agents: u32,
    /// Tasks in Running / Review / Testing.
    pub running_tasks: u32,
    /// Tasks not yet terminal (anything but Completed / Cancelled).
    pub open_tasks: u32,
    /// Failed or interrupted tasks.
    pub failed_tasks: u32,
    /// Completed tasks.
    pub completed_tasks: u32,
    /// Pending approvals scoped to the project.
    pub pending_approvals: u32,
    /// Live Manager runs.
    pub live_runs: u32,
    /// Spend today (USD, incl. estimates).
    pub spend_today_usd: f64,
    /// Spend this month (USD, incl. estimates).
    pub spend_month_usd: f64,
}

/// Totals across all projects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardTotals {
    /// Non-archived projects.
    pub projects: u32,
    /// Agents whose run is still going.
    pub active_agents: u32,
    /// All agent instances.
    pub total_agents: u32,
    /// Tasks in Running / Review / Testing.
    pub running_tasks: u32,
    /// Failed or interrupted tasks.
    pub failed_tasks: u32,
    /// Pending approvals (all scopes).
    pub pending_approvals: u32,
    /// Live Manager runs.
    pub live_runs: u32,
}

/// Spend overview (global).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpendOverview {
    /// Spend today (USD, incl. estimates).
    pub today_usd: f64,
    /// Part of `today_usd` that is estimated.
    pub today_estimated_usd: f64,
    /// Spend this month (USD, incl. estimates).
    pub month_usd: f64,
    /// Part of `month_usd` that is estimated.
    pub month_estimated_usd: f64,
    /// Tokens (input + output) today.
    pub today_tokens: u64,
    /// Highest used fraction among applicable limited budgets.
    pub budget_used: Option<f64>,
}

/// Provider health row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderHealthRow {
    /// Provider.
    pub provider_id: ProviderId,
    /// Display name.
    pub name: String,
    /// Adapter family as its wire string (`openai`, `claude_code`, …).
    pub kind: String,
    /// Enabled by the user.
    pub enabled: bool,
    /// Local provider (no network).
    pub is_local: bool,
    /// Health.
    pub health: ProviderHealth,
    /// Auth state.
    pub auth_state: AuthState,
    /// Last probe latency.
    pub latency_ms: Option<u64>,
    /// Moving average latency.
    pub avg_latency_ms: Option<u64>,
    /// Rate-limited until.
    pub rate_limited_until: Option<Timestamp>,
    /// Quota exhausted until.
    pub quota_exhausted_until: Option<Timestamp>,
    /// Paused after repeated failures.
    pub circuit_open: bool,
}

/// Params of `dashboard.snapshot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardSnapshotParams {
    /// Include archived projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_archived: Option<bool>,
}

/// Result of `dashboard.snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardSnapshot {
    /// When the snapshot was taken.
    pub generated_at: Timestamp,
    /// Core uptime.
    pub uptime_ms: u64,
    /// Core version.
    pub core_version: String,
    /// Projects, most active first.
    pub projects: Vec<ProjectOverview>,
    /// Totals.
    pub totals: DashboardTotals,
    /// Spend.
    pub spend: SpendOverview,
    /// Provider health.
    pub providers: Vec<ProviderHealthRow>,
    /// Emergency stop is engaged.
    pub emergency_stopped: bool,
    /// How long the aggregation took.
    pub took_ms: u64,
}

/// Kind of an Orkestr-spawned process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum OrkestrProcessKind {
    /// The core itself (embedded in the desktop app or `orkestrd`).
    Core,
    /// A terminal session started by the user.
    Terminal,
    /// A terminal session owned by an agent.
    AgentTerminal,
    /// Another child process of the core (CLI coding agents, git, tools).
    Child,
}

/// A process spawned by Orkestr, with its resource usage (subtree included).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OrkestrProcess {
    /// Kind.
    pub kind: OrkestrProcessKind,
    /// Stable id: terminal id, or `pid:<n>`.
    pub id: String,
    /// Label (terminal title or executable name).
    pub label: String,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Owning agent (agent terminals).
    pub agent_id: Option<orkestr_domain::AgentId>,
    /// Pid, when the process is alive.
    pub pid: Option<u32>,
    /// Still running.
    pub running: bool,
    /// CPU of the process and its descendants, 0..=100 of the whole machine.
    pub cpu_percent: Option<f32>,
    /// Resident memory of the process and its descendants.
    pub memory_bytes: Option<u64>,
    /// Number of processes in the subtree (incl. itself).
    pub process_count: u32,
}

/// Storage sizes of the core.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StorageHealth {
    /// SQLite main file size.
    pub db_bytes: u64,
    /// SQLite WAL size.
    pub db_wal_bytes: u64,
    /// Object store size on disk.
    pub object_store_bytes: u64,
    /// Objects counted.
    pub object_count: u64,
    /// The object walk stopped early (very large store); sizes are lower bounds.
    pub object_scan_truncated: bool,
}

/// Result of `dashboard.health`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CoreHealthResult {
    /// Core version.
    pub core_version: String,
    /// Core pid.
    pub pid: u32,
    /// Core start.
    pub started_at: Timestamp,
    /// Core uptime.
    pub uptime_ms: u64,
    /// Highest event sequence number published.
    pub last_event_seq: u64,
    /// RPC methods served.
    pub rpc_methods: u32,
    /// Storage.
    pub storage: StorageHealth,
    /// Core process usage.
    pub core_process: Option<ProcessUsage>,
    /// Processes spawned by Orkestr.
    pub processes: Vec<OrkestrProcess>,
    /// How long the probe took.
    pub took_ms: u64,
}

method!(
    /// `dashboard.snapshot`.
    Snapshot, "dashboard.snapshot", DashboardSnapshotParams, DashboardSnapshot
);
method!(
    /// `dashboard.health`.
    Health, "dashboard.health", super::Empty, CoreHealthResult
);
