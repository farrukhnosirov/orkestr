//! # orkestr-memory
//!
//! Memory engine (MASTER_SPEC §20–21) on top of the core SQLite database:
//!
//! * **Memory items** ([`MemoryEngine::create_item`] …) in Global / Project
//!   (and private Agent) scopes with kinds, tags, source, pin and an optional
//!   rule key. Full-text search via SQLite FTS5 with bm25 ranking over
//!   title + body + tags ([`MemoryEngine::search`]).
//! * **Effective rules** ([`MemoryEngine::effective_rules`]): project items
//!   override global items with the same key (§20 "project rules win").
//! * **Decision log** (§21) with per-project ADR numbers.
//! * **Knowledge base**: markdown/text/code-doc files or folders chunked by
//!   headings and size into an FTS5 index ([`chunk`]). PDFs and URLs are
//!   recorded as `Failed` with an explicit "not supported yet" message.
//!
//! Every mutation publishes `MemoryUpdated` / `DecisionRecorded` on the event
//! bus when one is attached ([`MemoryEngine::with_events`]).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chunk;
mod decisions;
pub mod fts;
mod items;
mod knowledge;
pub mod schema;
mod util;
pub mod validate;

use orkestr_events::EventBus;
use orkestr_protocol::NewEvent;
use orkestr_storage::{Db, StorageError};

pub use knowledge::{IngestLimits, SUPPORTED_EXTENSIONS};
pub use schema::memory_migrations;

/// Errors raised by the memory engine.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// Entity not found.
    #[error("{0} not found")]
    NotFound(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// Conflicting state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Event publication failed (the change itself was stored).
    #[error("event publication failed: {0}")]
    Events(String),
}

/// Result alias.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// The memory engine. Cheap to clone.
#[derive(Debug, Clone)]
pub struct MemoryEngine {
    db: Db,
    events: Option<EventBus>,
    limits: IngestLimits,
}

impl MemoryEngine {
    /// Engine over a database that has [`memory_migrations`] applied (after the
    /// storage migrations — memory tables reference `projects`).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db, events: None, limits: IngestLimits::default() }
    }

    /// Attaches an event bus: mutations publish `MemoryUpdated` / `DecisionRecorded`.
    #[must_use]
    pub fn with_events(mut self, events: EventBus) -> Self {
        self.events = Some(events);
        self
    }

    /// Overrides knowledge ingestion limits.
    #[must_use]
    pub fn with_limits(mut self, limits: IngestLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Underlying database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    async fn publish(&self, event: NewEvent) -> MemoryResult<()> {
        if let Some(bus) = &self.events {
            bus.publish(event).await.map_err(|e| {
                tracing::error!(error = %e, "failed to publish memory event");
                MemoryError::Events(e.to_string())
            })?;
        }
        Ok(())
    }

    async fn ensure_project(&self, project_id: orkestr_domain::ProjectId) -> MemoryResult<()> {
        let id = project_id.to_string();
        let exists = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)", [&id], |r| r.get::<_, bool>(0))?)
            })
            .await?;
        if exists {
            Ok(())
        } else {
            Err(MemoryError::NotFound(format!("project {project_id}")))
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use orkestr_domain::{now, Project, ProjectId};
    use orkestr_events::EventBus;
    use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};

    use super::*;

    pub async fn engine() -> (MemoryEngine, EventBus) {
        let db = Db::open_in_memory().unwrap();
        let mut all = orkestr_storage::storage_migrations();
        all.extend(memory_migrations());
        db.migrate(all).await.unwrap();
        let bus = EventBus::in_memory(256);
        (MemoryEngine::new(db).with_events(bus.clone()), bus)
    }

    pub async fn project(engine: &MemoryEngine, name: &str) -> ProjectId {
        let p = Project::new(name.to_owned(), now());
        SqliteMetadataStore::new(engine.db().clone()).insert(&p).await.unwrap();
        p.id
    }
}
