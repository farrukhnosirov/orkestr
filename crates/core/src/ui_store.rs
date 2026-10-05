//! UI config + theme library store (`ui.*`, §29, ADR-0015; Phase 3).
//!
//! Tables (owned by the core):
//! * `core_ui_config(scope, config, version, updated_at)` — `scope` is
//!   `global` or `device:<id>`; `version` increments on every write.
//! * `core_ui_themes(id, theme, updated_at)` — user themes (built-ins are
//!   compiled in, see `orkestr_protocol::ui_theme::builtin_themes`).
//!
//! Stored JSON is re-validated on read: a row that no longer parses or
//! validates (hand-edited DB, downgraded build) is ignored instead of breaking
//! startup — the UI then falls back to the defaults (safe mode by data).

use orkestr_protocol::methods::ui::ThemeEntry;
use orkestr_protocol::ui_config::{ThemeConfig, UiConfig};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Migrations owned by the UI store (appended after every other list).
pub const UI_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_0003_ui_config",
    owner: "core",
    up: "CREATE TABLE core_ui_config (
            scope      TEXT PRIMARY KEY NOT NULL,
            config     TEXT NOT NULL,
            version    INTEGER NOT NULL,
            updated_at TEXT NOT NULL
         );
         CREATE TABLE core_ui_themes (
            id         TEXT PRIMARY KEY NOT NULL,
            theme      TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS core_ui_themes; DROP TABLE IF EXISTS core_ui_config;",
}];

/// Scope key of the global config.
pub const GLOBAL_SCOPE: &str = "global";

/// Scope key of a device override.
#[must_use]
pub fn device_scope(device_id: &str) -> String {
    format!("device:{device_id}")
}

/// A stored config row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredConfig {
    /// The document (validated).
    pub config: UiConfig,
    /// Version.
    pub version: u64,
}

fn parse_config(raw: &str) -> Option<UiConfig> {
    let mut cfg: UiConfig = serde_json::from_str(raw).ok()?;
    cfg.validate().ok()?;
    cfg.upgrade();
    Some(cfg)
}

fn parse_theme(raw: &str) -> Option<ThemeConfig> {
    let t: ThemeConfig = serde_json::from_str(raw).ok()?;
    t.validate().ok().map(|()| t)
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Reads one config row (`None` if absent or invalid).
pub async fn get_config(db: &Db, scope: &str) -> StorageResult<Option<StoredConfig>> {
    let scope = scope.to_owned();
    let row: Option<(String, i64)> = db
        .call(move |c| {
            Ok(c.query_row("SELECT config, version FROM core_ui_config WHERE scope = ?1", [scope], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()?)
        })
        .await?;
    Ok(row.and_then(|(raw, version)| {
        let parsed = parse_config(&raw);
        if parsed.is_none() {
            tracing::warn!("ignoring an invalid stored UI config");
        }
        parsed.map(|config| StoredConfig { config, version: to_u64(version) })
    }))
}

/// Current version of a scope (0 if never stored).
pub async fn version_of(db: &Db, scope: &str) -> StorageResult<u64> {
    let scope = scope.to_owned();
    let v: Option<i64> = db
        .call(move |c| {
            Ok(c.query_row("SELECT version FROM core_ui_config WHERE scope = ?1", [scope], |r| r.get(0)).optional()?)
        })
        .await?;
    Ok(v.map_or(0, to_u64))
}

/// Writes a config row and returns the new version.
pub async fn put_config(db: &Db, scope: &str, config: &UiConfig) -> StorageResult<u64> {
    let scope = scope.to_owned();
    let raw = serde_json::to_string(config).map_err(StorageError::from)?;
    let now = chrono::Utc::now().to_rfc3339();
    let v: i64 = db
        .call(move |c| {
            Ok(c.query_row(
                "INSERT INTO core_ui_config (scope, config, version, updated_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(scope) DO UPDATE SET config = excluded.config,
                     version = core_ui_config.version + 1, updated_at = excluded.updated_at
                 RETURNING version",
                params![scope, raw, now],
                |r| r.get(0),
            )?)
        })
        .await?;
    Ok(to_u64(v))
}

/// Removes a config row; returns whether one existed.
pub async fn delete_config(db: &Db, scope: &str) -> StorageResult<bool> {
    let scope = scope.to_owned();
    db.call(move |c| Ok(c.execute("DELETE FROM core_ui_config WHERE scope = ?1", [scope])? > 0)).await
}

/// Every stored config row (scope, document), invalid rows skipped.
pub async fn all_configs(db: &Db) -> StorageResult<Vec<(String, StoredConfig)>> {
    let rows: Vec<(String, String, i64)> = db
        .call(|c| {
            let mut stmt = c.prepare("SELECT scope, config, version FROM core_ui_config ORDER BY scope")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(scope, raw, v)| {
            parse_config(&raw).map(|config| (scope, StoredConfig { config, version: to_u64(v) }))
        })
        .collect())
}

/// User themes, sorted by name (invalid rows skipped).
pub async fn user_themes(db: &Db) -> StorageResult<Vec<ThemeEntry>> {
    let rows: Vec<(String, String)> = db
        .call(|c| {
            let mut stmt = c.prepare("SELECT theme, updated_at FROM core_ui_themes")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    let mut out: Vec<ThemeEntry> = rows
        .into_iter()
        .filter_map(|(raw, at)| {
            let theme = parse_theme(&raw)?;
            let updated_at = chrono::DateTime::parse_from_rfc3339(&at).ok().map(|d| d.with_timezone(&chrono::Utc));
            Some(ThemeEntry { theme, builtin: false, updated_at })
        })
        .collect();
    out.sort_by(|a, b| a.theme.name.to_lowercase().cmp(&b.theme.name.to_lowercase()).then(a.theme.id.cmp(&b.theme.id)));
    Ok(out)
}

/// Whether a user theme with this id exists.
pub async fn theme_exists(db: &Db, id: &str) -> StorageResult<bool> {
    let id = id.to_owned();
    db.call(move |c| {
        Ok(c.query_row("SELECT 1 FROM core_ui_themes WHERE id = ?1", [id], |_| Ok(())).optional()?.is_some())
    })
    .await
}

/// Number of stored user themes.
pub async fn theme_count(db: &Db) -> StorageResult<usize> {
    let n: i64 = db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM core_ui_themes", [], |r| r.get(0))?)).await?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Inserts or replaces a user theme; returns the stored entry.
pub async fn put_theme(db: &Db, theme: &ThemeConfig) -> StorageResult<ThemeEntry> {
    let raw = serde_json::to_string(theme).map_err(StorageError::from)?;
    let now = chrono::Utc::now();
    let id = theme.id.clone();
    let at = now.to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO core_ui_themes (id, theme, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET theme = excluded.theme, updated_at = excluded.updated_at",
            params![id, raw, at],
        )?;
        Ok(())
    })
    .await?;
    Ok(ThemeEntry { theme: theme.clone(), builtin: false, updated_at: Some(now) })
}

/// Deletes a user theme; returns whether it existed.
pub async fn delete_theme(db: &Db, id: &str) -> StorageResult<bool> {
    let id = id.to_owned();
    db.call(move |c| Ok(c.execute("DELETE FROM core_ui_themes WHERE id = ?1", [id])? > 0)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ui.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn config_versions_and_device_overrides() {
        let (_dir, db) = db().await;
        assert!(get_config(&db, GLOBAL_SCOPE).await.unwrap().is_none());
        let cfg = UiConfig::default();
        assert_eq!(put_config(&db, GLOBAL_SCOPE, &cfg).await.unwrap(), 1);
        assert_eq!(put_config(&db, GLOBAL_SCOPE, &cfg).await.unwrap(), 2);
        let dev = device_scope("d1");
        assert_eq!(put_config(&db, &dev, &cfg).await.unwrap(), 1);
        assert_eq!(all_configs(&db).await.unwrap().len(), 2);
        assert_eq!(get_config(&db, GLOBAL_SCOPE).await.unwrap().unwrap().version, 2);
        assert!(delete_config(&db, &dev).await.unwrap());
        assert!(!delete_config(&db, &dev).await.unwrap());
        assert_eq!(version_of(&db, &dev).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn invalid_rows_are_ignored() {
        let (_dir, db) = db().await;
        db.call(|c| {
            c.execute(
                "INSERT INTO core_ui_config (scope, config, version, updated_at) VALUES ('global', '{\"bad\":1}', 3, 'x')",
                [],
            )?;
            c.execute("INSERT INTO core_ui_themes (id, theme, updated_at) VALUES ('x', 'nope', 'x')", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(get_config(&db, GLOBAL_SCOPE).await.unwrap().is_none());
        assert!(user_themes(&db).await.unwrap().is_empty());
        assert_eq!(theme_count(&db).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn themes_roundtrip() {
        let (_dir, db) = db().await;
        let mut t = UiConfig::default().theme;
        t.id = "mine".into();
        t.name = "Mine".into();
        put_theme(&db, &t).await.unwrap();
        assert!(theme_exists(&db, "mine").await.unwrap());
        t.name = "Mine 2".into();
        put_theme(&db, &t).await.unwrap();
        let all = user_themes(&db).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].theme.name, "Mine 2");
        assert!(delete_theme(&db, "mine").await.unwrap());
        assert!(user_themes(&db).await.unwrap().is_empty());
    }
}
