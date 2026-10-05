//! `scheduler.*` — the global parallel-execution scheduler (§11, §12, §38,
//! §68.24–26).
//!
//! One admission controller in the core decides which ready tasks of every
//! live Manager run (all projects) may start an agent now: global, per-project
//! and per-team caps, provider/model concurrency, provider rate-limit back-off,
//! host CPU/RAM/disk/GPU thresholds and file-conflict avoidance, with a
//! priority queue + aging and fair share across projects. Runs keep going
//! regardless of which project the UI shows (§12); after a core restart
//! interrupted work resumes automatically when the policy allows.
//!
//! Events: `SchedulerUpdated` (topic `global`) when the running / queued /
//! waiting picture changes; `MergeRiskDetected` (topic `project:<id>`) when two
//! agents in separate worktrees are predicted to conflict on merge.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ManagerRunId, ProjectId, TaskId, TaskPriority, TaskStatus, Timestamp};

use super::method;

/// Upper bound of any parallel cap.
pub const MAX_PARALLEL_CAP: u32 = 256;

/// What happens to interrupted work after a core restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AutoResumePolicy {
    /// Never resume automatically — wait for the user.
    Off,
    /// Resume unless the run's approval policy is `Ask` (then wait for the user).
    #[default]
    FollowApproval,
    /// Always resume automatically.
    Always,
}

/// Per-project cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectCap {
    /// Project.
    pub project_id: ProjectId,
    /// Max running agents.
    pub max: u32,
}

/// Per-team cap (tasks tagged `team:<name>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamCap {
    /// Team name.
    pub team: String,
    /// Max running agents.
    pub max: u32,
}

/// Provider concurrency override (default: the adapter's `maxConcurrency`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderCap {
    /// Provider id.
    pub provider_id: String,
    /// Max agents using it at once (= max in-flight requests).
    pub max: u32,
}

/// Model concurrency cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelCap {
    /// Provider id.
    pub provider_id: String,
    /// Model id.
    pub model: String,
    /// Max agents / in-flight requests on this model.
    pub max: u32,
}

fn d_true() -> bool {
    true
}
fn d_global() -> u32 {
    8
}
fn d_project() -> u32 {
    4
}
fn d_cpu() -> f32 {
    85.0
}
fn d_ram() -> u64 {
    2048
}
fn d_disk() -> u64 {
    1024
}
fn d_aging() -> u32 {
    20
}
fn d_backoff() -> u32 {
    30
}
fn d_resume_attempts() -> u32 {
    3
}

/// Scheduler settings (`scheduler.setLimits`; persisted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerSettings {
    /// Max agents running at once across all projects.
    #[serde(default = "d_global")]
    pub max_parallel_global: u32,
    /// Default max agents per project.
    #[serde(default = "d_project")]
    pub max_parallel_per_project: u32,
    /// Per-project overrides.
    #[serde(default)]
    pub project_caps: Vec<ProjectCap>,
    /// Per-team caps.
    #[serde(default)]
    pub team_caps: Vec<TeamCap>,
    /// Provider concurrency overrides.
    #[serde(default)]
    pub provider_caps: Vec<ProviderCap>,
    /// Model concurrency caps.
    #[serde(default)]
    pub model_caps: Vec<ModelCap>,
    /// No new agents above this projected CPU usage (percent).
    #[serde(default = "d_cpu")]
    pub max_cpu_percent: f32,
    /// RAM that must stay free (MiB).
    #[serde(default = "d_ram")]
    pub min_free_ram_mb: u64,
    /// Disk space that must stay free on the data volume (MiB).
    #[serde(default = "d_disk")]
    pub min_free_disk_mb: u64,
    /// No new agents while the GPU is busier than this (percent; unset = ignore).
    #[serde(default)]
    pub max_gpu_percent: Option<f32>,
    /// Minutes of waiting per priority level gained (0 = no aging).
    #[serde(default = "d_aging")]
    pub aging_minutes: u32,
    /// Fair share across projects among equally urgent tasks.
    #[serde(default = "d_true")]
    pub fair_share: bool,
    /// Initial back-off after a rate limit without `Retry-After` (seconds; doubles on repeats, max 10 min).
    #[serde(default = "d_backoff")]
    pub rate_limit_backoff_secs: u32,
    /// Resume interrupted work after a restart.
    #[serde(default)]
    pub auto_resume: AutoResumePolicy,
    /// Max automatic resumes of one task before it waits for the user.
    #[serde(default = "d_resume_attempts")]
    pub max_auto_resume_attempts: u32,
    /// Predict merge conflicts between agents in separate worktrees (`git merge-tree`).
    #[serde(default = "d_true")]
    pub merge_risk_checks: bool,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            max_parallel_global: d_global(),
            max_parallel_per_project: d_project(),
            project_caps: Vec::new(),
            team_caps: Vec::new(),
            provider_caps: Vec::new(),
            model_caps: Vec::new(),
            max_cpu_percent: d_cpu(),
            min_free_ram_mb: d_ram(),
            min_free_disk_mb: d_disk(),
            max_gpu_percent: None,
            aging_minutes: d_aging(),
            fair_share: true,
            rate_limit_backoff_secs: d_backoff(),
            auto_resume: AutoResumePolicy::default(),
            max_auto_resume_attempts: d_resume_attempts(),
            merge_risk_checks: true,
        }
    }
}

impl SchedulerSettings {
    /// Validates ranges; returns a human message for the first problem.
    pub fn validate(&self) -> Result<(), String> {
        let cap = |name: &str, v: u32| {
            if (1..=MAX_PARALLEL_CAP).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} must be 1..={MAX_PARALLEL_CAP}"))
            }
        };
        cap("maxParallelGlobal", self.max_parallel_global)?;
        cap("maxParallelPerProject", self.max_parallel_per_project)?;
        for c in &self.project_caps {
            cap("projectCaps.max", c.max)?;
        }
        for c in &self.team_caps {
            cap("teamCaps.max", c.max)?;
            if c.team.trim().is_empty() || c.team.len() > 64 {
                return Err("teamCaps.team must be 1..=64 characters".into());
            }
        }
        for c in &self.provider_caps {
            cap("providerCaps.max", c.max)?;
            if c.provider_id.trim().is_empty() {
                return Err("providerCaps.providerId must not be empty".into());
            }
        }
        for c in &self.model_caps {
            cap("modelCaps.max", c.max)?;
            if c.provider_id.trim().is_empty() || c.model.trim().is_empty() {
                return Err("modelCaps needs providerId and model".into());
            }
        }
        if !(self.max_cpu_percent.is_finite() && (5.0..=100.0).contains(&self.max_cpu_percent)) {
            return Err("maxCpuPercent must be 5..=100".into());
        }
        if self.max_gpu_percent.is_some_and(|g| !(g.is_finite() && (5.0..=100.0).contains(&g))) {
            return Err("maxGpuPercent must be 5..=100".into());
        }
        if self.min_free_ram_mb > 1_048_576 || self.min_free_disk_mb > 10_485_760 {
            return Err("memory/disk reserves are out of range".into());
        }
        if self.aging_minutes > 24 * 60 {
            return Err("agingMinutes must be 0..=1440".into());
        }
        if !(1..=3600).contains(&self.rate_limit_backoff_secs) {
            return Err("rateLimitBackoffSecs must be 1..=3600".into());
        }
        if self.max_auto_resume_attempts > 20 {
            return Err("maxAutoResumeAttempts must be 0..=20".into());
        }
        Ok(())
    }
}

/// Why a ready task is not running yet (tagged by `kind`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum WaitReason {
    /// Global cap.
    GlobalLimit {
        /// Running.
        running: u32,
        /// Cap.
        limit: u32,
    },
    /// Project cap (or the run's parallel cap).
    ProjectLimit {
        /// Running.
        running: u32,
        /// Cap.
        limit: u32,
    },
    /// Team cap.
    TeamLimit {
        /// Team.
        team: String,
        /// Running.
        running: u32,
        /// Cap.
        limit: u32,
    },
    /// Provider rate-limit back-off.
    RateLimited {
        /// Provider id.
        provider_id: String,
        /// Back-off end.
        until: Timestamp,
    },
    /// Provider concurrency.
    ProviderConcurrency {
        /// Provider id.
        provider_id: String,
        /// Running.
        running: u32,
        /// Slots.
        limit: u32,
    },
    /// Model concurrency.
    ModelConcurrency {
        /// Model key `<provider>/<model>`.
        model: String,
        /// Running.
        running: u32,
        /// Slots.
        limit: u32,
    },
    /// CPU threshold.
    Cpu {
        /// Projected usage.
        projected_percent: f32,
        /// Threshold.
        limit_percent: f32,
    },
    /// RAM reserve.
    Memory {
        /// Free MiB.
        free_mb: u64,
        /// Needed MiB.
        needed_mb: u64,
    },
    /// Disk reserve.
    Disk {
        /// Free MiB.
        free_mb: u64,
        /// Reserve MiB.
        min_mb: u64,
    },
    /// GPU threshold.
    Gpu {
        /// Usage.
        usage_percent: f32,
        /// Threshold.
        limit_percent: f32,
    },
    /// Same files are being edited by another agent in the same worktree.
    FileConflict {
        /// The other task.
        with_task: TaskId,
        /// One overlapping path.
        path: String,
    },
    /// The run reached its total agent budget.
    AgentBudget {
        /// Agents spawned.
        spawned: u32,
        /// Cap.
        limit: u32,
    },
    /// The run is paused.
    Paused,
}

/// Lifecycle of a scheduler entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SchedEntryState {
    /// An agent works on it.
    Running,
    /// Ready, admissible, about to start.
    Queued,
    /// Ready but held back (see `reason`).
    Waiting,
}

/// A task as the scheduler sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedEntry {
    /// Task.
    pub task_id: TaskId,
    /// Project.
    pub project_id: ProjectId,
    /// Manager run, if any.
    pub run_id: Option<ManagerRunId>,
    /// Title.
    pub title: String,
    /// State.
    pub state: SchedEntryState,
    /// Task status.
    pub status: TaskStatus,
    /// Base priority.
    pub priority: TaskPriority,
    /// Priority after aging.
    pub effective_priority: TaskPriority,
    /// When it became ready / started.
    pub since: Timestamp,
    /// Provider id.
    pub provider_id: Option<String>,
    /// Model key.
    pub model: Option<String>,
    /// Team.
    pub team: Option<String>,
    /// Agent working on it.
    pub agent_id: Option<AgentId>,
    /// Why it waits (`Waiting`).
    pub reason: Option<WaitReason>,
    /// English explanation ("waiting: provider concurrency 2/2").
    pub reason_text: Option<String>,
    /// Position in the global queue (1 = next), for queued/waiting entries.
    pub position: Option<u32>,
}

/// Load of one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderLoad {
    /// Provider id.
    pub provider_id: String,
    /// Display name, when known.
    pub name: Option<String>,
    /// Agents using it.
    pub running_agents: u32,
    /// Agent slots (None = unlimited).
    pub agent_limit: Option<u32>,
    /// Model requests in flight.
    pub in_flight: u32,
    /// Requests waiting for a concurrency permit.
    pub queued_requests: u32,
    /// Rate-limit back-off end.
    pub rate_limited_until: Option<Timestamp>,
    /// Rate limits seen in this session.
    pub rate_limit_hits: u32,
}

/// Load of one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectLoad {
    /// Project.
    pub project_id: ProjectId,
    /// Running agents.
    pub running: u32,
    /// Ready tasks not running yet.
    pub queued: u32,
    /// Effective cap.
    pub limit: u32,
}

/// Host resources as used for admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerResources {
    /// A live sample exists (otherwise resources never limit).
    pub sampled: bool,
    /// CPU usage percent.
    pub cpu_percent: f32,
    /// Logical cores.
    pub logical_cores: u32,
    /// Free RAM MiB.
    pub ram_available_mb: u64,
    /// Total RAM MiB.
    pub ram_total_mb: u64,
    /// Free disk MiB (data volume).
    pub disk_free_mb: Option<u64>,
    /// GPU usage percent (not sampled yet on any platform → None).
    pub gpu_percent: Option<f32>,
}

/// Predicted merge conflict between two agents in separate worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[ts(rename = "SchedulerMergeRisk")]
pub struct MergeRisk {
    /// Project.
    pub project_id: ProjectId,
    /// First task.
    pub task_a: TaskId,
    /// Second task.
    pub task_b: TaskId,
    /// Branch of the first.
    pub branch_a: Option<String>,
    /// Branch of the second.
    pub branch_b: Option<String>,
    /// Overlapping files (predicted from the plan and actually touched).
    pub files: Vec<String>,
    /// `git merge-tree` confirmed conflicting paths (empty = overlap only).
    pub conflicts: Vec<String>,
    /// Detected at.
    pub detected_at: Timestamp,
}

/// Counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerCounts {
    /// Running agents (scheduled tasks).
    pub running: u32,
    /// Ready, about to start.
    pub queued: u32,
    /// Ready but held back.
    pub waiting: u32,
}

/// Params of `scheduler.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerStatusParams {
    /// Only entries of this project (counts and loads stay global).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `scheduler.status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerStatus {
    /// Settings in force.
    pub settings: SchedulerSettings,
    /// Global counters.
    pub counts: SchedulerCounts,
    /// Running entries.
    pub running: Vec<SchedEntry>,
    /// Queued + waiting entries in service order.
    pub queue: Vec<SchedEntry>,
    /// Provider loads.
    pub providers: Vec<ProviderLoad>,
    /// Project loads.
    pub projects: Vec<ProjectLoad>,
    /// Host resources.
    pub resources: SchedulerResources,
    /// Predicted merge conflicts.
    pub merge_risks: Vec<MergeRisk>,
    /// Snapshot time.
    pub updated_at: Timestamp,
}

/// Params of `scheduler.setLimits` (full replacement, validated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetSchedulerLimitsParams {
    /// New settings.
    pub settings: SchedulerSettings,
}

/// Result of `scheduler.setLimits`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetSchedulerLimitsResult {
    /// Stored settings.
    pub settings: SchedulerSettings,
}

/// Params of `scheduler.explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerExplainParams {
    /// Task.
    pub task_id: TaskId,
}

/// One admission check result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdmissionCheckResult {
    /// Check (`global`, `project`, `team`, `rateLimit`, `provider`, `model`,
    /// `cpu`, `memory`, `disk`, `gpu`, `files`).
    pub check: String,
    /// Passes.
    pub ok: bool,
    /// Failure reason.
    pub reason: Option<WaitReason>,
    /// English explanation.
    pub reason_text: Option<String>,
}

/// Result of `scheduler.explain`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerExplanation {
    /// Task.
    pub task_id: TaskId,
    /// Task status.
    pub status: TaskStatus,
    /// Scheduler entry, when the task is running or queued by a live run.
    pub entry: Option<SchedEntry>,
    /// Managed by a live Manager run (otherwise nothing will start it automatically).
    pub scheduled: bool,
    /// Every admission check, evaluated now against the current load.
    pub checks: Vec<AdmissionCheckResult>,
    /// English one-line summary.
    pub summary: String,
}

/// `SchedulerUpdated` (topic `global`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchedulerUpdatedPayload {
    /// Counters.
    pub counts: SchedulerCounts,
    /// Settings changed (not just load).
    pub settings_changed: bool,
}

/// `MergeRiskDetected` (topic `project:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeRiskPayload {
    /// The risk.
    pub risk: MergeRisk,
}

method!(
    /// `scheduler.status` — running / queued / waiting work with reasons, loads, limits.
    Status, "scheduler.status", SchedulerStatusParams, SchedulerStatus
);
method!(
    /// `scheduler.setLimits` — replace the scheduler settings.
    SetLimits, "scheduler.setLimits", SetSchedulerLimitsParams, SetSchedulerLimitsResult
);
method!(
    /// `scheduler.explain` — why a task is (not) running, check by check.
    Explain, "scheduler.explain", SchedulerExplainParams, SchedulerExplanation
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_missing_fields_and_validate() {
        let s: SchedulerSettings = serde_json::from_value(serde_json::json!({"maxParallelGlobal": 3})).unwrap();
        assert_eq!(s.max_parallel_global, 3);
        assert_eq!(s.max_parallel_per_project, 4);
        assert_eq!(s.auto_resume, AutoResumePolicy::FollowApproval);
        assert!(s.validate().is_ok());
        assert!(SchedulerSettings { max_parallel_global: 0, ..SchedulerSettings::default() }.validate().is_err());
        assert!(SchedulerSettings { max_cpu_percent: f32::NAN, ..SchedulerSettings::default() }.validate().is_err());
    }

    #[test]
    fn wait_reason_wire_shape() {
        let r = WaitReason::ProviderConcurrency { provider_id: "p".into(), running: 2, limit: 2 };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "providerConcurrency", "providerId": "p", "running": 2, "limit": 2}));
        let back: WaitReason = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
        assert_eq!(serde_json::to_value(WaitReason::Paused).unwrap(), serde_json::json!({"kind": "paused"}));
    }
}
