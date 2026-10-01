//! `MetadataStore` (§45 storage interfaces) and its SQLite implementation.
//!
//! Projects are stored as a JSON document plus a few indexed columns. Other
//! engines (tasks, agents, memory, …) own their tables and repositories in
//! their own crates, built on [`Db`] and contributing their own migrations.

use async_trait::async_trait;
use orkestr_domain::{Project, ProjectId};
use rusqlite::{params, OptionalExtension};

use crate::db::Db;
use crate::error::{StorageError, StorageResult};
use crate::migrations::Migration;

/// Migrations owned by the metadata store.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "storage_0001_projects",
    owner: "storage",
    up: "CREATE TABLE projects (
            id             TEXT PRIMARY KEY NOT NULL,
            name           TEXT NOT NULL,
            name_key       TEXT NOT NULL UNIQUE,
            archived       INTEGER NOT NULL DEFAULT 0,
            created_at     TEXT NOT NULL,
            updated_at     TEXT NOT NULL,
            last_opened_at TEXT,
            doc            TEXT NOT NULL
         );
         CREATE INDEX projects_last_opened ON projects(last_opened_at);",
    down: "DROP INDEX IF EXISTS projects_last_opened; DROP TABLE IF EXISTS projects;",
}];

/// Repository of projects.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Lists projects: most recently opened first, then by name.
    async fn list(&self, include_archived: bool) -> StorageResult<Vec<Project>>;
    /// Fetches one project.
    async fn get(&self, id: ProjectId) -> StorageResult<Option<Project>>;
    /// Inserts a new project. Names are unique case-insensitively → `Conflict`.
    async fn insert(&self, project: &Project) -> StorageResult<()>;
    /// Replaces an existing project → `NotFound` / `Conflict`.
    async fn update(&self, project: &Project) -> StorageResult<()>;
    /// Deletes a project. Returns whether it existed.
    async fn delete(&self, id: ProjectId) -> StorageResult<bool>;
}

/// Facade over all metadata repositories owned by the storage crate.
pub trait MetadataStore: Send + Sync {
    /// Project repository.
    fn projects(&self) -> &dyn ProjectRepository;
}

/// SQLite implementation of [`MetadataStore`].
#[derive(Debug, Clone)]
pub struct SqliteMetadataStore {
    db: Db,
}

impl SqliteMetadataStore {
    /// Wraps a migrated database (see [`MIGRATIONS`]).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Underlying database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }
}

impl MetadataStore for SqliteMetadataStore {
    fn projects(&self) -> &dyn ProjectRepository {
        self
    }
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn map_conflict(err: StorageError, name: &str) -> StorageError {
    if err.is_unique_violation() {
        StorageError::Conflict(format!("a project named \"{name}\" already exists"))
    } else {
        err
    }
}

#[async_trait]
impl ProjectRepository for SqliteMetadataStore {
    async fn list(&self, include_archived: bool) -> StorageResult<Vec<Project>> {
        self.db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM projects WHERE (?1 OR archived = 0)
                     ORDER BY last_opened_at IS NULL, last_opened_at DESC, name_key ASC",
                )?;
                let docs = stmt.query_map([include_archived], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for doc in docs {
                    out.push(serde_json::from_str(&doc?)?);
                }
                Ok(out)
            })
            .await
    }

    async fn get(&self, id: ProjectId) -> StorageResult<Option<Project>> {
        self.db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM projects WHERE id = ?1", [id.to_string()], |r| r.get(0)).optional()?;
                Ok(match doc {
                    Some(d) => Some(serde_json::from_str(&d)?),
                    None => None,
                })
            })
            .await
    }

    async fn insert(&self, project: &Project) -> StorageResult<()> {
        let p = project.clone();
        let doc = serde_json::to_string(&p)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO projects (id, name, name_key, archived, created_at, updated_at, last_opened_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        p.id.to_string(),
                        p.name,
                        name_key(&p.name),
                        p.archived,
                        p.created_at.to_rfc3339(),
                        p.updated_at.to_rfc3339(),
                        p.last_opened_at.map(|t| t.to_rfc3339()),
                        doc
                    ],
                )
                .map_err(|e| map_conflict(e.into(), &p.name))?;
                Ok(())
            })
            .await
    }

    async fn update(&self, project: &Project) -> StorageResult<()> {
        let p = project.clone();
        let doc = serde_json::to_string(&p)?;
        self.db
            .call(move |c| {
                let n = c
                    .execute(
                        "UPDATE projects SET name = ?2, name_key = ?3, archived = ?4, updated_at = ?5,
                         last_opened_at = ?6, doc = ?7 WHERE id = ?1",
                        params![
                            p.id.to_string(),
                            p.name,
                            name_key(&p.name),
                            p.archived,
                            p.updated_at.to_rfc3339(),
                            p.last_opened_at.map(|t| t.to_rfc3339()),
                            doc
                        ],
                    )
                    .map_err(|e| map_conflict(e.into(), &p.name))?;
                if n == 0 {
                    return Err(StorageError::NotFound(format!("project {}", p.id)));
                }
                Ok(())
            })
            .await
    }

    async fn delete(&self, id: ProjectId) -> StorageResult<bool> {
        self.db.call(move |c| Ok(c.execute("DELETE FROM projects WHERE id = ?1", [id.to_string()])? > 0)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orkestr_domain::now;

    async fn store() -> (tempfile::TempDir, SqliteMetadataStore) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.db")).unwrap();
        db.migrate(MIGRATIONS.to_vec()).await.unwrap();
        (dir, SqliteMetadataStore::new(db))
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let (_d, s) = store().await;
        let repo = s.projects();
        let mut p = Project::new("Alpha", now());
        repo.insert(&p).await.unwrap();
        assert_eq!(repo.get(p.id).await.unwrap().unwrap(), p);
        p.description = "changed".into();
        repo.update(&p).await.unwrap();
        assert_eq!(repo.get(p.id).await.unwrap().unwrap().description, "changed");
        assert!(repo.delete(p.id).await.unwrap());
        assert!(!repo.delete(p.id).await.unwrap());
        assert!(repo.get(p.id).await.unwrap().is_none());
        assert!(matches!(repo.update(&p).await, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn names_unique_case_insensitive() {
        let (_d, s) = store().await;
        s.insert(&Project::new("Alpha", now())).await.unwrap();
        let err = s.insert(&Project::new(" alpha ", now())).await.unwrap_err();
        assert!(matches!(err, StorageError::Conflict(_)), "{err}");
    }

    #[tokio::test]
    async fn list_orders_and_filters() {
        let (_d, s) = store().await;
        let t = now();
        let a = Project::new("Bravo", t);
        let mut b = Project::new("Alpha", t);
        let mut c = Project::new("Charlie", t);
        c.last_opened_at = Some(t + Duration::seconds(10));
        b.archived = true;
        for p in [&a, &b, &c] {
            s.insert(p).await.unwrap();
        }
        let names: Vec<_> = s.list(false).await.unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Charlie", "Bravo"]);
        let names: Vec<_> = s.list(true).await.unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Charlie", "Alpha", "Bravo"]);
    }
}
