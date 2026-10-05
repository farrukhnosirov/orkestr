//! Persistence of code reviews and consensus runs (Phase 2,
//! review-consensus). Documents are stored as JSON next to indexed columns,
//! like the Manager's plans and runs.

use orkestr_domain::{ConsensusId, ProjectId, ReviewId, TaskId};
use orkestr_protocol::methods::consensus::{ConsensusRun, ConsensusStatus};
use orkestr_protocol::methods::reviews::{Review, ReviewStatus};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};

use crate::error::{OrchResult, OrchestratorError};

/// Migrations of the review / consensus tables (applied after the orchestrator's).
pub const REVIEW_MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_0002_reviews_consensus",
    owner: "orchestrator",
    up: "CREATE TABLE reviews (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            task_id     TEXT NOT NULL,
            kind        TEXT NOT NULL,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX reviews_task ON reviews(task_id, created_at);
         CREATE INDEX reviews_project ON reviews(project_id, created_at);
         CREATE INDEX reviews_status ON reviews(status);
         CREATE TABLE consensus_runs (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX consensus_runs_project ON consensus_runs(project_id, created_at);
         CREATE INDEX consensus_runs_status ON consensus_runs(status);",
    down: "DROP INDEX IF EXISTS consensus_runs_status;
           DROP INDEX IF EXISTS consensus_runs_project;
           DROP TABLE IF EXISTS consensus_runs;
           DROP INDEX IF EXISTS reviews_status;
           DROP INDEX IF EXISTS reviews_project;
           DROP INDEX IF EXISTS reviews_task;
           DROP TABLE IF EXISTS reviews;",
}];

/// Review / consensus migrations (the core appends them after
/// [`crate::orchestrator_migrations`]).
#[must_use]
pub fn review_migrations() -> Vec<Migration> {
    REVIEW_MIGRATIONS.to_vec()
}

fn enum_str<T: serde::Serialize>(v: &T) -> Result<String, StorageError> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

fn parse_doc<T: serde::de::DeserializeOwned>(doc: &str) -> Result<T, StorageError> {
    serde_json::from_str(doc).map_err(StorageError::from)
}

/// Persistence facade (cheap to clone).
#[derive(Debug, Clone)]
pub struct ReviewStore {
    db: Db,
}

impl ReviewStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces a review.
    pub async fn save_review(&self, r: &Review) -> OrchResult<()> {
        let doc = serde_json::to_string(r)?;
        let kind = enum_str(&r.kind)?;
        let status = enum_str(&r.status)?;
        let (id, project, task) = (r.id.to_string(), r.project_id.to_string(), r.task_id.to_string());
        let (created, updated) = (r.created_at.to_rfc3339(), r.updated_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO reviews (id, project_id, task_id, kind, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![id, project, task, kind, status, created, updated, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One review.
    pub async fn review(&self, id: ReviewId) -> OrchResult<Review> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| Ok(c.query_row("SELECT doc FROM reviews WHERE id = ?1", [key], |r| r.get(0)).optional()?))
            .await?;
        match doc {
            Some(d) => Ok(parse_doc(&d)?),
            None => Err(OrchestratorError::NotFound(format!("review {id}"))),
        }
    }

    /// Reviews, newest first, filtered by project and/or task.
    pub async fn reviews(&self, project: Option<ProjectId>, task: Option<TaskId>, limit: u32) -> OrchResult<Vec<Review>> {
        let (p, t) = (project.map(|p| p.to_string()), task.map(|t| t.to_string()));
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM reviews WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR task_id = ?2)
                     ORDER BY created_at DESC, id DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![p, t, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }

    /// Reviews still marked in progress (crash recovery).
    pub async fn unfinished_reviews(&self) -> OrchResult<Vec<Review>> {
        let (running, verifying) = (enum_str(&ReviewStatus::Running)?, enum_str(&ReviewStatus::Verifying)?);
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare("SELECT doc FROM reviews WHERE status IN (?1, ?2)")?;
                let rows = stmt.query_map(params![running, verifying], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }

    /// Inserts or replaces a consensus run.
    pub async fn save_consensus(&self, r: &ConsensusRun) -> OrchResult<()> {
        let doc = serde_json::to_string(r)?;
        let status = enum_str(&r.status)?;
        let (id, project) = (r.id.to_string(), r.project_id.to_string());
        let (created, updated) = (r.created_at.to_rfc3339(), r.updated_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO consensus_runs (id, project_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![id, project, status, created, updated, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One consensus run.
    pub async fn consensus(&self, id: ConsensusId) -> OrchResult<ConsensusRun> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM consensus_runs WHERE id = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?;
        match doc {
            Some(d) => Ok(parse_doc(&d)?),
            None => Err(OrchestratorError::NotFound(format!("consensus run {id}"))),
        }
    }

    /// Consensus runs of a project, newest first.
    pub async fn consensus_runs(&self, project: ProjectId, limit: u32) -> OrchResult<Vec<ConsensusRun>> {
        let p = project.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM consensus_runs WHERE project_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![p, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }

    /// Consensus runs still in progress (crash recovery).
    pub async fn unfinished_consensus(&self) -> OrchResult<Vec<ConsensusRun>> {
        let (a, b) = (enum_str(&ConsensusStatus::Proposing)?, enum_str(&ConsensusStatus::Synthesizing)?);
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare("SELECT doc FROM consensus_runs WHERE status IN (?1, ?2)")?;
                let rows = stmt.query_map(params![a, b], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }
}
