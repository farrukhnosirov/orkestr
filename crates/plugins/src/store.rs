//! Persistence. Tables are prefixed `plugins_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).

use orkestr_domain::{now, Plugin, PluginId, PluginManifest};
use orkestr_protocol::methods::plugins::{PluginGrant, PluginLogLine};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::{Deserialize, Serialize};

use crate::PluginError;

/// Migrations owned by this crate (owner `plugins`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "plugins_0001_init",
    owner: "plugins",
    up: r"
CREATE TABLE plugins_installed (
    id TEXT PRIMARY KEY,
    package_id TEXT NOT NULL UNIQUE,
    doc TEXT NOT NULL,
    installed_at TEXT NOT NULL
);
CREATE TABLE plugins_kv (
    plugin_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (plugin_id, key)
);
CREATE TABLE plugins_logs (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL
);
CREATE INDEX plugins_logs_plugin ON plugins_logs(plugin_id, seq);
CREATE TABLE plugins_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE plugins_trusted_keys (
    key_id TEXT PRIMARY KEY,
    public_key TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    source TEXT NOT NULL,
    added_at TEXT NOT NULL
);
",
    down: r"
DROP TABLE IF EXISTS plugins_trusted_keys;
DROP TABLE IF EXISTS plugins_meta;
DROP INDEX IF EXISTS plugins_logs_plugin;
DROP TABLE IF EXISTS plugins_logs;
DROP TABLE IF EXISTS plugins_kv;
DROP TABLE IF EXISTS plugins_installed;
",
}];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

/// Max log lines kept per plugin.
pub const MAX_LOG_LINES: i64 = 2_000;
/// Max KV keys per plugin.
pub const MAX_KV_KEYS: i64 = 1_000;

/// One installed plugin as persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPlugin {
    /// The plugin.
    pub plugin: Plugin,
    /// Manifest.
    pub manifest: PluginManifest,
    /// Decided grants (mirror of the permission rules of principal `plugin:<id>`).
    pub grants: Vec<PluginGrant>,
    /// Signer fingerprint (`None` = unsigned dev package).
    pub signer_fingerprint: Option<String>,
    /// Signer key id.
    #[serde(default)]
    pub signer_key_id: Option<String>,
    /// Package sha256.
    pub sha256: String,
    /// Last error.
    pub last_error: Option<String>,
    /// Consecutive traps (quarantine at [`crate::service::QUARANTINE_AFTER_TRAPS`]).
    #[serde(default)]
    pub trap_count: u32,
}

/// Typed access to the plugin tables.
#[derive(Clone, Debug)]
pub struct PluginStore {
    db: Db,
}

type R<T> = Result<T, PluginError>;

impl PluginStore {
    /// Store over the migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces a plugin.
    ///
    /// # Errors
    /// Storage.
    pub async fn save(&self, p: &StoredPlugin) -> R<()> {
        let doc = serde_json::to_string(p).map_err(StorageError::Json)?;
        let (id, pkg, at) = (p.plugin.id.to_string(), p.plugin.package_id.clone(), p.plugin.installed_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO plugins_installed (id, package_id, doc, installed_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET package_id = excluded.package_id, doc = excluded.doc",
                    params![id, pkg, doc, at],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Every plugin (install order).
    ///
    /// # Errors
    /// Storage / corrupt rows.
    pub async fn all(&self) -> R<Vec<StoredPlugin>> {
        let docs: Vec<String> = self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT doc FROM plugins_installed ORDER BY installed_at, id")?;
                let rows = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
                Ok(rows)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(|e| PluginError::Storage(StorageError::Json(e)))).collect()
    }

    /// Deletes a plugin with its KV and logs.
    ///
    /// # Errors
    /// Storage.
    pub async fn delete(&self, id: PluginId) -> R<bool> {
        let id = id.to_string();
        let n = self
            .db
            .call(move |c| {
                c.execute("DELETE FROM plugins_kv WHERE plugin_id = ?1", [&id])?;
                c.execute("DELETE FROM plugins_logs WHERE plugin_id = ?1", [&id])?;
                Ok(c.execute("DELETE FROM plugins_installed WHERE id = ?1", [&id])?)
            })
            .await?;
        Ok(n > 0)
    }

    /// Reads a meta flag.
    ///
    /// # Errors
    /// Storage.
    pub async fn meta(&self, key: &'static str) -> R<Option<String>> {
        Ok(self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT value FROM plugins_meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?)
    }

    /// Writes a meta flag.
    ///
    /// # Errors
    /// Storage.
    pub async fn set_meta(&self, key: &'static str, value: String) -> R<()> {
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO plugins_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Appends a log line (keeps the newest [`MAX_LOG_LINES`]).
    ///
    /// # Errors
    /// Storage.
    pub async fn log(&self, id: PluginId, level: &str, message: &str) -> R<()> {
        let (id, level, message) = (id.to_string(), level.to_owned(), message.chars().take(4_000).collect::<String>());
        let ts = now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO plugins_logs (plugin_id, ts, level, message) VALUES (?1, ?2, ?3, ?4)",
                    params![id, ts, level, message],
                )?;
                c.execute(
                    "DELETE FROM plugins_logs WHERE plugin_id = ?1 AND seq <= (SELECT seq FROM plugins_logs WHERE plugin_id = ?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2)",
                    params![id, MAX_LOG_LINES],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Newest `limit` log lines (oldest first).
    ///
    /// # Errors
    /// Storage.
    pub async fn logs(&self, id: PluginId, limit: u32) -> R<Vec<PluginLogLine>> {
        let id = id.to_string();
        let rows: Vec<(String, String, String)> = self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT ts, level, message FROM plugins_logs WHERE plugin_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?;
                let rows = st
                    .query_map(params![id, i64::from(limit)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(rows
            .into_iter()
            .rev()
            .map(|(ts, level, message)| PluginLogLine {
                ts: chrono::DateTime::parse_from_rfc3339(&ts).map_or_else(|_| now(), |d| d.to_utc()),
                level,
                message,
            })
            .collect())
    }

    /// KV read.
    ///
    /// # Errors
    /// Storage.
    pub async fn kv_get(&self, id: PluginId, key: String) -> R<Option<String>> {
        let id = id.to_string();
        Ok(self
            .db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT value FROM plugins_kv WHERE plugin_id = ?1 AND key = ?2",
                    params![id, key],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .await?)
    }

    /// KV write (`None` deletes). Refuses new keys beyond [`MAX_KV_KEYS`].
    ///
    /// # Errors
    /// Storage / quota.
    pub async fn kv_set(&self, id: PluginId, key: String, value: Option<String>) -> R<()> {
        let id = id.to_string();
        let ok = self
            .db
            .call(move |c| {
                match value {
                    None => {
                        c.execute("DELETE FROM plugins_kv WHERE plugin_id = ?1 AND key = ?2", params![id, key])?;
                    }
                    Some(v) => {
                        let exists: Option<i64> = c
                            .query_row(
                                "SELECT 1 FROM plugins_kv WHERE plugin_id = ?1 AND key = ?2",
                                params![id, key],
                                |r| r.get(0),
                            )
                            .optional()?;
                        if exists.is_none() {
                            let n: i64 =
                                c.query_row("SELECT COUNT(*) FROM plugins_kv WHERE plugin_id = ?1", [&id], |r| {
                                    r.get(0)
                                })?;
                            if n >= MAX_KV_KEYS {
                                return Ok(false);
                            }
                        }
                        c.execute(
                            "INSERT INTO plugins_kv (plugin_id, key, value) VALUES (?1, ?2, ?3)
                             ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
                            params![id, key, v],
                        )?;
                    }
                }
                Ok(true)
            })
            .await?;
        if ok {
            Ok(())
        } else {
            Err(PluginError::Denied(format!("KV quota of {MAX_KV_KEYS} keys reached")))
        }
    }
}
