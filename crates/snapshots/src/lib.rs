//! # orkestr-snapshots — **SKELETON** (Phase 1 fills it in)
//!
//! Snapshots and rollback (MASTER_SPEC §25): checkpoints before risky or
//! autonomous changes, including uncommitted and untracked state (not a git
//! commit). File contents go to the content-addressed `ObjectStore`; a
//! manifest maps paths → object ids. `Rollback Task` restores only the files a
//! task touched and never destroys unrelated changes; every rollback first
//! takes a `PreRollback` snapshot so it is itself undoable.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{Actor, SnapshotId, SnapshotReason, TaskId, WorkspaceId, WorkspaceSnapshot};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Unknown snapshot.
    #[error("snapshot {0} not found")]
    NotFound(SnapshotId),
    /// Restoring would overwrite unrelated changes.
    #[error("rollback would overwrite unrelated changes in {0:?}")]
    WouldClobber(Vec<String>),
    /// I/O / storage failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Outcome of a rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackReport {
    /// Files restored.
    pub files_restored: Vec<String>,
    /// Safety snapshot taken before restoring.
    pub pre_rollback_snapshot: SnapshotId,
}

/// Snapshot engine.
#[async_trait]
pub trait SnapshotEngine: Send + Sync {
    /// Captures a workspace (including uncommitted state).
    async fn create(
        &self,
        workspace: WorkspaceId,
        task: Option<TaskId>,
        reason: SnapshotReason,
        label: &str,
        by: Actor,
    ) -> Result<WorkspaceSnapshot, SnapshotError>;
    /// Reverts only the changes made by `task` since its snapshot.
    async fn rollback_task(&self, task: TaskId) -> Result<RollbackReport, SnapshotError>;
    /// Restores a whole workspace to a snapshot.
    async fn rollback_workspace(&self, snapshot: SnapshotId) -> Result<RollbackReport, SnapshotError>;
}
