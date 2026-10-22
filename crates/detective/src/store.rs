//! Persistence. Tables are prefixed `detective_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).

use orkestr_domain::{IncidentId, ProjectId};
use orkestr_protocol::methods::detective::Incident;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::{Deserialize, Serialize};

use crate::{stage, DetectiveError};

/// Migrations owned by this crate (owner `detective`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "detective_0001_incidents",
    owner: "detective",
    up: "CREATE TABLE detective_incidents (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL,
            stage TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            worktree TEXT,
            data TEXT NOT NULL
        );
        CREATE INDEX detective_incidents_project ON detective_incidents(project_id, created_at);
        CREATE TABLE detective_settings (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );",
    down: "DROP TABLE IF EXISTS detective_settings;
           DROP INDEX IF EXISTS detective_incidents_project;
           DROP TABLE IF EXISTS detective_incidents;",
}];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

/// Detective settings (stored in `detective_settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectiveSettings {
    /// Open an incident automatically on `AlertRaised` (default off).
    pub auto_open_on_alert: bool,
    /// Default budget cap per incident (USD).
    pub default_budget_usd: f64,
    /// Fix attempts when the fix's tests fail.
    pub max_fix_attempts: u32,
}

impl Default for DetectiveSettings {
    fn default() -> Self {
        Self { auto_open_on_alert: false, default_budget_usd: 2.0, max_fix_attempts: 2 }
    }
}

fn ser(e: serde_json::Error) -> DetectiveError {
    DetectiveError::Internal(format!("serialization: {e}"))
}

/// Incident rows.
#[derive(Debug, Clone)]
pub struct IncidentStore {
    db: Db,
}

impl IncidentStore {
    /// Store over a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces an incident.
    pub async fn put(&self, inc: &Incident) -> Result<(), DetectiveError> {
        let data = serde_json::to_string(inc).map_err(ser)?;
        let (id, project, st, created, updated) = (
            inc.id.to_string(),
            inc.project_id.to_string(),
            stage::name(inc.stage),
            inc.created_at.to_rfc3339(),
            inc.updated_at.to_rfc3339(),
        );
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO detective_incidents (id, project_id, stage, created_at, updated_at, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET stage = ?3, updated_at = ?5, data = ?6",
                    params![id, project, st, created, updated, data],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One incident.
    pub async fn get(&self, id: IncidentId) -> Result<Option<Incident>, DetectiveError> {
        let key = id.to_string();
        let row: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT data FROM detective_incidents WHERE id = ?1", params![key], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        row.map(|d| serde_json::from_str(&d).map_err(ser)).transpose()
    }

    /// Newest first.
    pub async fn list(
        &self,
        project: Option<ProjectId>,
        include_closed: bool,
        limit: u32,
    ) -> Result<Vec<Incident>, DetectiveError> {
        let project = project.map(|p| p.to_string());
        let limit = i64::from(limit.clamp(1, 500));
        let rows: Vec<String> = self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT data FROM detective_incidents
                     WHERE (?1 IS NULL OR project_id = ?1)
                       AND (?2 = 1 OR stage NOT IN ('Resolved', 'Failed', 'Cancelled'))
                     ORDER BY created_at DESC, id DESC LIMIT ?3",
                )?;
                let rows = st
                    .query_map(params![project, i64::from(include_closed), limit], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        rows.iter().map(|d| serde_json::from_str(d).map_err(ser)).collect()
    }

    /// Records the fix worktree path.
    pub async fn set_worktree(&self, id: IncidentId, path: Option<String>) -> Result<(), DetectiveError> {
        let key = id.to_string();
        self.db
            .call(move |c| {
                c.execute("UPDATE detective_incidents SET worktree = ?2 WHERE id = ?1", params![key, path])?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// The fix worktree path.
    pub async fn worktree(&self, id: IncidentId) -> Result<Option<String>, DetectiveError> {
        let key = id.to_string();
        let v: Option<Option<String>> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT worktree FROM detective_incidents WHERE id = ?1", params![key], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        Ok(v.flatten())
    }

    /// Settings (defaults when unset).
    pub async fn settings(&self) -> Result<DetectiveSettings, DetectiveError> {
        let v: Option<String> = self
            .db
            .call(|c| {
                Ok(c.query_row("SELECT value FROM detective_settings WHERE key = 'settings'", [], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        Ok(v.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    /// Saves settings.
    pub async fn set_settings(&self, s: &DetectiveSettings) -> Result<(), DetectiveError> {
        let v = serde_json::to_string(s).map_err(ser)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO detective_settings (key, value) VALUES ('settings', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = ?1",
                    params![v],
                )
                .map_err(StorageError::from)?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
