//! `snapshots.*` — workspace snapshots and rollback (§25, §63.J).
//!
//! A snapshot captures every non-ignored file of a workspace directory
//! (tracked, uncommitted and untracked) into the content-addressed object
//! store plus a manifest — never git commits, refs or stashes (ADR-0021).
//!
//! Rollbacks are planned first and applied only with `confirm: true`. Every
//! rollback takes a `PreRollback` safety snapshot so it is itself undoable,
//! and task rollback never overwrites files changed by someone else after
//! the task (they are reported as conflicts and skipped).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, SnapshotId, SnapshotReason, TaskId, TaskSnapshotPhase, WorkspaceSnapshot};

use super::method;

/// One file in a snapshot manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManifestEntry {
    /// Path relative to the workspace root, `/`-separated.
    pub path: String,
    /// blake3 hex digest = object id of the content.
    pub hash: String,
    /// Git-style mode (`0o100644` regular, `0o100755` executable).
    pub mode: u32,
    /// Size in bytes.
    pub size: u64,
}

/// Why a path was not captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ExclusionReason {
    /// Larger than the per-file limit.
    OverSizeLimit,
    /// Symbolic link (not followed, not captured).
    Symlink,
    /// Could not be read (locked, permission denied, vanished).
    Unreadable,
}

/// A path skipped while capturing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExcludedEntry {
    /// Path.
    pub path: String,
    /// Reason.
    pub reason: ExclusionReason,
}

/// Params of `snapshots.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListSnapshotsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Only snapshots of this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Only snapshots of this workspace directory (absolute path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace_path: Option<String>,
    /// Max results (default 200, max 1000), newest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `snapshots.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListSnapshotsResult {
    /// Snapshots, newest first.
    pub snapshots: Vec<WorkspaceSnapshot>,
}

/// Params of `snapshots.get`, `snapshots.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotIdParams {
    /// Snapshot.
    pub snapshot_id: SnapshotId,
}

/// Result of `snapshots.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotDetail {
    /// The snapshot.
    pub snapshot: WorkspaceSnapshot,
    /// Captured files, sorted by path.
    pub entries: Vec<ManifestEntry>,
    /// Skipped files.
    pub excluded: Vec<ExcludedEntry>,
}

/// Params of `snapshots.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateSnapshotParams {
    /// Project.
    pub project_id: ProjectId,
    /// Directory to capture (absolute; must be the project root or one of
    /// its repository's worktrees). Default: the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace_path: Option<String>,
    /// Label (default derived from the reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    /// Reason (default `Manual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<SnapshotReason>,
    /// Task the snapshot belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Agent the snapshot belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Before/after marker of a task checkpoint pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_phase: Option<TaskSnapshotPhase>,
}

/// Result of `snapshots.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotResult {
    /// The snapshot.
    pub snapshot: WorkspaceSnapshot,
}

/// How a path differs between a snapshot and the current workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SnapshotChangeKind {
    /// Exists now, not in the snapshot.
    Added,
    /// Content or mode differs.
    Modified,
    /// In the snapshot, missing now.
    Deleted,
}

/// A difference between a snapshot and the current workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotChange {
    /// Path.
    pub path: String,
    /// Kind.
    pub kind: SnapshotChangeKind,
    /// Hash in the snapshot.
    pub snapshot_hash: Option<String>,
    /// Current hash.
    pub current_hash: Option<String>,
}

/// Params of `snapshots.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotDiffParams {
    /// Snapshot.
    pub snapshot_id: SnapshotId,
    /// Also return both contents of this path (for a side-by-side viewer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
}

/// Both versions of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotFileContents {
    /// Path.
    pub path: String,
    /// Content in the snapshot (None when absent there).
    pub snapshot: Option<String>,
    /// Current content (None when absent now).
    pub current: Option<String>,
    /// Either side is binary (contents omitted).
    pub binary: bool,
}

/// Result of `snapshots.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotDiffResult {
    /// Snapshot.
    pub snapshot_id: SnapshotId,
    /// Changed paths, sorted.
    pub changes: Vec<SnapshotChange>,
    /// Contents of `path` when requested.
    pub file: Option<SnapshotFileContents>,
}

/// Params of `snapshots.restoreFile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreFileParams {
    /// Snapshot.
    pub snapshot_id: SnapshotId,
    /// Path to restore (must be in the snapshot).
    pub path: String,
    /// Must be `true` (overwrites the current file).
    #[serde(default)]
    pub confirm: bool,
}

/// What a rollback does to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RollbackAction {
    /// Overwrite the current file with the snapshot version.
    Restore,
    /// Re-create a file deleted since the snapshot.
    Recreate,
    /// Delete a file created since the snapshot.
    Delete,
    /// Leave the file as it is.
    Keep,
}

/// One path of a rollback plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackPlanItem {
    /// Path.
    pub path: String,
    /// Planned action.
    pub action: RollbackAction,
    /// Human explanation.
    pub reason: String,
    /// Current hash (None when absent).
    pub current_hash: Option<String>,
    /// Hash after the rollback (None when the file will not exist).
    pub target_hash: Option<String>,
}

/// Params of `snapshots.planRollback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanRollbackParams {
    /// Snapshot to roll back to.
    pub snapshot_id: SnapshotId,
    /// Delete files created after the snapshot (default false: they are kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub delete_created: Option<bool>,
}

/// A rollback plan (nothing changed yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackPlan {
    /// Snapshot.
    pub snapshot_id: SnapshotId,
    /// Workspace directory.
    pub workspace_path: String,
    /// Items for every path that differs (sorted by path).
    pub items: Vec<RollbackPlanItem>,
    /// Fingerprint of the current state of the planned paths; pass it to
    /// `snapshots.rollbackWorkspace` to refuse when files changed meanwhile.
    pub fingerprint: String,
}

/// Params of `snapshots.rollbackWorkspace` (Dangerous).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackWorkspaceParams {
    /// Snapshot to roll back to.
    pub snapshot_id: SnapshotId,
    /// Must be `true`.
    #[serde(default)]
    pub confirm: bool,
    /// Delete files created after the snapshot (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub delete_created: Option<bool>,
    /// Apply only these paths of the plan (per-file confirmation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub paths: Option<Vec<String>>,
    /// Plan fingerprint from `snapshots.planRollback`; mismatch → `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_fingerprint: Option<String>,
}

/// Params of `snapshots.rollbackTask`.
///
/// The task's checkpoint pair defaults to its earliest `Before` (or oldest)
/// snapshot and its latest `After` (or newest other) snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackTaskParams {
    /// Project.
    pub project_id: ProjectId,
    /// Task to roll back.
    pub task_id: TaskId,
    /// Explicit "before" snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before_snapshot_id: Option<SnapshotId>,
    /// Explicit "after" snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after_snapshot_id: Option<SnapshotId>,
    /// Only compute what would happen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dry_run: Option<bool>,
    /// Must be `true` unless `dryRun`.
    #[serde(default)]
    pub confirm: bool,
}

/// A path a rollback refused to touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackConflict {
    /// Path.
    pub path: String,
    /// Why it was skipped.
    pub reason: String,
}

/// Result of a rollback (also `snapshots.restoreFile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackResult {
    /// Snapshot rolled back to.
    pub snapshot_id: SnapshotId,
    /// Safety snapshot taken right before (None for dry runs).
    pub pre_rollback_snapshot_id: Option<SnapshotId>,
    /// Files written (restored or re-created).
    pub restored: Vec<String>,
    /// Files deleted.
    pub deleted: Vec<String>,
    /// Paths skipped because someone else changed them (never overwritten).
    pub conflicts: Vec<RollbackConflict>,
    /// Paths already in the target state.
    pub unchanged: Vec<String>,
    /// Nothing was written (dry run).
    pub dry_run: bool,
}

/// Result of `snapshots.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteSnapshotResult {
    /// Deleted snapshot.
    pub snapshot_id: SnapshotId,
}

method!(
    /// `snapshots.list`.
    List, "snapshots.list", ListSnapshotsParams, ListSnapshotsResult
);
method!(
    /// `snapshots.get` — snapshot + manifest.
    Get, "snapshots.get", SnapshotIdParams, SnapshotDetail
);
method!(
    /// `snapshots.create` — emits `SnapshotCreated`.
    Create, "snapshots.create", CreateSnapshotParams, SnapshotResult
);
method!(
    /// `snapshots.diff` — snapshot vs current workspace.
    Diff, "snapshots.diff", SnapshotDiffParams, SnapshotDiffResult
);
method!(
    /// `snapshots.restoreFile` — restores one file (safety snapshot first).
    RestoreFile, "snapshots.restoreFile", RestoreFileParams, RollbackResult
);
method!(
    /// `snapshots.planRollback` — what `rollbackWorkspace` would do.
    PlanRollback, "snapshots.planRollback", PlanRollbackParams, RollbackPlan
);
method!(
    /// `snapshots.rollbackWorkspace` — Dangerous; emits `RollbackPerformed`.
    RollbackWorkspace, "snapshots.rollbackWorkspace", RollbackWorkspaceParams, RollbackResult
);
method!(
    /// `snapshots.rollbackTask` — reverts only the task's own changes (§63.J).
    RollbackTask, "snapshots.rollbackTask", RollbackTaskParams, RollbackResult
);
method!(
    /// `snapshots.delete` — removes the snapshot record (objects are GC'd later).
    Delete, "snapshots.delete", SnapshotIdParams, DeleteSnapshotResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_params_default_unconfirmed() {
        let p: RollbackWorkspaceParams =
            serde_json::from_str(r#"{"snapshotId":"0190f0b0-0000-7000-8000-000000000001"}"#).unwrap();
        assert!(!p.confirm);
        assert!(p.paths.is_none());
    }
}
