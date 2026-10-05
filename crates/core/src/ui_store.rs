//! Versioned UI configuration store (ADR-0015, `ui.previewPatch` /
//! `ui.applyPatch`).
//!
//! Every accepted change is a new row in `ui_config_versions` (the table
//! shape from ADR-0015: version, document, author, summary, time); the
//! newest row is the current document. Version 0 means "never stored" (the
//! built-in defaults). A row that no longer deserializes or validates (e.g.
//! hand-edited) is skipped on load, so a bad document never locks the user
//! out of the UI.
//!
//! INTEGRATOR(P3 theme-engine): if the theme engine lands its own
//! `ui.getConfig` / `ui.setConfig` / `ui.history` over the same table, keep
//! one store (this one or theirs) and route both areas through it.

use orkestr_protocol::ui_config::UiConfig;
use orkestr_protocol::ui_patch::UiPatchActor;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};
use parking_lot::RwLock;

/// Migrations owned by the UI store (appended after every other list).
/// `IF NOT EXISTS`: the theme engine may create the same ADR-0015 table.
pub const UI_STORE_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_ui_0001_config_versions",
    owner: "core",
    up: "CREATE TABLE IF NOT EXISTS ui_config_versions (
            version     INTEGER PRIMARY KEY NOT NULL,
            config_json TEXT NOT NULL,
            author      TEXT NOT NULL,
            summary     TEXT NOT NULL,
            created_at  TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS ui_config_versions;",
}];

/// Max rows kept (older versions are pruned on write).
pub const MAX_VERSIONS: i64 = 200;

/// The store.
pub struct UiConfigStore {
    db: Db,
    current: RwLock<(UiConfig, u64)>,
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for UiConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiConfigStore").field("version", &self.current.read().1).finish_non_exhaustive()
    }
}

fn author_str(a: UiPatchActor) -> &'static str {
    match a {
        UiPatchActor::User => "user",
        UiPatchActor::Manager => "manager",
        UiPatchActor::Agent => "agent",
    }
}

impl UiConfigStore {
    /// Loads the newest valid version.
    pub async fn load(db: Db) -> StorageResult<Self> {
        let rows: Vec<(i64, String)> = db
            .call(|c| {
                let mut stmt =
                    c.prepare("SELECT version, config_json FROM ui_config_versions ORDER BY version DESC LIMIT 20")?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        let newest = rows.first().map_or(0, |(v, _)| u64::try_from(*v).unwrap_or(0));
        let current = rows
            .iter()
            .find_map(|(v, json)| {
                let cfg: UiConfig = serde_json::from_str(json).ok()?;
                cfg.validate().ok()?;
                Some((cfg, u64::try_from(*v).unwrap_or(0)))
            })
            .map_or_else(
                || (UiConfig::default(), newest),
                |(cfg, v)| {
                    if v != newest {
                        tracing::warn!(version = v, newest, "newest UI config is invalid; using an older version");
                    }
                    // Keep numbering monotonic even when falling back.
                    (cfg, newest)
                },
            );
        Ok(Self { db, current: RwLock::new(current), write_lock: tokio::sync::Mutex::new(()) })
    }

    /// Current document and version.
    #[must_use]
    pub fn current(&self) -> (UiConfig, u64) {
        self.current.read().clone()
    }

    /// Serializes read-validate-write cycles.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    /// Stores `config` as the next version (call while holding [`Self::lock`]).
    pub async fn commit(&self, config: UiConfig, author: UiPatchActor, summary: &str) -> StorageResult<u64> {
        let json = serde_json::to_string(&config).map_err(StorageError::from)?;
        let version = self.current.read().1 + 1;
        let v = i64::try_from(version).map_err(|_| StorageError::Invalid("version overflow".into()))?;
        let summary: String = summary.chars().take(200).collect();
        let author = author_str(author);
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO ui_config_versions (version, config_json, author, summary, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![v, json, author, summary, now],
                )?;
                tx.execute("DELETE FROM ui_config_versions WHERE version <= ?1", params![v - MAX_VERSIONS])?;
                tx.commit()?;
                Ok::<_, StorageError>(())
            })
            .await?;
        *self.current.write() = (config, version);
        Ok(version)
    }

    /// Summary of a stored version (tests, diagnostics).
    pub async fn summary_of(&self, version: u64) -> StorageResult<Option<(String, String)>> {
        let v = i64::try_from(version).unwrap_or(i64::MAX);
        self.db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT author, summary FROM ui_config_versions WHERE version = ?1",
                    params![v],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::ui_prefs::{Density, UiPrefs};

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("u.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn versions_persist_and_bad_rows_are_skipped() {
        let (_dir, db) = db().await;
        let store = UiConfigStore::load(db.clone()).await.unwrap();
        assert_eq!(store.current(), (UiConfig::default(), 0));
        let cfg = UiConfig { prefs: Some(UiPrefs { density: Density::Compact, ..UiPrefs::default() }), ..UiConfig::default() };
        let _g = store.lock().await;
        assert_eq!(store.commit(cfg.clone(), UiPatchActor::Manager, "compact").await.unwrap(), 1);
        let reloaded = UiConfigStore::load(db.clone()).await.unwrap();
        assert_eq!(reloaded.current(), (cfg.clone(), 1));
        assert_eq!(reloaded.summary_of(1).await.unwrap(), Some(("manager".into(), "compact".into())));
        // A corrupt newer row falls back to the last valid document, numbering stays monotonic.
        db.call(|c| {
            c.execute(
                "INSERT INTO ui_config_versions (version, config_json, author, summary, created_at) VALUES (2, '{\"bad\":1}', 'user', 'x', 'now')",
                [],
            )?;
            Ok::<_, StorageError>(())
        })
        .await
        .unwrap();
        let reloaded = UiConfigStore::load(db).await.unwrap();
        assert_eq!(reloaded.current(), (cfg, 2));
    }
}
