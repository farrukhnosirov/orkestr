//! Tasks (§10–11). The state machine itself lives in `orkestr-task-engine`
//! (ENGINEERING_CONTRACT §5); this module only defines the data.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{ArtifactRef, Budget, ModelRef, Timestamp, UsageSummary, WorktreeRef};
use crate::ids::{AgentId, ApprovalId, ExecutionId, ProjectId, TaskId};

/// The 12 task states (contract §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TaskStatus {
    /// Planned, dependencies not evaluated yet.
    #[default]
    Planned,
    /// All dependencies completed; can be scheduled.
    Ready,
    /// Being executed.
    Running,
    /// Waiting for approval / user / provider.
    Waiting,
    /// Unmet dependency or policy.
    Blocked,
    /// Under review.
    Review,
    /// Under test.
    Testing,
    /// Done (terminal).
    Completed,
    /// Failed (retryable).
    Failed,
    /// Cancelled (terminal).
    Cancelled,
    /// Interrupted by crash/shutdown.
    Interrupted,
    /// Being recovered after interruption/failure.
    Recovering,
}

impl TaskStatus {
    /// All states in a stable order.
    pub const ALL: [TaskStatus; 12] = [
        TaskStatus::Planned,
        TaskStatus::Ready,
        TaskStatus::Running,
        TaskStatus::Waiting,
        TaskStatus::Blocked,
        TaskStatus::Review,
        TaskStatus::Testing,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Interrupted,
        TaskStatus::Recovering,
    ];

    /// Terminal states never transition again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Cancelled)
    }

    /// States that become `Interrupted` after a crash (contract §5, §62.13).
    #[must_use]
    pub fn is_interruptible_on_crash(self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Testing | TaskStatus::Review)
    }
}

/// Priority (§11). Ordered: `Background < Low < Normal < High < Critical`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TaskPriority {
    /// Background work.
    Background,
    /// Low.
    Low,
    /// Normal (default).
    #[default]
    Normal,
    /// High.
    High,
    /// Critical.
    Critical,
}

/// Review state of a task's output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReviewState {
    /// No review required.
    #[default]
    NotRequired,
    /// Review pending.
    Pending,
    /// Reviewer requested changes.
    ChangesRequested,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
}

/// One acceptance criterion (§9, §58).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AcceptanceCriterion {
    /// Criterion text.
    pub text: String,
    /// Whether it was verified as met.
    pub met: bool,
}

/// Final result of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskResult {
    /// Success flag.
    pub success: bool,
    /// Human-readable summary (no hidden chain-of-thought, §1.14).
    pub summary: String,
    /// Structured stop code when the agent run ended on a limit
    /// (`run_budget_exceeded`, `budget_exceeded`, `project_budget_exceeded`,
    /// `max_turns`, `time_limit`). Decisions (retry, run budget) use this —
    /// never the free-text summary. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stop_code: Option<String>,
}

/// Task (§10) — all fields from the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Task {
    /// Id.
    pub id: TaskId,
    /// Project.
    pub project_id: ProjectId,
    /// Parent task (epic/feature/story decomposition, §9).
    pub parent_id: Option<TaskId>,
    /// Title.
    pub title: String,
    /// Description (markdown).
    pub description: String,
    /// Acceptance criteria.
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    /// Ids this task depends on (see also [`TaskDependency`]).
    pub dependencies: Vec<TaskId>,
    /// Priority.
    pub priority: TaskPriority,
    /// Status.
    pub status: TaskStatus,
    /// Assigned agent.
    pub agent_id: Option<AgentId>,
    /// Provider/model used.
    pub model: Option<ModelRef>,
    /// Worktree.
    pub worktree: Option<WorktreeRef>,
    /// Files touched (relative paths).
    pub files_touched: Vec<String>,
    /// Execution history (attempts).
    pub executions: Vec<ExecutionId>,
    /// Usage / cost.
    pub usage: UsageSummary,
    /// Budget for this task.
    pub budget: Option<Budget>,
    /// Object id of the aggregated log, if any.
    pub log_object_id: Option<String>,
    /// Artifacts.
    pub artifacts: Vec<ArtifactRef>,
    /// Approvals requested for this task.
    pub approvals: Vec<ApprovalId>,
    /// Result.
    pub result: Option<TaskResult>,
    /// Review state.
    pub review_state: ReviewState,
    /// Number of attempts so far (self-healing bound, §17).
    pub attempts: u32,
    /// Free tags.
    pub tags: Vec<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
    /// First time it entered `Running`.
    pub started_at: Option<Timestamp>,
    /// Time it reached a terminal state.
    pub finished_at: Option<Timestamp>,
    /// Optimistic-concurrency version, incremented on every persisted change
    /// (task-state-machine.md §3). Added in Phase 1.
    #[serde(default)]
    pub version: u32,
    /// Planner estimate in minutes (critical path weight, §11). `None` = unknown (weight 1).
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    /// When the task last entered `Ready` (queue FIFO + aging, §11).
    #[serde(default)]
    pub ready_since: Option<Timestamp>,
}

impl Task {
    /// Creates a planned task with defaults.
    #[must_use]
    pub fn new(project_id: ProjectId, title: impl Into<String>, now: Timestamp) -> Self {
        Self {
            id: TaskId::new(),
            project_id,
            parent_id: None,
            title: title.into(),
            description: String::new(),
            acceptance_criteria: Vec::new(),
            dependencies: Vec::new(),
            priority: TaskPriority::default(),
            status: TaskStatus::Planned,
            agent_id: None,
            model: None,
            worktree: None,
            files_touched: Vec::new(),
            executions: Vec::new(),
            usage: UsageSummary::default(),
            budget: None,
            log_object_id: None,
            artifacts: Vec::new(),
            approvals: Vec::new(),
            result: None,
            review_state: ReviewState::default(),
            attempts: 0,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            version: 0,
            estimate_minutes: None,
            ready_since: None,
        }
    }
}

/// Dependency semantics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DependencyKind {
    /// `task` may start only after `depends_on` completed.
    #[default]
    FinishToStart,
    /// Informational ordering; does not block.
    Soft,
}

/// An edge of the task DAG (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskDependency {
    /// Dependent task.
    pub task_id: TaskId,
    /// Prerequisite task.
    pub depends_on: TaskId,
    /// Kind.
    pub kind: DependencyKind,
}

/// Who requested a task status change (recorded in the task history).
///
/// `SystemRecovery` is the only actor allowed to use the privileged
/// `{Waiting, Review, Testing} → Interrupted` crash-recovery edges
/// (ADR-0022, CA-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "id")]
#[ts(export)]
pub enum TaskActor {
    /// The local user (desktop/web/mobile client).
    User,
    /// An agent instance.
    Agent(AgentId),
    /// The AI Manager / orchestrator.
    Manager,
    /// Automatic engine behaviour (dependency propagation, scheduler).
    System,
    /// Crash / shutdown recovery of the core (privileged).
    SystemRecovery,
}

/// One persisted status change of a task (`task_history`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskHistoryEntry {
    /// Monotonic row id.
    pub seq: i64,
    /// Task.
    pub task_id: TaskId,
    /// Previous status (`None` for creation).
    pub from: Option<TaskStatus>,
    /// New status.
    pub to: TaskStatus,
    /// Who requested it.
    pub actor: TaskActor,
    /// Reason code / human note.
    pub reason: Option<String>,
    /// Whether a privileged (system-only) edge was used.
    pub forced: bool,
    /// When.
    pub at: Timestamp,
}

/// One execution attempt of a task (`task_executions`). Opened when the task
/// enters `Running` from `Ready`/`Recovering`, closed when it reaches
/// `Completed`, `Failed`, `Cancelled` or `Interrupted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskAttempt {
    /// Id.
    pub id: ExecutionId,
    /// Task.
    pub task_id: TaskId,
    /// Attempt number starting at 1.
    pub attempt: u32,
    /// Agent that ran it, if known.
    pub agent_id: Option<AgentId>,
    /// Status.
    pub status: crate::execution::ExecutionStatus,
    /// Usage attributed to this attempt.
    pub usage: UsageSummary,
    /// Short outcome summary.
    pub summary: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_states() {
        assert_eq!(TaskStatus::ALL.len(), 12);
        assert!(TaskStatus::Completed.is_terminal());
        assert!(!TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Review.is_interruptible_on_crash());
        assert!(!TaskStatus::Ready.is_interruptible_on_crash());
    }

    #[test]
    fn priority_order() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::Background < TaskPriority::Low);
    }

    #[test]
    fn task_roundtrip() {
        let t = Task::new(ProjectId::new(), "Build payroll", crate::common::now());
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["status"], "Planned");
        assert_eq!(v["priority"], "Normal");
        let back: Task = serde_json::from_value(v).unwrap();
        assert_eq!(back, t);
    }
}
