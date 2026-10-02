//! # orkestr-snapshots
//!
//! Snapshots and rollback (MASTER_SPEC §25, §63.J, ADR-0021): checkpoints of
//! a workspace directory including uncommitted and untracked state — never a
//! git commit, ref or stash. File contents go to the content-addressed
//! `ObjectStore` (blake3, deduplicated across snapshots); a JSON manifest
//! `{path, hash, mode, size}` plus git HEAD/branch/dirty metadata describes
//! each snapshot.
//!
//! * `Rollback Workspace` is planned first ([`SnapshotService::plan_rollback`])
//!   and applied only on confirmation; files created after the snapshot are
//!   kept unless explicitly selected for deletion.
//! * `Rollback Task` restores only the files a task touched, and only where
//!   the file still holds the task's after-state; anything changed by someone
//!   else afterwards is reported as a conflict and never overwritten.
//! * Every rollback first takes a `PreRollback` snapshot, so it is undoable.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod engine;
pub mod error;
pub mod scan;
pub mod store;

pub use engine::{CreateSnapshot, GitState, GitStateProvider, Manifest, SnapshotService, WorkspaceRollback};
pub use error::{SnapshotError, SnapshotResult};
pub use scan::ScanOptions;

/// Migrations owned by this crate.
#[must_use]
pub fn snapshot_migrations() -> Vec<orkestr_storage::Migration> {
    store::MIGRATIONS.to_vec()
}
