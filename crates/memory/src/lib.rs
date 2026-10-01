//! # orkestr-memory — **SKELETON** (Phase 1–2 fill it in)
//!
//! Memory engine (MASTER_SPEC §20–21): project memory, global memory (project
//! rules win), knowledge base sources, decision log, full-text search (SQLite
//! FTS5 via `orkestr-storage`, own migrations).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{Decision, MemoryItem, MemoryItemId, MemoryScope, ProjectId};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// Not found.
    #[error("not found")]
    NotFound,
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Memory storage and search.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Inserts or updates an item.
    async fn upsert(&self, item: &MemoryItem) -> Result<(), MemoryError>;
    /// Deletes an item.
    async fn delete(&self, id: MemoryItemId) -> Result<(), MemoryError>;
    /// Lists items of a scope.
    async fn list(&self, scope: MemoryScope) -> Result<Vec<MemoryItem>, MemoryError>;
    /// Full-text search across the given scopes (project scopes rank first).
    async fn search(&self, scopes: &[MemoryScope], query: &str, limit: u32) -> Result<Vec<MemoryItem>, MemoryError>;
}

/// Decision log (§21).
#[async_trait]
pub trait DecisionLog: Send + Sync {
    /// Records a decision.
    async fn record(&self, decision: &Decision) -> Result<(), MemoryError>;
    /// Lists decisions of a project, newest first.
    async fn list(&self, project: ProjectId) -> Result<Vec<Decision>, MemoryError>;
}
