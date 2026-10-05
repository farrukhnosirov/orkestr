//! Persistence of task quality pipelines and per-project pipeline
//! configurations (JSON documents next to indexed columns).

use orkestr_domain::{ManagerRunId, ProjectId, TaskId};
use orkestr_protocol::methods::pipeline::{PipelineConfig, TaskPipeline};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};

use crate::error::{OrchResult, OrchestratorError};

/// Migrations of the pipeline tables (the core appends them last).
pub const PIPELINE_MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_0005_quality_pipeline",
    owner: "orchestrator",
    up: "CREATE TABLE task_pipelines (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            task_id     TEXT NOT NULL,
            run_id      TEXT,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX task_pipelines_task ON task_pipelines(task_id, created_at);
         CREATE INDEX task_pipelines_project ON task_pipelines(project_id, created_at);
         CREATE INDEX task_pipelines_run ON task_pipelines(run_id);
         CREATE TABLE pipeline_configs (
            project_id  TEXT PRIMARY KEY NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS pipeline_configs;
           DROP INDEX IF EXISTS task_pipelines_run;
           DROP INDEX IF EXISTS task_pipelines_project;
           DROP INDEX IF EXISTS task_pipelines_task;
           DROP TABLE IF EXISTS task_pipelines;",
}];

/// Pipeline migrations (the core appends them after every other orchestrator list).
#[must_use]
pub fn pipeline_migrations() -> Vec<Migration> {
    PIPELINE_MIGRATIONS.to_vec()
}

fn enum_str<T: serde::Serialize>(v: &T) -> Result<String, StorageError> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

fn parse<T: serde::de::DeserializeOwned>(docs: &[String]) -> OrchResult<Vec<T>> {
    docs.iter().map(|d| serde_json::from_str(d).map_err(|e| OrchestratorError::from(StorageError::from(e)))).collect()
}

/// Persistence facade (cheap to clone).
#[derive(Debug, Clone)]
pub struct PipelineStore {
    db: Db,
}

impl PipelineStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces a pipeline.
    pub async fn save(&self, p: &TaskPipeline) -> OrchResult<()> {
        let doc = serde_json::to_string(p)?;
        let status = enum_str(&p.status)?;
        let (id, project, task) = (p.id.clone(), p.project_id.to_string(), p.task_id.to_string());
        let run = p.run_id.map(|r| r.to_string());
        let (created, updated) = (p.started_at.to_rfc3339(), p.updated_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO task_pipelines (id, project_id, task_id, run_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        run_id = excluded.run_id, doc = excluded.doc",
                    params![id, project, task, run, status, created, updated, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Pipelines of a task, newest first.
    pub async fn for_task(&self, task: TaskId, limit: u32) -> OrchResult<Vec<TaskPipeline>> {
        let t = task.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM task_pipelines WHERE task_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![t, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        parse(&docs)
    }

    /// Latest pipeline of a task.
    pub async fn latest(&self, task: TaskId) -> OrchResult<Option<TaskPipeline>> {
        Ok(self.for_task(task, 1).await?.into_iter().next())
    }

    /// Pipelines of a project (optionally of one run), newest first.
    pub async fn list(
        &self,
        project: ProjectId,
        run: Option<ManagerRunId>,
        limit: u32,
    ) -> OrchResult<Vec<TaskPipeline>> {
        let (p, r) = (project.to_string(), run.map(|r| r.to_string()));
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM task_pipelines WHERE project_id = ?1 AND (?2 IS NULL OR run_id = ?2)
                     ORDER BY created_at DESC, id DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![p, r, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        parse(&docs)
    }

    /// The saved configuration of a project, if any.
    pub async fn config(&self, project: ProjectId) -> OrchResult<Option<PipelineConfig>> {
        let p = project.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM pipeline_configs WHERE project_id = ?1", [p], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        Ok(match doc {
            Some(d) => parse::<PipelineConfig>(&[d])?.into_iter().next(),
            None => None,
        })
    }

    /// Saves a configuration.
    pub async fn save_config(&self, cfg: &PipelineConfig) -> OrchResult<()> {
        let doc = serde_json::to_string(cfg)?;
        let p = cfg.project_id.to_string();
        let at = cfg.updated_at.unwrap_or_else(orkestr_domain::now).to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO pipeline_configs (project_id, updated_at, doc) VALUES (?1, ?2, ?3)
                     ON CONFLICT(project_id) DO UPDATE SET updated_at = excluded.updated_at, doc = excluded.doc",
                    params![p, at, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Removes a saved configuration (defaults apply again).
    pub async fn reset_config(&self, project: ProjectId) -> OrchResult<()> {
        let p = project.to_string();
        self.db
            .call(move |c| {
                c.execute("DELETE FROM pipeline_configs WHERE project_id = ?1", [p])?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
