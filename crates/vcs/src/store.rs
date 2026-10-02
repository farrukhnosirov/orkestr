//! Persistence of Orkestr-managed worktrees (`vcs_worktrees`).
//!
//! Orkestr only ever removes worktrees/branches it recorded here
//! (git-worktree-strategy §2); rows also carry agent/task metadata that git
//! does not know about.

use orkestr_domain::{AgentId, ProjectId, TaskId, Timestamp};
use orkestr_storage::rusqlite::{params, Row};
use orkestr_storage::{Db, Migration, StorageResult};

/// Migrations owned by `orkestr-vcs`.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "vcs_0001_worktrees",
    owner: "vcs",
    up: "CREATE TABLE vcs_worktrees (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL,
            path TEXT NOT NULL,
            path_key TEXT NOT NULL,
            branch TEXT NOT NULL,
            agent_id TEXT,
            task_id TEXT,
            role TEXT,
            base_ref TEXT,
            base_commit TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            removed_at TEXT
        );
        CREATE INDEX vcs_worktrees_project ON vcs_worktrees(project_id, state);
        CREATE INDEX vcs_worktrees_path ON vcs_worktrees(path_key);",
    down: "DROP INDEX IF EXISTS vcs_worktrees_path;
           DROP INDEX IF EXISTS vcs_worktrees_project;
           DROP TABLE IF EXISTS vcs_worktrees;",
}];

/// Lifecycle state of a managed worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
    /// In use.
    Active,
    /// Removed from disk.
    Removed,
}

impl WorktreeState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Removed => "Removed",
        }
    }
}

/// A managed worktree row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    /// Row id (uuid).
    pub id: String,
    /// Project.
    pub project_id: ProjectId,
    /// Absolute path.
    pub path: String,
    /// Normalized comparison key of `path`.
    pub path_key: String,
    /// Branch.
    pub branch: String,
    /// Owning agent.
    pub agent_id: Option<AgentId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Role slug.
    pub role: Option<String>,
    /// Base ref as requested.
    pub base_ref: Option<String>,
    /// Base commit.
    pub base_commit: String,
    /// State.
    pub state: WorktreeState,
    /// Created.
    pub created_at: Timestamp,
}

fn conv_err(col: &str, e: impl std::fmt::Display) -> orkestr_storage::rusqlite::Error {
    orkestr_storage::rusqlite::Error::FromSqlConversionFailure(
        0,
        orkestr_storage::rusqlite::types::Type::Text,
        format!("vcs_worktrees.{col}: {e}").into(),
    )
}

fn from_row(r: &Row<'_>) -> orkestr_storage::rusqlite::Result<WorktreeRecord> {
    let project: String = r.get("project_id")?;
    let created: String = r.get("created_at")?;
    let state: String = r.get("state")?;
    let agent: Option<String> = r.get("agent_id")?;
    let task: Option<String> = r.get("task_id")?;
    Ok(WorktreeRecord {
        id: r.get("id")?,
        project_id: ProjectId::parse(&project).map_err(|e| conv_err("project_id", e))?,
        path: r.get("path")?,
        path_key: r.get("path_key")?,
        branch: r.get("branch")?,
        agent_id: agent.map(|a| AgentId::parse(&a)).transpose().map_err(|e| conv_err("agent_id", e))?,
        task_id: task.map(|t| TaskId::parse(&t)).transpose().map_err(|e| conv_err("task_id", e))?,
        role: r.get("role")?,
        base_ref: r.get("base_ref")?,
        base_commit: r.get("base_commit")?,
        state: if state == "Removed" { WorktreeState::Removed } else { WorktreeState::Active },
        created_at: chrono::DateTime::parse_from_rfc3339(&created)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|e| conv_err("created_at", e))?,
    })
}

/// Repository of managed worktrees.
#[derive(Debug, Clone)]
pub struct VcsStore {
    db: Db,
}

impl VcsStore {
    /// Wraps the database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts a record.
    pub async fn insert(&self, rec: &WorktreeRecord) -> StorageResult<()> {
        let rec = rec.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO vcs_worktrees (id, project_id, path, path_key, branch, agent_id, task_id, role, base_ref, base_commit, state, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        rec.id,
                        rec.project_id.to_string(),
                        rec.path,
                        rec.path_key,
                        rec.branch,
                        rec.agent_id.map(|a| a.to_string()),
                        rec.task_id.map(|t| t.to_string()),
                        rec.role,
                        rec.base_ref,
                        rec.base_commit,
                        rec.state.as_str(),
                        rec.created_at.to_rfc3339(),
                    ],
                )?;
                Ok(())
            })
            .await
    }

    /// Active records of a project (oldest first).
    pub async fn active(&self, project_id: ProjectId) -> StorageResult<Vec<WorktreeRecord>> {
        let pid = project_id.to_string();
        self.db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT * FROM vcs_worktrees WHERE project_id = ?1 AND state = 'Active' ORDER BY created_at",
                )?;
                let rows = stmt.query_map([pid], from_row)?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    /// Marks the active record with this path key as removed.
    pub async fn mark_removed(&self, project_id: ProjectId, path_key: &str) -> StorageResult<bool> {
        let pid = project_id.to_string();
        let key = path_key.to_owned();
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |c| {
                let n = c.execute(
                    "UPDATE vcs_worktrees SET state = 'Removed', removed_at = ?3 WHERE project_id = ?1 AND path_key = ?2 AND state = 'Active'",
                    params![pid, key, now],
                )?;
                Ok(n > 0)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_list_remove_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.migrate(MIGRATIONS.to_vec()).await.unwrap();
        let store = VcsStore::new(db);
        let project_id = ProjectId::new();
        let rec = WorktreeRecord {
            id: "w1".into(),
            project_id,
            path: "/data/worktrees/p/a".into(),
            path_key: "/data/worktrees/p/a".into(),
            branch: "agent/qa-12345678".into(),
            agent_id: Some(AgentId::new()),
            task_id: None,
            role: Some("qa".into()),
            base_ref: Some("main".into()),
            base_commit: "abc".into(),
            state: WorktreeState::Active,
            created_at: chrono::Utc::now(),
        };
        store.insert(&rec).await.unwrap();
        let got = store.active(project_id).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].agent_id, rec.agent_id);
        assert!(store.mark_removed(project_id, "/data/worktrees/p/a").await.unwrap());
        assert!(store.active(project_id).await.unwrap().is_empty());
    }
}
