//! SQLite persistence: profiles, gate configs, runs.

use orkestr_domain::{ProjectId, TaskId, Timestamp};
use orkestr_protocol::methods::quality::{CheckDef, CheckStatus, GateConfig, QualityRun};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Runs kept per project (older ones are pruned).
pub const RUNS_KEPT_PER_PROJECT: u32 = 200;

/// Migrations owned by the quality crate.
#[must_use]
pub fn quality_migrations() -> Vec<Migration> {
    vec![Migration {
        id: "quality_0001_profiles_gates_runs",
        owner: "quality",
        up: "CREATE TABLE quality_profiles (
                project_id TEXT PRIMARY KEY NOT NULL,
                checks_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE quality_gates (
                project_id TEXT PRIMARY KEY NOT NULL,
                config_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE quality_runs (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL,
                task_id TEXT,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL,
                run_json TEXT NOT NULL
            );
            CREATE INDEX quality_runs_project ON quality_runs (project_id, started_at DESC);
            CREATE INDEX quality_runs_task ON quality_runs (task_id, started_at DESC);",
        down: "DROP INDEX IF EXISTS quality_runs_task;
            DROP INDEX IF EXISTS quality_runs_project;
            DROP TABLE IF EXISTS quality_runs;
            DROP TABLE IF EXISTS quality_gates;
            DROP TABLE IF EXISTS quality_profiles;",
    }]
}

fn ts(t: Timestamp) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_ts(s: &str) -> StorageResult<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|e| StorageError::Corrupt(format!("bad timestamp {s}: {e}")))
}

/// Store over the shared database.
#[derive(Debug, Clone)]
pub struct QualityStore {
    db: Db,
}

impl QualityStore {
    /// New store.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Saved profile checks + save time.
    pub async fn profile(&self, project: ProjectId) -> StorageResult<Option<(Vec<CheckDef>, Timestamp)>> {
        self.db
            .call(move |c| {
                let row: Option<(String, String)> = c
                    .query_row(
                        "SELECT checks_json, updated_at FROM quality_profiles WHERE project_id = ?1",
                        [project.to_string()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                row.map(|(j, t)| Ok((serde_json::from_str(&j)?, parse_ts(&t)?))).transpose()
            })
            .await
    }

    /// Saves profile overrides.
    pub async fn save_profile(&self, project: ProjectId, checks: &[CheckDef], at: Timestamp) -> StorageResult<()> {
        let json = serde_json::to_string(checks)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO quality_profiles (project_id, checks_json, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(project_id) DO UPDATE SET checks_json = excluded.checks_json, updated_at = excluded.updated_at",
                    params![project.to_string(), json, ts(at)],
                )?;
                Ok(())
            })
            .await
    }

    /// Drops profile overrides.
    pub async fn delete_profile(&self, project: ProjectId) -> StorageResult<()> {
        self.db
            .call(move |c| {
                c.execute("DELETE FROM quality_profiles WHERE project_id = ?1", [project.to_string()])?;
                Ok(())
            })
            .await
    }

    /// Saved gate config.
    pub async fn gates(&self, project: ProjectId) -> StorageResult<Option<GateConfig>> {
        self.db
            .call(move |c| {
                let row: Option<String> = c
                    .query_row(
                        "SELECT config_json FROM quality_gates WHERE project_id = ?1",
                        [project.to_string()],
                        |r| r.get(0),
                    )
                    .optional()?;
                row.map(|j| serde_json::from_str(&j).map_err(StorageError::from)).transpose()
            })
            .await
    }

    /// Saves a gate config.
    pub async fn save_gates(&self, cfg: &GateConfig) -> StorageResult<()> {
        let json = serde_json::to_string(cfg)?;
        let project = cfg.project_id.to_string();
        let at = ts(cfg.updated_at.unwrap_or_else(orkestr_domain::now));
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO quality_gates (project_id, config_json, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(project_id) DO UPDATE SET config_json = excluded.config_json, updated_at = excluded.updated_at",
                    params![project, json, at],
                )?;
                Ok(())
            })
            .await
    }

    /// Deletes a gate config.
    pub async fn delete_gates(&self, project: ProjectId) -> StorageResult<()> {
        self.db
            .call(move |c| {
                c.execute("DELETE FROM quality_gates WHERE project_id = ?1", [project.to_string()])?;
                Ok(())
            })
            .await
    }

    /// Inserts or replaces a run; prunes old runs of the project.
    pub async fn save_run(&self, run: &QualityRun) -> StorageResult<()> {
        let json = serde_json::to_string(run)?;
        let id = run.id.clone();
        let project = run.project_id.to_string();
        let task = run.task_id.map(|t| t.to_string());
        let status = serde_json::to_value(run.status)?.as_str().unwrap_or("Error").to_owned();
        let started = ts(run.started_at);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO quality_runs (id, project_id, task_id, status, started_at, run_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, run_json = excluded.run_json",
                    params![id, project, task, status, started, json],
                )?;
                tx.execute(
                    "DELETE FROM quality_runs WHERE project_id = ?1 AND id NOT IN (
                        SELECT id FROM quality_runs WHERE project_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2)",
                    params![project, RUNS_KEPT_PER_PROJECT],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
    }

    /// One run.
    pub async fn run(&self, id: &str) -> StorageResult<Option<QualityRun>> {
        let id = id.to_owned();
        self.db
            .call(move |c| {
                let row: Option<String> =
                    c.query_row("SELECT run_json FROM quality_runs WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                row.map(|j| serde_json::from_str(&j).map_err(StorageError::from)).transpose()
            })
            .await
    }

    /// Runs of a project (optionally of one task), newest first.
    pub async fn runs(&self, project: ProjectId, task: Option<TaskId>, limit: u32) -> StorageResult<Vec<QualityRun>> {
        self.db
            .call(move |c| {
                let mut out = Vec::new();
                let mut stmt = c.prepare(
                    "SELECT run_json FROM quality_runs WHERE project_id = ?1 AND (?2 IS NULL OR task_id = ?2)
                     ORDER BY started_at DESC, id DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![project.to_string(), task.map(|t| t.to_string()), limit], |r| {
                    r.get::<_, String>(0)
                })?;
                for r in rows {
                    out.push(serde_json::from_str(&r?)?);
                }
                Ok(out)
            })
            .await
    }

    /// Marks runs left non-final by a previous session as cancelled.
    pub async fn interrupt_stale(&self, at: Timestamp) -> StorageResult<usize> {
        let stale: Vec<QualityRun> = self
            .db
            .call(|c| {
                let mut stmt = c.prepare("SELECT run_json FROM quality_runs WHERE status IN ('Queued', 'Running')")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(serde_json::from_str(&r?)?);
                }
                Ok(out)
            })
            .await?;
        let n = stale.len();
        for mut run in stale {
            for r in &mut run.results {
                if !r.status.is_final() {
                    r.status = CheckStatus::Cancelled;
                }
            }
            run.status = CheckStatus::Cancelled;
            run.finished_at = Some(at);
            self.save_run(&run).await?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::quality::RunTarget;

    async fn store() -> QualityStore {
        let db = Db::open_in_memory().unwrap();
        db.migrate(quality_migrations()).await.unwrap();
        QualityStore::new(db)
    }

    fn run(project: ProjectId, status: CheckStatus, minute: u32) -> QualityRun {
        QualityRun {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project,
            task_id: None,
            agent_id: None,
            target: RunTarget::Project,
            workspace: "/w".into(),
            kinds: vec![],
            status,
            results: vec![],
            started_at: chrono::DateTime::parse_from_rfc3339(&format!("2026-01-01T00:{minute:02}:00Z"))
                .unwrap()
                .with_timezone(&chrono::Utc),
            finished_at: None,
            duration_ms: 0,
        }
    }

    #[tokio::test]
    async fn runs_roundtrip_order_and_stale() {
        let s = store().await;
        let p = ProjectId::new();
        let a = run(p, CheckStatus::Passed, 1);
        let b = run(p, CheckStatus::Running, 2);
        s.save_run(&a).await.unwrap();
        s.save_run(&b).await.unwrap();
        let list = s.runs(p, None, 10).await.unwrap();
        assert_eq!(list.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![b.id.clone(), a.id.clone()]);
        assert_eq!(s.interrupt_stale(orkestr_domain::now()).await.unwrap(), 1);
        assert_eq!(s.run(&b.id).await.unwrap().unwrap().status, CheckStatus::Cancelled);
        assert!(s.run("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn profile_and_gates_roundtrip() {
        let s = store().await;
        let p = ProjectId::new();
        assert!(s.profile(p).await.unwrap().is_none());
        s.save_profile(p, &[], orkestr_domain::now()).await.unwrap();
        assert!(s.profile(p).await.unwrap().is_some());
        s.delete_profile(p).await.unwrap();
        assert!(s.profile(p).await.unwrap().is_none());
        let cfg = crate::gates::default_config(p);
        s.save_gates(&cfg).await.unwrap();
        assert_eq!(s.gates(p).await.unwrap().unwrap().gates.len(), cfg.gates.len());
    }
}
