//! Snapshot metadata in SQLite (`snapshots`, `snapshot_workspaces`).
//! File contents and manifests live in the object store.

use orkestr_domain::{ProjectId, SnapshotId, TaskId, WorkspaceId, WorkspaceSnapshot};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Migrations owned by `orkestr-snapshots`.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "snapshots_0001_init",
    owner: "snapshots",
    up: "CREATE TABLE snapshot_workspaces (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL,
            path_key TEXT NOT NULL,
            path TEXT NOT NULL,
            UNIQUE (project_id, path_key)
        );
        CREATE TABLE snapshots (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            workspace_key TEXT NOT NULL,
            task_id TEXT,
            task_phase TEXT,
            created_at TEXT NOT NULL,
            doc TEXT NOT NULL
        );
        CREATE INDEX snapshots_project ON snapshots(project_id, created_at);
        CREATE INDEX snapshots_task ON snapshots(task_id, created_at);",
    down: "DROP INDEX IF EXISTS snapshots_task;
           DROP INDEX IF EXISTS snapshots_project;
           DROP TABLE IF EXISTS snapshots;
           DROP TABLE IF EXISTS snapshot_workspaces;",
}];

/// Fixed-width UTC timestamp for correct lexicographic ordering.
fn ts_key(s: &WorkspaceSnapshot) -> String {
    s.created_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Snapshot repository.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    db: Db,
}

impl SnapshotStore {
    /// Wraps the database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Workspace id of `(project, path)`, created on first use.
    pub async fn workspace_id(&self, project_id: ProjectId, path_key: &str, path: &str) -> StorageResult<WorkspaceId> {
        let pid = project_id.to_string();
        let key = path_key.to_owned();
        let path = path.to_owned();
        self.db
            .call(move |c| {
                let existing: Option<String> = c
                    .query_row(
                        "SELECT id FROM snapshot_workspaces WHERE project_id = ?1 AND path_key = ?2",
                        params![pid, key],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = existing {
                    return WorkspaceId::parse(&id).map_err(|e| StorageError::Corrupt(e.to_string()));
                }
                let id = WorkspaceId::new();
                c.execute(
                    "INSERT INTO snapshot_workspaces (id, project_id, path_key, path) VALUES (?1, ?2, ?3, ?4)",
                    params![id.to_string(), pid, key, path],
                )?;
                Ok(id)
            })
            .await
    }

    /// Inserts a snapshot.
    pub async fn insert(&self, snap: &WorkspaceSnapshot, workspace_key: &str) -> StorageResult<()> {
        let doc = serde_json::to_string(snap)?;
        let row = (
            snap.id.to_string(),
            snap.project_id.to_string(),
            snap.workspace_id.to_string(),
            workspace_key.to_owned(),
            snap.task_id.map(|t| t.to_string()),
            snap.task_phase.map(|p| format!("{p:?}")),
            ts_key(snap),
            doc,
        );
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO snapshots (id, project_id, workspace_id, workspace_key, task_id, task_phase, created_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7],
                )?;
                Ok(())
            })
            .await
    }

    /// Loads one snapshot with its workspace key.
    pub async fn get(&self, id: SnapshotId) -> StorageResult<Option<(WorkspaceSnapshot, String)>> {
        let id = id.to_string();
        self.db
            .call(move |c| {
                let row: Option<(String, String)> = c
                    .query_row("SELECT doc, workspace_key FROM snapshots WHERE id = ?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                match row {
                    Some((doc, key)) => Ok(Some((serde_json::from_str(&doc)?, key))),
                    None => Ok(None),
                }
            })
            .await
    }

    /// Snapshots of a project, newest first.
    pub async fn list(
        &self,
        project_id: ProjectId,
        task_id: Option<TaskId>,
        workspace_key: Option<String>,
        limit: u32,
    ) -> StorageResult<Vec<WorkspaceSnapshot>> {
        let pid = project_id.to_string();
        let task = task_id.map(|t| t.to_string());
        self.db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM snapshots
                     WHERE project_id = ?1 AND (?2 IS NULL OR task_id = ?2) AND (?3 IS NULL OR workspace_key = ?3)
                     ORDER BY created_at DESC, id DESC LIMIT ?4",
                )?;
                let rows =
                    stmt.query_map(params![pid, task, workspace_key, i64::from(limit)], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for doc in rows {
                    out.push(serde_json::from_str(&doc?)?);
                }
                Ok(out)
            })
            .await
    }

    /// Deletes a snapshot row. Returns whether it existed.
    pub async fn delete(&self, id: SnapshotId) -> StorageResult<bool> {
        let id = id.to_string();
        self.db.call(move |c| Ok(c.execute("DELETE FROM snapshots WHERE id = ?1", [id])? > 0)).await
    }
}
