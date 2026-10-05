//! Persistence of architecture snapshots (table `architecture_snapshots`).

use orkestr_domain::ProjectId;
use orkestr_protocol::methods::architecture::{ArchSnapshotInfo, ArchitectureMap, MAX_ARCH_SNAPSHOTS};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration};

use crate::error::{InsightError, InsightResult};

/// Migrations owned by the insight crate.
#[must_use]
pub fn insight_migrations() -> Vec<Migration> {
    vec![Migration {
        id: "insight_0001_architecture_snapshots",
        owner: "insight",
        up: "CREATE TABLE architecture_snapshots (
                id           TEXT PRIMARY KEY NOT NULL,
                project_id   TEXT NOT NULL,
                generated_at TEXT NOT NULL,
                digest       TEXT NOT NULL,
                stats        TEXT NOT NULL,
                doc          TEXT NOT NULL
             );
             CREATE INDEX architecture_snapshots_project ON architecture_snapshots(project_id, generated_at);",
        down: "DROP INDEX IF EXISTS architecture_snapshots_project; DROP TABLE IF EXISTS architecture_snapshots;",
    }]
}

/// Snapshot store.
#[derive(Debug, Clone)]
pub struct ArchStore {
    db: Db,
}

impl ArchStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Stores a snapshot and prunes the oldest beyond [`MAX_ARCH_SNAPSHOTS`].
    pub async fn insert(&self, map: &ArchitectureMap) -> InsightResult<()> {
        let doc = serde_json::to_string(map)?;
        let stats = serde_json::to_string(&map.stats)?;
        let row = (
            map.snapshot_id.clone(),
            map.project_id.to_string(),
            map.generated_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            map.digest.clone(),
            stats,
            doc,
        );
        let keep = i64::try_from(MAX_ARCH_SNAPSHOTS).unwrap_or(50);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO architecture_snapshots (id, project_id, generated_at, digest, stats, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![row.0, row.1, row.2, row.3, row.4, row.5],
                )?;
                tx.execute(
                    "DELETE FROM architecture_snapshots WHERE project_id = ?1 AND id NOT IN (
                        SELECT id FROM architecture_snapshots WHERE project_id = ?1
                        ORDER BY generated_at DESC, id DESC LIMIT ?2)",
                    params![row.1, keep],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Latest snapshot of a project.
    pub async fn latest(&self, project: ProjectId) -> InsightResult<Option<ArchitectureMap>> {
        let pid = project.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT doc FROM architecture_snapshots WHERE project_id = ?1
                     ORDER BY generated_at DESC, id DESC LIMIT 1",
                    [pid],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .await?;
        doc.map(|d| serde_json::from_str(&d).map_err(InsightError::from)).transpose()
    }

    /// One snapshot (must belong to the project).
    pub async fn get(&self, project: ProjectId, id: &str) -> InsightResult<ArchitectureMap> {
        let pid = project.to_string();
        let sid = id.to_owned();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT doc FROM architecture_snapshots WHERE project_id = ?1 AND id = ?2",
                    [pid, sid],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .await?;
        let doc = doc.ok_or_else(|| InsightError::NotFound(format!("architecture snapshot {id}")))?;
        Ok(serde_json::from_str(&doc)?)
    }

    /// Snapshots of a project, newest first.
    pub async fn list(&self, project: ProjectId) -> InsightResult<Vec<ArchSnapshotInfo>> {
        let pid = project.to_string();
        let rows: Vec<(String, String, String, String)> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT id, generated_at, digest, stats FROM architecture_snapshots WHERE project_id = ?1
                     ORDER BY generated_at DESC, id DESC",
                )?;
                let rows = stmt.query_map([pid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        rows.into_iter()
            .map(|(id, at, digest, stats)| {
                Ok(ArchSnapshotInfo {
                    snapshot_id: id,
                    generated_at: chrono::DateTime::parse_from_rfc3339(&at)
                        .map_err(|e| InsightError::Invalid(format!("bad timestamp: {e}")))?
                        .with_timezone(&chrono::Utc),
                    digest,
                    stats: serde_json::from_str(&stats)?,
                })
            })
            .collect()
    }

    /// Whether a project has any snapshot.
    pub async fn has_any(&self, project: ProjectId) -> InsightResult<bool> {
        let pid = project.to_string();
        let n: i64 = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT COUNT(*) FROM architecture_snapshots WHERE project_id = ?1", [pid], |r| {
                    r.get(0)
                })?)
            })
            .await?;
        Ok(n > 0)
    }

    /// Deletes every snapshot of a project.
    pub async fn delete_project(&self, project: ProjectId) -> InsightResult<u64> {
        let pid = project.to_string();
        let n = self
            .db
            .call(move |c| Ok(c.execute("DELETE FROM architecture_snapshots WHERE project_id = ?1", [pid])?))
            .await?;
        Ok(n as u64)
    }
}
