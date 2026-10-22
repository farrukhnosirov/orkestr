//! # orkestr-sync
//!
//! Sync engine (MASTER_SPEC §40–45, ADR-0019, ADR-0031). The **Local Only**
//! backend never transfers anything. Phase 4 adds [`DefaultSyncEngine`]:
//! encrypted (zstd + XChaCha20-Poly1305, keyed chunk ids), incremental
//! (FastCDC), signed (Ed25519 manifests/refs, version vectors with
//! anti-rollback), offline-first (outbox) sync of the `workspace:` and
//! `records:` streams against the [`server_router`] store, with conflict copies.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod crypto;
pub mod engine;
pub mod local_engine;
pub mod manifest;
pub mod merge;
pub mod remote;
pub mod scan;
pub mod server;
pub mod vv;

pub use engine::{ConflictId, RemoteSyncConfig, Resolution, SyncConflict, SyncEngine, SyncStatus};
pub use local_engine::{check_policy, DefaultSyncEngine};
pub use server::{server_router, SyncServerState};

use async_trait::async_trait;
use orkestr_domain::{ProjectId, Timestamp};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Remote sync forbidden by the project's privacy policy (§56).
    #[error("remote sync is not allowed for this project")]
    NotAllowed,
    /// Backend failure.
    #[error("backend: {0}")]
    Backend(String),
    /// Forbidden by the project's privacy policy (§56, SR-76).
    #[error("forbidden by privacy policy: {0}")]
    PolicyDenied(String),
    /// Bad AEAD tag / signature / unknown signer / rollback.
    #[error("integrity check failed: {0}")]
    Integrity(String),
    /// Sync server unreachable or refused the request.
    #[error("network: {0}")]
    Network(String),
    /// Object or conflict not found.
    #[error("not found")]
    NotFound,
    /// Remote sync is not configured for the project.
    #[error("remote sync is not configured for this project")]
    NotConfigured,
    /// Invalid configuration.
    #[error("invalid configuration: {0}")]
    Config(String),
    /// Local storage failure.
    #[error("storage: {0}")]
    Storage(String),
}

/// A local journal entry (File Change → Local Journal → Snapshot → Sync).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Monotonic sequence.
    pub seq: u64,
    /// Project.
    pub project_id: ProjectId,
    /// Changed path (relative).
    pub path: String,
    /// Object id of the new content (None = deleted).
    pub object_id: Option<String>,
    /// Time.
    pub ts: Timestamp,
}

/// Result of a sync round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Items pushed.
    pub pushed: u64,
    /// Items pulled.
    pub pulled: u64,
    /// Bytes transferred.
    pub bytes: u64,
    /// Conflicts detected.
    pub conflicts: u32,
}

/// A sync backend (§45 interface).
#[async_trait]
pub trait SyncBackend: Send + Sync {
    /// Backend name (`local-only`, `self-hosted`, `cloud`).
    fn name(&self) -> &'static str;
    /// Whether data leaves this machine.
    fn is_remote(&self) -> bool;
    /// Pushes journal entries; returns what was synced.
    async fn push(&self, entries: &[JournalEntry]) -> Result<SyncReport, SyncError>;
    /// Pulls remote changes for a project.
    async fn pull(&self, project: ProjectId) -> Result<SyncReport, SyncError>;
}

/// Local-only storage mode: nothing is ever transferred.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalOnlyBackend;

#[async_trait]
impl SyncBackend for LocalOnlyBackend {
    fn name(&self) -> &'static str {
        "local-only"
    }
    fn is_remote(&self) -> bool {
        false
    }
    async fn push(&self, _entries: &[JournalEntry]) -> Result<SyncReport, SyncError> {
        Ok(SyncReport::default())
    }
    async fn pull(&self, _project: ProjectId) -> Result<SyncReport, SyncError> {
        Ok(SyncReport::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_only_never_transfers() {
        let b = LocalOnlyBackend;
        assert!(!b.is_remote());
        let entry = JournalEntry {
            seq: 1,
            project_id: ProjectId::new(),
            path: "a".into(),
            object_id: None,
            ts: orkestr_domain::now(),
        };
        assert_eq!(b.push(&[entry]).await.unwrap(), SyncReport::default());
    }
}
