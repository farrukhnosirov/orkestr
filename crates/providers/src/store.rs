//! SQLite persistence of provider configs (non-secret), settings documents
//! (Manager config, routing) and the model catalog cache.

use orkestr_domain::{Model, Provider, ProviderId, SecretRefId, Timestamp};
use orkestr_storage::rusqlite::{self, params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::catalog::{kind_slug, parse_kind};

/// Migrations owned by this crate (append-only).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "providers_0001_init",
    owner: "providers",
    up: "CREATE TABLE providers (
            id TEXT PRIMARY KEY NOT NULL,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            base_url TEXT,
            auth_method TEXT NOT NULL,
            secret_ref_id TEXT,
            is_local INTEGER NOT NULL DEFAULT 0,
            default_model TEXT,
            settings_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX providers_name_unique ON providers (name COLLATE NOCASE);
        CREATE TABLE provider_settings (
            key TEXT PRIMARY KEY NOT NULL,
            value_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE provider_model_cache (
            provider_id TEXT PRIMARY KEY NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
            models_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL
        );",
    down: "DROP TABLE IF EXISTS provider_model_cache;
           DROP TABLE IF EXISTS provider_settings;
           DROP INDEX IF EXISTS providers_name_unique;
           DROP TABLE IF EXISTS providers;",
}];

/// Settings key: AI Manager config.
pub const KEY_MANAGER: &str = "manager";
/// Settings key: routing config.
pub const KEY_ROUTING: &str = "routing";
/// Settings key: first-start bootstrap done.
pub const KEY_BOOTSTRAPPED: &str = "bootstrapped";

fn enum_to_str<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn corrupt(m: impl std::fmt::Display) -> StorageError {
    StorageError::Corrupt(m.to_string())
}

fn row_to_provider(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Provider, StorageError>> {
    let id: String = r.get(0)?;
    let kind: String = r.get(1)?;
    let auth: String = r.get(5)?;
    let secret: Option<String> = r.get(6)?;
    let settings: String = r.get(9)?;
    let created: String = r.get(10)?;
    let updated: String = r.get(11)?;
    let name: String = r.get(2)?;
    let enabled: bool = r.get(3)?;
    let base_url: Option<String> = r.get(4)?;
    let is_local: bool = r.get(7)?;
    let default_model: Option<String> = r.get(8)?;
    Ok((|| {
        Ok(Provider {
            id: ProviderId::parse(&id).map_err(corrupt)?,
            kind: parse_kind(&kind).ok_or_else(|| corrupt(format!("unknown provider kind {kind}")))?,
            name,
            enabled,
            base_url,
            auth_method: serde_json::from_value(serde_json::Value::String(auth)).map_err(corrupt)?,
            credential_ref_id: None,
            secret_ref_id: secret.as_deref().map(SecretRefId::parse).transpose().map_err(corrupt)?,
            is_local,
            default_model,
            settings: serde_json::from_str(&settings)?,
            created_at: created.parse::<Timestamp>().map_err(corrupt)?,
            updated_at: updated.parse::<Timestamp>().map_err(corrupt)?,
        })
    })())
}

const COLUMNS: &str =
    "id, kind, name, enabled, base_url, auth_method, secret_ref_id, is_local, default_model, settings_json, created_at, updated_at";

fn map_unique(e: StorageError, name: &str) -> StorageError {
    if e.is_unique_violation() {
        StorageError::Conflict(format!("a provider named \"{name}\" already exists"))
    } else {
        e
    }
}

/// Provider repository.
#[derive(Debug, Clone)]
pub struct ProviderStore {
    db: Db,
}

impl ProviderStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// All providers ordered by creation.
    pub async fn list(&self) -> StorageResult<Vec<Provider>> {
        self.db
            .call(|c| {
                let mut stmt = c.prepare(&format!("SELECT {COLUMNS} FROM providers ORDER BY created_at, id"))?;
                let rows = stmt.query_map([], row_to_provider)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r??);
                }
                Ok(out)
            })
            .await
    }

    /// One provider.
    pub async fn get(&self, id: ProviderId) -> StorageResult<Option<Provider>> {
        self.db
            .call(move |c| {
                c.query_row(
                    &format!("SELECT {COLUMNS} FROM providers WHERE id = ?1"),
                    [id.to_string()],
                    row_to_provider,
                )
                .optional()?
                .transpose()
            })
            .await
    }

    /// Inserts a provider (name conflicts → `Conflict`).
    pub async fn insert(&self, p: &Provider) -> StorageResult<()> {
        let p = p.clone();
        self.db
            .call(move |c| {
                c.execute(
                    &format!(
                        "INSERT INTO providers ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                    ),
                    params![
                        p.id.to_string(),
                        kind_slug(p.kind),
                        p.name,
                        p.enabled,
                        p.base_url,
                        enum_to_str(&p.auth_method),
                        p.secret_ref_id.map(|s| s.to_string()),
                        p.is_local,
                        p.default_model,
                        p.settings.to_string(),
                        p.created_at.to_rfc3339(),
                        p.updated_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| map_unique(e.into(), &p.name))?;
                Ok(())
            })
            .await
    }

    /// Updates a provider.
    pub async fn update(&self, p: &Provider) -> StorageResult<()> {
        let p = p.clone();
        self.db
            .call(move |c| {
                let n = c
                    .execute(
                        "UPDATE providers SET name = ?2, enabled = ?3, base_url = ?4, auth_method = ?5, secret_ref_id = ?6,
                         is_local = ?7, default_model = ?8, settings_json = ?9, updated_at = ?10 WHERE id = ?1",
                        params![
                            p.id.to_string(),
                            p.name,
                            p.enabled,
                            p.base_url,
                            enum_to_str(&p.auth_method),
                            p.secret_ref_id.map(|s| s.to_string()),
                            p.is_local,
                            p.default_model,
                            p.settings.to_string(),
                            p.updated_at.to_rfc3339(),
                        ],
                    )
                    .map_err(|e| map_unique(e.into(), &p.name))?;
                if n == 0 {
                    return Err(StorageError::NotFound(format!("provider {}", p.id)));
                }
                Ok(())
            })
            .await
    }

    /// Deletes a provider (and its model cache). Returns whether it existed.
    pub async fn delete(&self, id: ProviderId) -> StorageResult<bool> {
        self.db
            .call(move |c| {
                c.execute("DELETE FROM provider_model_cache WHERE provider_id = ?1", [id.to_string()])?;
                Ok(c.execute("DELETE FROM providers WHERE id = ?1", [id.to_string()])? > 0)
            })
            .await
    }

    /// Reads a settings document.
    pub async fn setting<T: DeserializeOwned + Send + 'static>(&self, key: &'static str) -> StorageResult<Option<T>> {
        self.db
            .call(move |c| {
                let raw: Option<String> = c
                    .query_row("SELECT value_json FROM provider_settings WHERE key = ?1", [key], |r| r.get(0))
                    .optional()?;
                raw.map(|s| serde_json::from_str(&s).map_err(StorageError::from)).transpose()
            })
            .await
    }

    /// Writes a settings document.
    pub async fn set_setting<T: Serialize>(&self, key: &'static str, value: &T) -> StorageResult<()> {
        let json = serde_json::to_string(value)?;
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO provider_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                    params![key, json, now],
                )?;
                Ok(())
            })
            .await
    }

    /// Cached models of a provider.
    pub async fn cached_models(&self, id: ProviderId) -> StorageResult<Option<(Vec<Model>, Timestamp)>> {
        self.db
            .call(move |c| {
                let row: Option<(String, String)> = c
                    .query_row(
                        "SELECT models_json, fetched_at FROM provider_model_cache WHERE provider_id = ?1",
                        [id.to_string()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                row.map(|(m, t)| Ok((serde_json::from_str(&m)?, t.parse::<Timestamp>().map_err(corrupt)?))).transpose()
            })
            .await
    }

    /// Stores the model cache of a provider.
    pub async fn cache_models(&self, id: ProviderId, models: &[Model], at: Timestamp) -> StorageResult<()> {
        let json = serde_json::to_string(models)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO provider_model_cache (provider_id, models_json, fetched_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(provider_id) DO UPDATE SET models_json = excluded.models_json, fetched_at = excluded.fetched_at",
                    params![id.to_string(), json, at.to_rfc3339()],
                )?;
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{AuthMethod, ManagerConfig, ProviderKind};

    async fn store() -> ProviderStore {
        let db = Db::open_in_memory().unwrap();
        db.migrate(MIGRATIONS.to_vec()).await.unwrap();
        ProviderStore::new(db)
    }

    fn provider(name: &str) -> Provider {
        let now = chrono::Utc::now();
        Provider {
            id: ProviderId::new(),
            kind: ProviderKind::OpenAiCompatible,
            name: name.into(),
            enabled: true,
            base_url: Some("http://localhost:1234/v1".into()),
            auth_method: AuthMethod::CustomHeaders,
            credential_ref_id: None,
            secret_ref_id: Some(SecretRefId::new()),
            is_local: true,
            default_model: Some("m".into()),
            settings: serde_json::json!({"headers": {"X-Team": "a"}}),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn crud_roundtrip_and_name_conflict() {
        let s = store().await;
        let mut p = provider("LM Studio");
        s.insert(&p).await.unwrap();
        let got = s.get(p.id).await.unwrap().unwrap();
        assert_eq!(got.kind, ProviderKind::OpenAiCompatible);
        assert_eq!(got.secret_ref_id, p.secret_ref_id);
        assert_eq!(got.settings["headers"]["X-Team"], "a");
        assert!(matches!(s.insert(&provider("lm studio")).await, Err(StorageError::Conflict(_))));
        p.enabled = false;
        p.secret_ref_id = None;
        s.update(&p).await.unwrap();
        let got = s.get(p.id).await.unwrap().unwrap();
        assert!(!got.enabled);
        assert!(got.secret_ref_id.is_none());
        assert_eq!(s.list().await.unwrap().len(), 1);
        s.cache_models(p.id, &[], chrono::Utc::now()).await.unwrap();
        assert!(s.cached_models(p.id).await.unwrap().is_some());
        assert!(s.delete(p.id).await.unwrap());
        assert!(!s.delete(p.id).await.unwrap());
        assert!(s.cached_models(p.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn settings_documents() {
        let s = store().await;
        assert!(s.setting::<ManagerConfig>(KEY_MANAGER).await.unwrap().is_none());
        let cfg = ManagerConfig { max_parallel_agents: 7, ..ManagerConfig::default() };
        s.set_setting(KEY_MANAGER, &cfg).await.unwrap();
        s.set_setting(KEY_MANAGER, &cfg).await.unwrap();
        assert_eq!(s.setting::<ManagerConfig>(KEY_MANAGER).await.unwrap(), Some(cfg));
    }
}
