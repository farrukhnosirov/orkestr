//! # orkestr-task-engine
//!
//! Responsibility (MASTER_SPEC §10–11, contract §5, ADR-0022):
//!
//! * [`state_machine`] — the task state machine, implemented here and only
//!   here, including the privileged crash-recovery edges reserved for
//!   [`orkestr_domain::TaskActor::SystemRecovery`].
//! * [`store`] — SQLite persistence (`tasks`, `task_dependencies`,
//!   `task_history`, `task_executions`) with reversible migrations.
//! * [`dag`] — dependency DAG: cycle detection, ready set, topological order,
//!   critical path, epic subtrees, dependency propagation rules.
//! * [`queue`] — priority queue (Critical > High > Normal > Low > Background,
//!   FIFO within a level) with aging.
//! * [`scheduler`] — resource-aware admission (parallel caps, provider slots,
//!   CPU/RAM thresholds, file-overlap conflict avoidance). Pure logic.
//! * [`service`] — [`TaskService`]: atomic operations (one SQLite transaction
//!   each) + `Task*` events, used by the core RPC handlers and crash recovery.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dag;
pub mod queue;
pub mod scheduler;
pub mod service;
pub mod state_machine;
pub mod store;

use orkestr_domain::{ProjectId, TaskId, TaskStatus};

pub use dag::{dependency_reaction, Dag, DagNode, Trigger};
pub use queue::{effective_priority, AgingPolicy, PriorityQueue, QueueEntry};
pub use scheduler::{Deferral, ResourceSnapshot, SchedTask, SchedulePlan, Scheduler, SchedulerLimits};
pub use service::{AgentRunUpdate, RecoveryReport, TaskService};
pub use state_machine::{
    allowed_transitions, can_transition, can_transition_as, is_privileged_edge, transition, transition_as, Applied,
    InvalidTransition,
};

/// Maximum parent/child depth (Epic, Feature, Story, Task, Subtask + 1 spare).
pub const MAX_DEPTH: usize = 6;

/// Errors of the task engine.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// Illegal state change.
    #[error(transparent)]
    InvalidTransition(#[from] InvalidTransition),
    /// Unknown task.
    #[error("task {0} not found")]
    NotFound(TaskId),
    /// Unknown project.
    #[error("project {0} not found")]
    ProjectNotFound(ProjectId),
    /// Unknown dependency edge.
    #[error("task {task} does not depend on {depends_on}")]
    DependencyNotFound {
        /// Dependent.
        task: TaskId,
        /// Prerequisite.
        depends_on: TaskId,
    },
    /// Adding the dependency (or parent) would create a cycle; the path closes it.
    #[error("dependency cycle: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join(" → "))]
    Cycle(Vec<TaskId>),
    /// A transition guard does not hold.
    #[error("guard `{guard}` failed: {detail}")]
    GuardFailed {
        /// Guard name.
        guard: &'static str,
        /// Explanation.
        detail: String,
    },
    /// Optimistic concurrency conflict.
    #[error("version conflict: task is at version {current}")]
    VersionConflict {
        /// Current version.
        current: u32,
    },
    /// The operation is not allowed in the task's current state.
    #[error("{0}")]
    Conflict(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

impl From<orkestr_storage::StorageError> for TaskError {
    fn from(e: orkestr_storage::StorageError) -> Self {
        TaskError::Storage(e.to_string())
    }
}

impl From<orkestr_storage::rusqlite::Error> for TaskError {
    fn from(e: orkestr_storage::rusqlite::Error) -> Self {
        TaskError::Storage(format!("sqlite: {e}"))
    }
}

impl From<serde_json::Error> for TaskError {
    fn from(e: serde_json::Error) -> Self {
        TaskError::Storage(format!("json: {e}"))
    }
}

/// Migrations owned by this crate (compose after `storage_migrations()`).
#[must_use]
pub fn task_migrations() -> Vec<orkestr_storage::Migration> {
    store::MIGRATIONS.to_vec()
}

/// Whether a task may become `Ready` given the statuses of its dependencies.
#[must_use]
pub fn dependencies_satisfied(dependency_statuses: &[TaskStatus]) -> bool {
    dependency_statuses.iter().all(|s| *s == TaskStatus::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_only_when_all_dependencies_completed() {
        assert!(dependencies_satisfied(&[]));
        assert!(dependencies_satisfied(&[TaskStatus::Completed, TaskStatus::Completed]));
        assert!(!dependencies_satisfied(&[TaskStatus::Completed, TaskStatus::Review]));
        assert!(!dependencies_satisfied(&[TaskStatus::Cancelled]));
    }

    #[test]
    fn cycle_error_renders_path() {
        let a = TaskId::new();
        let e = TaskError::Cycle(vec![a, a]);
        assert!(e.to_string().contains('→'));
    }
}
