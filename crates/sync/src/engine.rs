//! The Phase 4 sync engine contract (ADR-0031, `sync-protocol.md`).
//!
//! Step 0 skeleton: the trait and its types are frozen for the core
//! (`Services.sync`, `rpc/sync.rs`); the sync package implements them.
//! Types may grow additively.

use std::path::PathBuf;

use async_trait::async_trait;
use orkestr_domain::{ProjectId, Timestamp};

use crate::{SyncError, SyncReport};

/// Remote sync configuration of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSyncConfig {
    /// Sync server base URL (the gateway; router mounted at `/v1/sync`).
    pub server_url: String,
    /// Bearer token at the sync server.
    pub access_token: String,
    /// Shared remote key material (base64url) all devices of the account use.
    pub remote_key: String,
}

/// Conflict id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConflictId(pub String);

/// How to resolve a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the local version.
    KeepLocal,
    /// Keep the remote version.
    KeepRemote,
    /// Keep both (the conflict copy stays on disk).
    KeepBoth,
}

/// A detected conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConflict {
    /// Id.
    pub id: ConflictId,
    /// Project.
    pub project_id: ProjectId,
    /// Workspace path or record key.
    pub path: String,
    /// Conflict copy path (workspace stream).
    pub conflict_copy: Option<String>,
    /// Detection time.
    pub detected_at: Timestamp,
}

/// Sync state of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStatus {
    /// Remote sync configured.
    pub configured: bool,
    /// Last successful round.
    pub last_sync_at: Option<Timestamp>,
    /// Pending outbox items.
    pub pending: u64,
    /// Unresolved conflicts.
    pub conflicts: u32,
    /// Last error.
    pub last_error: Option<String>,
}

/// The sync engine used by the core.
#[async_trait]
pub trait SyncEngine: Send + Sync {
    /// Configures (or reconfigures) remote sync of a project rooted at `root`.
    async fn configure(&self, p: ProjectId, root: PathBuf, cfg: RemoteSyncConfig) -> Result<(), SyncError>;
    /// Stores a record in the `records:` stream (uiState, task summaries).
    async fn put_record(&self, p: ProjectId, key: &str, v: serde_json::Value) -> Result<(), SyncError>;
    /// Reads a record from the `records:` stream.
    async fn get_record(&self, p: ProjectId, key: &str) -> Result<Option<serde_json::Value>, SyncError>;
    /// Runs one push + pull round now.
    async fn sync_now(&self, p: ProjectId) -> Result<SyncReport, SyncError>;
    /// Current state.
    async fn status(&self, p: ProjectId) -> Result<SyncStatus, SyncError>;
    /// Unresolved conflicts.
    async fn list_conflicts(&self, p: ProjectId) -> Result<Vec<SyncConflict>, SyncError>;
    /// Resolves a conflict.
    async fn resolve_conflict(&self, id: ConflictId, r: Resolution) -> Result<(), SyncError>;
}
