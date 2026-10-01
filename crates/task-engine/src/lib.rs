//! # orkestr-task-engine
//!
//! Responsibility (MASTER_SPEC §10–11, contract §5):
//!
//! * **Task state machine — implemented here and only here.** See
//!   [`state_machine`].
//! * Task DAG, dependency resolution (`Ready` only when every dependency is
//!   `Completed`), priority queue (Critical > High > Normal > Low > Background)
//!   and a resource-aware scheduler (CPU/RAM/GPU, API concurrency, provider
//!   limits, git conflicts) — **SKELETON**, see [`TaskStore`] and [`Scheduler`].
//! * Crash recovery hook: `Running/Waiting/Testing/Review` → `Interrupted`
//!   (registered into `orkestr-core::recovery` in Phase 1).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod state_machine;

use async_trait::async_trait;
use orkestr_domain::{ProjectId, Task, TaskDependency, TaskId, TaskStatus};

pub use state_machine::{allowed_transitions, can_transition, transition, InvalidTransition};

/// Errors of the task engine.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// Illegal state change.
    #[error(transparent)]
    InvalidTransition(#[from] InvalidTransition),
    /// Unknown task.
    #[error("task {0} not found")]
    NotFound(TaskId),
    /// Adding the dependency would create a cycle.
    #[error("dependency cycle involving {0}")]
    Cycle(TaskId),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Persistence of tasks and dependencies (SQLite impl arrives in Phase 1).
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Lists tasks of a project.
    async fn list(&self, project: ProjectId) -> Result<Vec<Task>, TaskError>;
    /// Loads a task.
    async fn get(&self, id: TaskId) -> Result<Option<Task>, TaskError>;
    /// Inserts or replaces a task.
    async fn save(&self, task: &Task) -> Result<(), TaskError>;
    /// Adds a dependency edge (rejecting cycles).
    async fn add_dependency(&self, dep: TaskDependency) -> Result<(), TaskError>;
    /// Every task left in an interruptible state (crash recovery).
    async fn interruptible(&self) -> Result<Vec<Task>, TaskError>;
}

/// Picks the next runnable tasks under resource constraints.
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// Returns up to `slots` `Ready` tasks to start, highest priority first.
    async fn next_batch(&self, slots: usize) -> Result<Vec<TaskId>, TaskError>;
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
}
