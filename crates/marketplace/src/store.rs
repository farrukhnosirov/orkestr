//! Persistence. Tables are prefixed `marketplace_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).

use orkestr_protocol::methods::marketplace::{InstalledPackage, PublisherKey, RegistrySource};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::MarketplaceError;

/// Migrations owned by this crate (owner `marketplace`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "marketplace_0001_init",
    owner: "marketplace",
    up: "CREATE TABLE marketplace_sources (
            id TEXT PRIMARY KEY,
            position INTEGER NOT NULL,
            doc TEXT NOT NULL,
            pinned_key TEXT,
            index_json BLOB,
            index_sig BLOB
         );
         CREATE TABLE marketplace_keys (key_id TEXT PRIMARY KEY, doc TEXT NOT NULL);
         CREATE TABLE marketplace_pins (package_id TEXT PRIMARY KEY, key_id TEXT NOT NULL);
         CREATE TABLE marketplace_installed (package_id TEXT PRIMARY KEY, doc TEXT NOT NULL);
         CREATE TABLE marketplace_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    down: "DROP TABLE marketplace_meta;
           DROP TABLE marketplace_installed;
           DROP TABLE marketplace_pins;
           DROP TABLE marketplace_keys;
           DROP TABLE marketplace_sources;",
}];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

type R<T> = Result<T, MarketplaceError>;

fn to_json<T: Serialize>(v: &T) -> R<String> {
    serde_json::to_string(v).map_err(|e| MarketplaceError::Internal(e.to_string()))
}

fn from_json<T: DeserializeOwned>(s: &str) -> Result<T, StorageError> {
    serde_json::from_str(s).map_err(|e| StorageError::Corrupt(e.to_string()))
}

/// A stored source with its cached (verified) index.
#[derive(Debug, Clone)]
pub struct SourceRow {
    /// Public view.
    pub source: RegistrySource,
    /// Registry key pinned on first refresh.
    pub pinned_key: Option<String>,
    /// Last verified `index.json`.
    pub index_json: Option<Vec<u8>>,
    /// Its `index.sig`.
    pub index_sig: Option<Vec<u8>>,
}

/// Typed access to the marketplace tables.
#[derive(Debug, Clone)]
pub struct Store {
    db: Db,
}

impl Store {
    /// Wraps the database (migrations must be applied).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// All sources in order.
    pub async fn sources(&self) -> R<Vec<SourceRow>> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare(
                    "SELECT doc, pinned_key, index_json, index_sig FROM marketplace_sources ORDER BY position, id",
                )?;
                let rows = st.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<Vec<u8>>>(2)?,
                        r.get::<_, Option<Vec<u8>>>(3)?,
                    ))
                })?;
                let mut out = Vec::new();
                for row in rows {
                    let (doc, pinned_key, index_json, index_sig) = row?;
                    out.push(SourceRow { source: from_json(&doc)?, pinned_key, index_json, index_sig });
                }
                Ok(out)
            })
            .await?)
    }

    /// Inserts or replaces a source row (keeps its position).
    pub async fn put_source(&self, row: SourceRow) -> R<()> {
        let doc = to_json(&row.source)?;
        Ok(self
            .db
            .call(move |c| {
                let pos: i64 = c
                    .query_row("SELECT position FROM marketplace_sources WHERE id = ?1", [&row.source.id], |r| r.get(0))
                    .optional()?
                    .map_or_else(
                        || {
                            c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM marketplace_sources", [], |r| {
                                r.get(0)
                            })
                        },
                        Ok,
                    )?;
                c.execute(
                    "INSERT OR REPLACE INTO marketplace_sources (id, position, doc, pinned_key, index_json, index_sig)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![row.source.id, pos, doc, row.pinned_key, row.index_json, row.index_sig],
                )?;
                Ok(())
            })
            .await?)
    }

    /// Deletes a source.
    pub async fn delete_source(&self, id: &str) -> R<bool> {
        let id = id.to_owned();
        Ok(self.db.call(move |c| Ok(c.execute("DELETE FROM marketplace_sources WHERE id = ?1", [id])? > 0)).await?)
    }

    /// All keys.
    pub async fn keys(&self) -> R<Vec<PublisherKey>> {
        self.docs("SELECT doc FROM marketplace_keys ORDER BY key_id").await
    }

    /// One key.
    pub async fn key(&self, key_id: &str) -> R<Option<PublisherKey>> {
        let id = key_id.to_owned();
        Ok(self
            .db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM marketplace_keys WHERE key_id = ?1", [id], |r| r.get(0)).optional()?;
                doc.map(|d| from_json(&d)).transpose()
            })
            .await?)
    }

    /// Inserts or replaces a key.
    pub async fn put_key(&self, key: &PublisherKey) -> R<()> {
        let doc = to_json(key)?;
        let id = key.key_id.clone();
        Ok(self
            .db
            .call(move |c| {
                c.execute("INSERT OR REPLACE INTO marketplace_keys (key_id, doc) VALUES (?1, ?2)", params![id, doc])?;
                Ok(())
            })
            .await?)
    }

    /// Key pinned for a package id.
    pub async fn pin(&self, package_id: &str) -> R<Option<String>> {
        self.scalar("SELECT key_id FROM marketplace_pins WHERE package_id = ?1", package_id).await
    }

    /// Pins a key for a package id.
    pub async fn set_pin(&self, package_id: &str, key_id: &str) -> R<()> {
        let (p, k) = (package_id.to_owned(), key_id.to_owned());
        Ok(self
            .db
            .call(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO marketplace_pins (package_id, key_id) VALUES (?1, ?2)",
                    params![p, k],
                )?;
                Ok(())
            })
            .await?)
    }

    /// Installed packages.
    pub async fn installed(&self) -> R<Vec<InstalledPackage>> {
        self.docs("SELECT doc FROM marketplace_installed ORDER BY package_id").await
    }

    /// Records an installed package.
    pub async fn put_installed(&self, p: &InstalledPackage) -> R<()> {
        let doc = to_json(p)?;
        let id = p.id.clone();
        Ok(self
            .db
            .call(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO marketplace_installed (package_id, doc) VALUES (?1, ?2)",
                    params![id, doc],
                )?;
                Ok(())
            })
            .await?)
    }

    /// Forgets an installed package.
    pub async fn delete_installed(&self, id: &str) -> R<bool> {
        let id = id.to_owned();
        Ok(self
            .db
            .call(move |c| Ok(c.execute("DELETE FROM marketplace_installed WHERE package_id = ?1", [id])? > 0))
            .await?)
    }

    /// Meta value.
    pub async fn meta(&self, key: &str) -> R<Option<String>> {
        self.scalar("SELECT value FROM marketplace_meta WHERE key = ?1", key).await
    }

    /// Sets a meta value.
    pub async fn set_meta(&self, key: &str, value: &str) -> R<()> {
        let (k, v) = (key.to_owned(), value.to_owned());
        Ok(self
            .db
            .call(move |c| {
                c.execute("INSERT OR REPLACE INTO marketplace_meta (key, value) VALUES (?1, ?2)", params![k, v])?;
                Ok(())
            })
            .await?)
    }

    async fn scalar(&self, sql: &'static str, arg: &str) -> R<Option<String>> {
        let arg = arg.to_owned();
        Ok(self.db.call(move |c| Ok(c.query_row(sql, [arg], |r| r.get(0)).optional()?)).await?)
    }

    async fn docs<T: DeserializeOwned + Send + 'static>(&self, sql: &'static str) -> R<Vec<T>> {
        Ok(self
            .db
            .call(move |c| {
                let mut st = c.prepare(sql)?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(from_json(&row?)?);
                }
                Ok(out)
            })
            .await?)
    }
}
