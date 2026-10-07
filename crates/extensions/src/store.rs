//! Persisted registry: installed extensions, editor preferences and
//! remembered language-server approvals (SQLite, owner `extensions`).

use orkestr_protocol::methods::extensions::{EditorPrefs, InstalledExtension};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration};

use crate::error::ExtResult;

/// Migrations of this crate.
pub const EXTENSIONS_MIGRATIONS: &[Migration] = &[Migration {
    id: "extensions_0001_registry",
    owner: "extensions",
    up: "
CREATE TABLE ext_installed (
    id TEXT PRIMARY KEY NOT NULL,
    version TEXT NOT NULL,
    dir TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    installed_at TEXT NOT NULL,
    doc TEXT NOT NULL
);
CREATE TABLE ext_prefs (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
CREATE TABLE lsp_trust (
    fingerprint TEXT PRIMARY KEY NOT NULL,
    server_id TEXT NOT NULL,
    command TEXT NOT NULL,
    approval_id TEXT,
    trusted_at TEXT NOT NULL
);
",
    down: "DROP TABLE IF EXISTS lsp_trust; DROP TABLE IF EXISTS ext_prefs; DROP TABLE IF EXISTS ext_installed;",
}];

/// Migrations of this crate (for the core's composition).
#[must_use]
pub fn extensions_migrations() -> Vec<Migration> {
    EXTENSIONS_MIGRATIONS.to_vec()
}

/// A registry row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Extension.
    pub ext: InstalledExtension,
    /// Folder name under the extensions dir.
    pub dir: String,
}

/// Registry store.
#[derive(Debug, Clone)]
pub struct Store {
    db: Db,
}

const PREFS_KEY: &str = "editor";

impl Store {
    /// Store over `db` (migrated).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// All rows.
    pub async fn list(&self) -> ExtResult<Vec<Row>> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT doc, dir, enabled FROM ext_installed ORDER BY id")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    let (doc, dir, enabled) = row?;
                    match serde_json::from_str::<InstalledExtension>(&doc) {
                        Ok(mut ext) => {
                            ext.enabled = enabled != 0;
                            out.push(Row { ext, dir });
                        }
                        Err(e) => tracing::warn!(error = %e, "skipping an unreadable extension registry row"),
                    }
                }
                Ok(out)
            })
            .await?)
    }

    /// One row.
    pub async fn get(&self, id: &str) -> ExtResult<Option<Row>> {
        let id = id.to_ascii_lowercase();
        Ok(self.list().await?.into_iter().find(|r| r.ext.id == id))
    }

    /// Inserts or replaces a row.
    pub async fn upsert(&self, ext: &InstalledExtension, dir: &str) -> ExtResult<()> {
        let doc = serde_json::to_string(ext)?;
        let (id, version, dir, enabled, at) =
            (ext.id.clone(), ext.version.clone(), dir.to_owned(), i64::from(ext.enabled), ext.installed_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO ext_installed (id, version, dir, enabled, installed_at, doc) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET version = ?2, dir = ?3, enabled = ?4, installed_at = ?5, doc = ?6",
                    params![id, version, dir, enabled, at, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Sets the enabled flag; `false` when the row does not exist.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> ExtResult<bool> {
        let id = id.to_ascii_lowercase();
        Ok(self
            .db
            .call(move |c| {
                Ok(c.execute("UPDATE ext_installed SET enabled = ?2 WHERE id = ?1", params![id, i64::from(enabled)])? > 0)
            })
            .await?)
    }

    /// Deletes a row.
    pub async fn delete(&self, id: &str) -> ExtResult<bool> {
        let id = id.to_ascii_lowercase();
        Ok(self.db.call(move |c| Ok(c.execute("DELETE FROM ext_installed WHERE id = ?1", params![id])? > 0)).await?)
    }

    /// Editor preferences.
    pub async fn prefs(&self) -> ExtResult<EditorPrefs> {
        let raw: Option<String> = self
            .db
            .call(|c| {
                Ok(c.query_row("SELECT value FROM ext_prefs WHERE key = ?1", [PREFS_KEY], |r| r.get(0)).optional()?)
            })
            .await?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    /// Stores editor preferences.
    pub async fn set_prefs(&self, prefs: &EditorPrefs) -> ExtResult<()> {
        let v = serde_json::to_string(prefs)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO ext_prefs (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
                    params![PREFS_KEY, v],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Whether a language-server command fingerprint is trusted.
    pub async fn is_trusted(&self, fingerprint: &str) -> ExtResult<bool> {
        let fp = fingerprint.to_owned();
        Ok(self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT 1 FROM lsp_trust WHERE fingerprint = ?1", [fp], |_| Ok(()))
                    .optional()?
                    .is_some())
            })
            .await?)
    }

    /// Remembers an approved language-server command.
    pub async fn trust(&self, fingerprint: &str, server_id: &str, command: &str, approval_id: Option<String>) -> ExtResult<()> {
        let (fp, sid, cmd, at) =
            (fingerprint.to_owned(), server_id.to_owned(), command.to_owned(), chrono::Utc::now().to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO lsp_trust (fingerprint, server_id, command, approval_id, trusted_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![fp, sid, cmd, approval_id, at],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Forgets every approval of a server.
    pub async fn revoke(&self, server_id: &str) -> ExtResult<u32> {
        let sid = server_id.to_owned();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM lsp_trust WHERE server_id = ?1", [sid])?)).await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}
