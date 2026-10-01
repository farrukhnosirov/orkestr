//! SQLite database handle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::Connection;

use crate::error::{StorageError, StorageResult};
use crate::migrations::{Migration, MigrationReport, Migrator};

/// Cloneable handle to one SQLite connection.
///
/// All access is serialized through a mutex and executed on Tokio's blocking
/// pool ([`Db::call`]), so async tasks never block on disk I/O. One writer
/// connection is the right model for SQLite in WAL mode at our scale; a read
/// pool can be added behind the same API later without touching callers.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Db {
    /// Opens (creating if needed) a database file with WAL and foreign keys on.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::configure(&conn, true)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: Some(path.to_path_buf()) })
    }

    /// Opens a private in-memory database (tests, ephemeral cores).
    pub fn open_in_memory() -> StorageResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn, false)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: None })
    }

    fn configure(conn: &Connection, wal: bool) -> StorageResult<()> {
        conn.busy_timeout(Duration::from_secs(5))?;
        if wal {
            let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
            if !mode.eq_ignore_ascii_case("wal") {
                tracing::warn!(%mode, "SQLite refused WAL journal mode");
            }
            conn.pragma_update(None, "synchronous", "NORMAL")?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    }

    /// Database file path (`None` for in-memory).
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Runs `f` with exclusive access to the connection on the blocking pool.
    pub async fn call<F, R>(&self, f: F) -> StorageResult<R>
    where
        F: FnOnce(&mut Connection) -> StorageResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock();
            f(&mut guard)
        })
        .await
        .map_err(|e| StorageError::Join(e.to_string()))?
    }

    /// Synchronous variant of [`Db::call`] for non-async contexts (startup,
    /// tests). Never call this from inside an async task.
    pub fn call_blocking<F, R>(&self, f: F) -> StorageResult<R>
    where
        F: FnOnce(&mut Connection) -> StorageResult<R>,
    {
        let mut guard = self.conn.lock();
        f(&mut guard)
    }

    /// Applies all pending migrations in order.
    pub async fn migrate(&self, migrations: Vec<Migration>) -> StorageResult<MigrationReport> {
        self.call(move |conn| Migrator::new(migrations).apply(conn)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_file_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("nested/orkestr.db")).unwrap();
        let mode: String = db.call(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = db.call(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn concurrent_calls_are_serialized() {
        let db = Db::open_in_memory().unwrap();
        db.call(|c| Ok(c.execute_batch("CREATE TABLE t(v INTEGER)")?)).await.unwrap();
        let mut handles = Vec::new();
        for i in 0..32 {
            let db = db.clone();
            handles.push(tokio::spawn(async move {
                db.call(move |c| Ok(c.execute("INSERT INTO t(v) VALUES (?1)", [i])?)).await
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let n: i64 = db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(n, 32);
    }
}
