//! Ordered, named, reversible schema migrations (contract §3, spec §60.10).
//!
//! * Each migration has a globally unique `id` (convention:
//!   `<owner>_<NNNN>_<description>`, e.g. `storage_0001_projects`), an `owner`
//!   crate name, `up` SQL and `down` SQL.
//! * Applied migrations are recorded in `_orkestr_migrations` together with a
//!   blake3 checksum of `up` and the `down` SQL, so rollbacks work even after
//!   the code that introduced a migration is gone.
//! * Editing an applied migration is detected (checksum drift) and refused.
//! * A database containing applied migrations unknown to this binary (written
//!   by a newer version) is refused to protect data (§70 priority 1).
//! * Each migration runs in its own transaction.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{StorageError, StorageResult};

/// A schema migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    /// Globally unique id.
    pub id: &'static str,
    /// Owning crate (`storage`, `events`, `task-engine`, …).
    pub owner: &'static str,
    /// Forward SQL (may contain several statements).
    pub up: &'static str,
    /// Reverse SQL. Must undo `up` completely.
    pub down: &'static str,
}

impl Migration {
    /// blake3 checksum of the `up` SQL (hex).
    #[must_use]
    pub fn checksum(&self) -> String {
        blake3::hash(self.up.as_bytes()).to_hex().to_string()
    }
}

/// Outcome of [`Migrator::apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Ids applied by this call, in order.
    pub applied: Vec<String>,
    /// Ids that were already applied.
    pub already_applied: Vec<String>,
}

/// Status row of an applied migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    /// Order of application.
    pub seq: i64,
    /// Id.
    pub id: String,
    /// Owner.
    pub owner: String,
    /// Checksum of `up` at application time.
    pub checksum: String,
    /// RFC 3339 timestamp.
    pub applied_at: String,
}

const BOOKKEEPING: &str = "CREATE TABLE IF NOT EXISTS _orkestr_migrations (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    id         TEXT NOT NULL UNIQUE,
    owner      TEXT NOT NULL,
    checksum   TEXT NOT NULL,
    down_sql   TEXT NOT NULL,
    applied_at TEXT NOT NULL
);";

/// Applies and reverts migrations.
#[derive(Debug, Clone)]
pub struct Migrator {
    migrations: Vec<Migration>,
}

impl Migrator {
    /// Creates a migrator for an ordered list of migrations.
    #[must_use]
    pub fn new(migrations: Vec<Migration>) -> Self {
        Self { migrations }
    }

    fn validate(&self) -> StorageResult<()> {
        let mut seen = std::collections::HashSet::new();
        for m in &self.migrations {
            if m.id.trim().is_empty() || m.owner.trim().is_empty() {
                return Err(StorageError::Migration {
                    id: m.id.into(),
                    message: "id and owner must be non-empty".into(),
                });
            }
            if !seen.insert(m.id) {
                return Err(StorageError::Migration { id: m.id.into(), message: "duplicate migration id".into() });
            }
            if m.down.trim().is_empty() {
                return Err(StorageError::Migration {
                    id: m.id.into(),
                    message: "down SQL is required (migrations must be reversible)".into(),
                });
            }
        }
        Ok(())
    }

    /// Lists applied migrations in application order.
    pub fn applied(conn: &Connection) -> StorageResult<Vec<AppliedMigration>> {
        conn.execute_batch(BOOKKEEPING)?;
        let mut stmt =
            conn.prepare("SELECT seq, id, owner, checksum, applied_at FROM _orkestr_migrations ORDER BY seq")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AppliedMigration {
                    seq: r.get(0)?,
                    id: r.get(1)?,
                    owner: r.get(2)?,
                    checksum: r.get(3)?,
                    applied_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Applies every pending migration in list order.
    pub fn apply(&self, conn: &mut Connection) -> StorageResult<MigrationReport> {
        self.validate()?;
        let applied = Self::applied(conn)?;
        let known: std::collections::HashSet<&str> = self.migrations.iter().map(|m| m.id).collect();
        if let Some(unknown) = applied.iter().find(|a| !known.contains(a.id.as_str())) {
            return Err(StorageError::Migration {
                id: unknown.id.clone(),
                message: "database contains a migration unknown to this build (written by a newer Orkestr?); \
                          refusing to continue to protect data"
                    .into(),
            });
        }
        let mut report = MigrationReport::default();
        for m in &self.migrations {
            if let Some(a) = applied.iter().find(|a| a.id == m.id) {
                if a.checksum != m.checksum() {
                    return Err(StorageError::Migration {
                        id: m.id.into(),
                        message: "applied migration was modified (checksum mismatch); add a new migration instead"
                            .into(),
                    });
                }
                report.already_applied.push(m.id.into());
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(m.up).map_err(|e| StorageError::Migration { id: m.id.into(), message: e.to_string() })?;
            tx.execute(
                "INSERT INTO _orkestr_migrations (id, owner, checksum, down_sql, applied_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![m.id, m.owner, m.checksum(), m.down, chrono::Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
            tracing::info!(id = m.id, owner = m.owner, "applied migration");
            report.applied.push(m.id.into());
        }
        Ok(report)
    }

    /// Reverts the last `n` applied migrations (newest first) using their stored
    /// `down` SQL. Returns the reverted ids.
    pub fn rollback_last(conn: &mut Connection, n: usize) -> StorageResult<Vec<String>> {
        let mut reverted = Vec::new();
        for _ in 0..n {
            conn.execute_batch(BOOKKEEPING)?;
            let last: Option<(i64, String, String)> = conn
                .query_row("SELECT seq, id, down_sql FROM _orkestr_migrations ORDER BY seq DESC LIMIT 1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let Some((seq, id, down)) = last else { break };
            let tx = conn.transaction()?;
            tx.execute_batch(&down).map_err(|e| StorageError::Migration { id: id.clone(), message: e.to_string() })?;
            tx.execute("DELETE FROM _orkestr_migrations WHERE seq = ?1", [seq])?;
            tx.commit()?;
            tracing::warn!(%id, "reverted migration");
            reverted.push(id);
        }
        Ok(reverted)
    }

    /// Reverts migrations newer than `id` (exclusive) — i.e. after this call
    /// `id` is the newest applied migration.
    pub fn rollback_to(conn: &mut Connection, id: &str) -> StorageResult<Vec<String>> {
        let applied = Self::applied(conn)?;
        let Some(pos) = applied.iter().position(|a| a.id == id) else {
            return Err(StorageError::NotFound(format!("migration {id}")));
        };
        Self::rollback_last(conn, applied.len() - pos - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Migration = Migration {
        id: "test_0001_a",
        owner: "test",
        up: "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT);",
        down: "DROP TABLE a;",
    };
    const B: Migration = Migration {
        id: "test_0002_b",
        owner: "test",
        up: "CREATE TABLE b (id INTEGER PRIMARY KEY); CREATE INDEX b_idx ON b(id);",
        down: "DROP INDEX b_idx; DROP TABLE b;",
    };

    fn tables(conn: &Connection) -> Vec<String> {
        let mut s = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn apply_is_ordered_and_idempotent() {
        let mut c = Connection::open_in_memory().unwrap();
        let r = Migrator::new(vec![A, B]).apply(&mut c).unwrap();
        assert_eq!(r.applied, vec!["test_0001_a", "test_0002_b"]);
        let r = Migrator::new(vec![A, B]).apply(&mut c).unwrap();
        assert!(r.applied.is_empty());
        assert_eq!(r.already_applied.len(), 2);
        assert_eq!(tables(&c), vec!["_orkestr_migrations", "a", "b"]);
        let applied = Migrator::applied(&c).unwrap();
        assert_eq!(applied[0].id, "test_0001_a");
        assert!(applied[0].seq < applied[1].seq);
    }

    #[test]
    fn rollback_reverses_everything() {
        let mut c = Connection::open_in_memory().unwrap();
        Migrator::new(vec![A, B]).apply(&mut c).unwrap();
        let reverted = Migrator::rollback_last(&mut c, 1).unwrap();
        assert_eq!(reverted, vec!["test_0002_b"]);
        assert_eq!(tables(&c), vec!["_orkestr_migrations", "a"]);
        // Re-apply after rollback works.
        Migrator::new(vec![A, B]).apply(&mut c).unwrap();
        let reverted = Migrator::rollback_to(&mut c, "test_0001_a").unwrap();
        assert_eq!(reverted, vec!["test_0002_b"]);
        Migrator::rollback_last(&mut c, 10).unwrap();
        assert_eq!(tables(&c), vec!["_orkestr_migrations"]);
    }

    #[test]
    fn checksum_drift_is_refused() {
        let mut c = Connection::open_in_memory().unwrap();
        Migrator::new(vec![A]).apply(&mut c).unwrap();
        let edited = Migration { up: "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT, w TEXT);", ..A };
        let err = Migrator::new(vec![edited]).apply(&mut c).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
    }

    #[test]
    fn unknown_applied_migration_is_refused() {
        let mut c = Connection::open_in_memory().unwrap();
        Migrator::new(vec![A, B]).apply(&mut c).unwrap();
        let err = Migrator::new(vec![A]).apply(&mut c).unwrap_err();
        assert!(err.to_string().contains("unknown"), "{err}");
    }

    #[test]
    fn failing_migration_rolls_back_its_transaction() {
        let mut c = Connection::open_in_memory().unwrap();
        let bad = Migration {
            id: "test_0003_bad",
            owner: "test",
            up: "CREATE TABLE c (x); SELEC nonsense;",
            down: "DROP TABLE c;",
        };
        let err = Migrator::new(vec![A, bad]).apply(&mut c).unwrap_err();
        assert!(matches!(err, StorageError::Migration { .. }));
        assert_eq!(tables(&c), vec!["_orkestr_migrations", "a"]);
        assert_eq!(Migrator::applied(&c).unwrap().len(), 1);
    }

    #[test]
    fn validation_rejects_duplicates_and_irreversible() {
        let mut c = Connection::open_in_memory().unwrap();
        assert!(Migrator::new(vec![A, A]).apply(&mut c).is_err());
        let irreversible = Migration { down: "  ", ..B };
        assert!(Migrator::new(vec![irreversible]).apply(&mut c).is_err());
    }
}
