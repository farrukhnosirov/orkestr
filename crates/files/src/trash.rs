//! Trash: copies of deleted files kept in the content-addressed object
//! store, indexed in SQLite (`files_trash`), restorable with `files.restore`
//! (§60.8 — no silent destructive actions).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use orkestr_domain::{AgentId, ProjectId, Timestamp};
use orkestr_protocol::event::FileChangeActor;
use orkestr_protocol::methods::files::TrashItem;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, ObjectId, ObjectStore, StorageError};
use parking_lot::Mutex;

use crate::error::{FilesError, FilesResult};

/// Migrations owned by this crate (compose after core/storage/events).
pub const FILES_MIGRATIONS: &[Migration] = &[Migration {
    id: "files_0001_trash",
    owner: "files",
    up: "CREATE TABLE files_trash (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL,
            root TEXT NOT NULL,
            path TEXT NOT NULL,
            object_id TEXT NOT NULL,
            size INTEGER NOT NULL,
            actor TEXT NOT NULL,
            agent_id TEXT,
            deleted_at TEXT NOT NULL,
            restored_at TEXT
         );
         CREATE INDEX files_trash_project ON files_trash (project_id, deleted_at);",
    down: "DROP INDEX IF EXISTS files_trash_project; DROP TABLE IF EXISTS files_trash;",
}];

/// Migrations of this crate.
#[must_use]
pub fn files_migrations() -> Vec<Migration> {
    FILES_MIGRATIONS.to_vec()
}

/// A file about to be trashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrashEntry {
    /// Project.
    pub project_id: ProjectId,
    /// Canonical workspace root the file lived in.
    pub root: String,
    /// Workspace-relative path.
    pub path: String,
    /// Who deleted it.
    pub actor: FileChangeActor,
    /// Agent, when an agent deleted it.
    pub agent_id: Option<AgentId>,
}

/// A stored trash entry with its location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRecord {
    /// Wire item.
    pub item: TrashItem,
    /// Project.
    pub project_id: ProjectId,
    /// Canonical workspace root.
    pub root: String,
}

/// Where deleted file copies go.
#[async_trait]
pub trait TrashStore: Send + Sync {
    /// Stores a copy; returns the entry.
    async fn stash(&self, entry: NewTrashEntry, bytes: Vec<u8>) -> FilesResult<TrashItem>;
    /// Newest entries of a project.
    async fn list(&self, project_id: ProjectId, limit: u32) -> FilesResult<Vec<TrashItem>>;
    /// Entry + content.
    async fn fetch(&self, id: &str) -> FilesResult<Option<(TrashRecord, Vec<u8>)>>;
    /// Marks an entry restored.
    async fn mark_restored(&self, id: &str) -> FilesResult<()>;
}

fn actor_str(a: FileChangeActor) -> &'static str {
    match a {
        FileChangeActor::User => "user",
        FileChangeActor::Agent => "agent",
        FileChangeActor::External => "external",
    }
}

fn parse_actor(s: &str) -> FileChangeActor {
    match s {
        "user" => FileChangeActor::User,
        "agent" => FileChangeActor::Agent,
        _ => FileChangeActor::External,
    }
}

/// SQLite + object store implementation.
pub struct StorageTrash {
    db: Db,
    objects: Arc<dyn ObjectStore>,
}

impl std::fmt::Debug for StorageTrash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageTrash").finish_non_exhaustive()
    }
}

impl StorageTrash {
    /// New store (the `files_trash` migration must be applied).
    #[must_use]
    pub fn new(db: Db, objects: Arc<dyn ObjectStore>) -> Self {
        Self { db, objects }
    }
}

struct Row {
    id: String,
    project_id: String,
    root: String,
    path: String,
    object_id: String,
    size: i64,
    actor: String,
    deleted_at: String,
    restored: bool,
}

impl Row {
    fn into_record(self) -> FilesResult<(TrashRecord, String)> {
        let project_id = ProjectId::parse(&self.project_id).map_err(|e| FilesError::Storage(e.to_string()))?;
        let deleted_at: Timestamp = chrono::DateTime::parse_from_rfc3339(&self.deleted_at)
            .map_err(|e| FilesError::Storage(e.to_string()))?
            .with_timezone(&Utc);
        Ok((
            TrashRecord {
                item: TrashItem {
                    id: self.id,
                    path: self.path,
                    size: u64::try_from(self.size).unwrap_or(0),
                    deleted_at,
                    actor: parse_actor(&self.actor),
                    restored: self.restored,
                },
                project_id,
                root: self.root,
            },
            self.object_id,
        ))
    }
}

const SELECT: &str =
    "SELECT id, project_id, root, path, object_id, size, actor, deleted_at, restored_at IS NOT NULL FROM files_trash";

fn map_row(r: &orkestr_storage::rusqlite::Row<'_>) -> orkestr_storage::rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        project_id: r.get(1)?,
        root: r.get(2)?,
        path: r.get(3)?,
        object_id: r.get(4)?,
        size: r.get(5)?,
        actor: r.get(6)?,
        deleted_at: r.get(7)?,
        restored: r.get(8)?,
    })
}

#[async_trait]
impl TrashStore for StorageTrash {
    async fn stash(&self, entry: NewTrashEntry, bytes: Vec<u8>) -> FilesResult<TrashItem> {
        let size = bytes.len() as u64;
        let object_id = self.objects.put(bytes).await?;
        let item = TrashItem {
            id: uuid::Uuid::now_v7().to_string(),
            path: entry.path.clone(),
            size,
            deleted_at: Utc::now(),
            actor: entry.actor,
            restored: false,
        };
        let (id, ts) = (item.id.clone(), item.deleted_at.to_rfc3339());
        let oid = object_id.as_str().to_owned();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO files_trash (id, project_id, root, path, object_id, size, actor, agent_id, deleted_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        id,
                        entry.project_id.to_string(),
                        entry.root,
                        entry.path,
                        oid,
                        i64::try_from(size).unwrap_or(i64::MAX),
                        actor_str(entry.actor),
                        entry.agent_id.map(|a| a.to_string()),
                        ts
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(item)
    }

    async fn list(&self, project_id: ProjectId, limit: u32) -> FilesResult<Vec<TrashItem>> {
        let rows = self
            .db
            .call(move |c| {
                let mut stmt =
                    c.prepare(&format!("{SELECT} WHERE project_id = ?1 ORDER BY deleted_at DESC, id DESC LIMIT ?2"))?;
                let rows = stmt
                    .query_map(params![project_id.to_string(), i64::from(limit)], map_row)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        rows.into_iter().map(|r| r.into_record().map(|(rec, _)| rec.item)).collect()
    }

    async fn fetch(&self, id: &str) -> FilesResult<Option<(TrashRecord, Vec<u8>)>> {
        let id = id.to_owned();
        let row = self
            .db
            .call(move |c| Ok(c.query_row(&format!("{SELECT} WHERE id = ?1"), params![id], map_row).optional()?))
            .await?;
        let Some(row) = row else { return Ok(None) };
        let (record, oid) = row.into_record()?;
        let object_id = ObjectId::parse(&oid)?;
        let bytes =
            self.objects.get(&object_id).await?.ok_or_else(|| {
                FilesError::Storage(StorageError::NotFound(format!("trash object {oid}")).to_string())
            })?;
        Ok(Some((record, bytes)))
    }

    async fn mark_restored(&self, id: &str) -> FilesResult<()> {
        let id = id.to_owned();
        let ts = Utc::now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute("UPDATE files_trash SET restored_at = ?2 WHERE id = ?1", params![id, ts])?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}

/// In-memory trash (tests and embedders without storage).
#[derive(Debug, Default)]
pub struct MemoryTrash {
    entries: Mutex<Vec<(TrashRecord, Vec<u8>)>>,
}

impl MemoryTrash {
    /// Empty trash.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TrashStore for MemoryTrash {
    async fn stash(&self, entry: NewTrashEntry, bytes: Vec<u8>) -> FilesResult<TrashItem> {
        let item = TrashItem {
            id: uuid::Uuid::now_v7().to_string(),
            path: entry.path,
            size: bytes.len() as u64,
            deleted_at: Utc::now(),
            actor: entry.actor,
            restored: false,
        };
        self.entries
            .lock()
            .push((TrashRecord { item: item.clone(), project_id: entry.project_id, root: entry.root }, bytes));
        Ok(item)
    }

    async fn list(&self, project_id: ProjectId, limit: u32) -> FilesResult<Vec<TrashItem>> {
        let entries = self.entries.lock();
        Ok(entries
            .iter()
            .rev()
            .filter(|(r, _)| r.project_id == project_id)
            .take(limit as usize)
            .map(|(r, _)| r.item.clone())
            .collect())
    }

    async fn fetch(&self, id: &str) -> FilesResult<Option<(TrashRecord, Vec<u8>)>> {
        Ok(self.entries.lock().iter().find(|(r, _)| r.item.id == id).cloned())
    }

    async fn mark_restored(&self, id: &str) -> FilesResult<()> {
        let mut entries = self.entries.lock();
        let by_id: HashMap<_, _> = entries.iter().enumerate().map(|(i, (r, _))| (r.item.id.clone(), i)).collect();
        if let Some(&i) = by_id.get(id) {
            entries[i].0.item.restored = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_storage::{Compression, FsObjectStore};

    #[tokio::test]
    async fn storage_trash_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.db")).unwrap();
        let mut migs = orkestr_storage::storage_migrations();
        migs.extend(files_migrations());
        db.migrate(migs).await.unwrap();
        let objects: Arc<dyn ObjectStore> =
            Arc::new(FsObjectStore::open(dir.path().join("objects"), Compression::Zstd { level: 3 }).unwrap());
        let trash = StorageTrash::new(db, objects);
        let pid = ProjectId::new();
        let mk = |path: &str| NewTrashEntry {
            project_id: pid,
            root: "/w".into(),
            path: path.into(),
            actor: FileChangeActor::Agent,
            agent_id: Some(AgentId::new()),
        };
        let a = trash.stash(mk("a.txt"), b"alpha".to_vec()).await.unwrap();
        let b = trash.stash(mk("dir/b.txt"), b"beta".to_vec()).await.unwrap();
        assert_eq!(a.size, 5);
        let list = trash.list(pid, 10).await.unwrap();
        assert_eq!(list.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), vec!["dir/b.txt", "a.txt"]);
        assert!(trash.list(ProjectId::new(), 10).await.unwrap().is_empty());
        let (rec, bytes) = trash.fetch(&b.id).await.unwrap().unwrap();
        assert_eq!(bytes, b"beta");
        assert_eq!(rec.root, "/w");
        assert_eq!(rec.item.actor, FileChangeActor::Agent);
        assert!(!rec.item.restored);
        trash.mark_restored(&b.id).await.unwrap();
        assert!(trash.fetch(&b.id).await.unwrap().unwrap().0.item.restored);
        assert!(trash.fetch("missing").await.unwrap().is_none());
    }
}
