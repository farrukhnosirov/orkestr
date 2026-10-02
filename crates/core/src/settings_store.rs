//! Application settings store (`settings.*`, §29/§31/§55).
//!
//! Key/value JSON in table `core_settings` (one row per top-level
//! [`AppSettings`] field, owned by the core). Loading starts from
//! `AppSettings::default()` and overlays each stored row that still
//! deserializes, so unknown keys (from newer builds) and invalid values
//! (hand-edited DBs) are ignored instead of breaking startup.
//!
//! Updates are serialized; each update writes only the changed keys in one
//! SQLite transaction and then swaps the in-memory copy.

use orkestr_protocol::methods::settings::AppSettings;
use orkestr_storage::rusqlite::params;
use orkestr_storage::{Db, Migration, StorageError, StorageResult};
use parking_lot::RwLock;
use serde_json::Value;

/// Migrations owned by the settings store (appended after every other list).
pub const SETTINGS_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_0002_settings",
    owner: "core",
    up: "CREATE TABLE core_settings (
            key        TEXT PRIMARY KEY NOT NULL,
            value      TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS core_settings;",
}];

/// Settings store with an in-memory cache.
pub struct SettingsStore {
    db: Db,
    current: RwLock<AppSettings>,
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsStore").finish_non_exhaustive()
    }
}

/// Overlays stored rows onto the defaults, skipping rows that do not fit.
#[must_use]
pub fn merge_rows(rows: &[(String, String)]) -> AppSettings {
    let defaults = AppSettings::default();
    let Ok(Value::Object(mut merged)) = serde_json::to_value(&defaults) else { return defaults };
    for (key, raw) in rows {
        if !merged.contains_key(key) {
            tracing::debug!(%key, "ignoring unknown settings key");
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            tracing::warn!(%key, "ignoring unparsable settings value");
            continue;
        };
        let mut candidate = merged.clone();
        candidate.insert(key.clone(), value);
        if serde_json::from_value::<AppSettings>(Value::Object(candidate.clone())).is_ok() {
            merged = candidate;
        } else {
            tracing::warn!(%key, "ignoring settings value of the wrong type");
        }
    }
    serde_json::from_value(Value::Object(merged)).unwrap_or(defaults)
}

impl SettingsStore {
    /// Loads settings from a migrated database.
    pub async fn load(db: Db) -> StorageResult<Self> {
        let rows: Vec<(String, String)> = db
            .call(|c| {
                let mut stmt = c.prepare("SELECT key, value FROM core_settings")?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        Ok(Self { db, current: RwLock::new(merge_rows(&rows)), write_lock: tokio::sync::Mutex::new(()) })
    }

    /// Current settings.
    #[must_use]
    pub fn get(&self) -> AppSettings {
        self.current.read().clone()
    }

    /// Serializes read-modify-write cycles (hold the guard across
    /// [`Self::get`] → side effects → [`Self::save`]).
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    /// Persists `next`, writing only `changed` keys (camelCase), and makes it current.
    pub async fn save(&self, next: AppSettings, changed: &[&str]) -> StorageResult<()> {
        if changed.is_empty() {
            *self.current.write() = next;
            return Ok(());
        }
        let Value::Object(map) = serde_json::to_value(&next).map_err(StorageError::from)? else {
            return Err(StorageError::Invalid("settings must serialize to an object".into()));
        };
        let mut rows = Vec::with_capacity(changed.len());
        for key in changed {
            let value = map.get(*key).ok_or_else(|| StorageError::Invalid(format!("unknown settings key `{key}`")))?;
            rows.push(((*key).to_owned(), value.to_string()));
        }
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO core_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    )?;
                    for (k, v) in &rows {
                        stmt.execute(params![k, v, now])?;
                    }
                }
                tx.commit()?;
                Ok::<_, StorageError>(())
            })
            .await?;
        *self.current.write() = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("s.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn roundtrip_and_partial_writes() {
        let (_dir, db) = db().await;
        let store = SettingsStore::load(db.clone()).await.unwrap();
        assert_eq!(store.get(), AppSettings::default());
        let mut next = store.get();
        next.close_to_tray = false;
        next.language = "ru".into();
        store.save(next.clone(), &["closeToTray", "language"]).await.unwrap();
        assert_eq!(store.get(), next);
        let reloaded = SettingsStore::load(db.clone()).await.unwrap();
        assert_eq!(reloaded.get(), next);
        let n: i64 =
            db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM core_settings", [], |r| r.get(0))?)).await.unwrap();
        assert_eq!(n, 2, "only changed keys are stored");
        assert!(store.save(next, &["noSuchKey"]).await.is_err());
    }

    #[test]
    fn merge_skips_bad_rows() {
        let rows = vec![
            ("closeToTray".to_owned(), "false".to_owned()),
            ("eventRetentionDays".to_owned(), "\"many\"".to_owned()),
            ("futureKey".to_owned(), "1".to_owned()),
            ("language".to_owned(), "not json".to_owned()),
        ];
        let s = merge_rows(&rows);
        assert!(!s.close_to_tray);
        assert_eq!(s.event_retention_days, AppSettings::default().event_retention_days);
        assert_eq!(s.language, "en");
    }
}
