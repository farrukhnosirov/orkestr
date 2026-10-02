//! Secrets (§24, contract §6, ADR-0011).
//!
//! * [`stores`] — value backends: OS keyring, encrypted file vault, memory.
//! * [`SecretService`] — `SecretReference` rows in SQLite (`secret_refs`,
//!   never values), scope checks, lock/unlock, audit of every access and
//!   registration of every stored/resolved value with the redactor.
//!
//! The RPC surface can list, set and delete secrets but never read values;
//! only engines inside the core call [`SecretService::resolve`].

pub mod stores;

use std::sync::Arc;

use orkestr_domain::{
    now, Actor, AuditOutcome, Capability, ProjectId, SecretBackend, SecretRefId, SecretReference, SecretScope, Timestamp,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use stores::{EncryptedFileVault, KeyringStore, MemorySecretStore, SecretStore};

use crate::audit::{AuditLog, NewAudit};
use crate::emergency::EmergencyController;
use crate::error::{SecurityError, SecurityResult};
use crate::redact;

/// Migration creating the reference table.
pub const MIGRATION: Migration = Migration {
    id: "security_0005_secrets",
    owner: "security",
    up: r"
CREATE TABLE secret_refs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    project_id TEXT,
    doc TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(name, scope_key)
);
CREATE INDEX secret_refs_project ON secret_refs(project_id);
",
    down: "DROP TABLE IF EXISTS secret_refs;",
};

/// Max secret value size.
pub const MAX_VALUE_LEN: usize = 64 * 1024;

/// Which value backend new secrets go to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretBackendChoice {
    /// OS keyring when it works (values ≤ 2 KiB), otherwise the vault (key in keyring or key file).
    #[default]
    Auto,
    /// Encrypted vault only (key in keyring if available, else key file).
    Vault,
    /// Encrypted vault with a key file — never touches the OS keyring (headless, tests).
    VaultFileKey,
    /// In-memory (tests / ephemeral cores). Values are lost on restart.
    Memory,
}

impl SecretBackendChoice {
    /// Parses `ORKESTR_SECRET_BACKEND` values.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "keyring" => Some(Self::Auto),
            "vault" => Some(Self::Vault),
            "vault-file" | "vaultfilekey" | "file" => Some(Self::VaultFileKey),
            "memory" => Some(Self::Memory),
            _ => None,
        }
    }
}

/// Vault status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    /// Locked: secret reads/writes are refused.
    pub locked: bool,
    /// Backend preference.
    pub backend: SecretBackendChoice,
    /// Number of stored secret references.
    pub secret_count: u32,
}

/// Access context for `resolve`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccessContext {
    /// Project the caller works in (required for project/environment scoped secrets).
    pub project_id: Option<ProjectId>,
}

/// The secret service.
pub struct SecretService {
    db: Db,
    keyring: Option<Arc<dyn SecretStore>>,
    vault: Arc<EncryptedFileVault>,
    memory: Option<Arc<MemorySecretStore>>,
    choice: SecretBackendChoice,
    audit: Arc<AuditLog>,
    emergency: Arc<EmergencyController>,
}

impl std::fmt::Debug for SecretService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretService").field("choice", &self.choice).finish_non_exhaustive()
    }
}

fn scope_key(scope: &SecretScope) -> (String, Option<ProjectId>) {
    match scope {
        SecretScope::Global => ("global".into(), None),
        SecretScope::Project { project_id } => (format!("project:{project_id}"), Some(*project_id)),
        SecretScope::Environment { project_id, environment_id } => (format!("env:{project_id}:{environment_id}"), Some(*project_id)),
    }
}

/// Validates a secret name (`OPENAI_API_KEY`, `db.url`, `deploy-token`).
pub fn validate_name(name: &str) -> SecurityResult<String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.len() <= 128
        && n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(n.to_owned())
    } else {
        Err(SecurityError::Invalid("secret name must match [A-Za-z_][A-Za-z0-9_.-]{0,127}".into()))
    }
}

impl SecretService {
    /// Creates the service.
    #[must_use]
    pub fn new(
        db: Db,
        keyring: Option<Arc<dyn SecretStore>>,
        vault: Arc<EncryptedFileVault>,
        choice: SecretBackendChoice,
        audit: Arc<AuditLog>,
        emergency: Arc<EmergencyController>,
    ) -> Self {
        let memory = (choice == SecretBackendChoice::Memory).then(|| Arc::new(MemorySecretStore::default()));
        Self { db, keyring, vault, memory, choice, audit, emergency }
    }

    fn locked(&self) -> bool {
        self.emergency.flags().vault_locked
    }

    fn store_for(&self, backend: SecretBackend) -> SecurityResult<Arc<dyn SecretStore>> {
        if let Some(m) = &self.memory {
            return Ok(Arc::clone(m) as Arc<dyn SecretStore>);
        }
        match backend {
            SecretBackend::OsKeyring => {
                self.keyring.clone().ok_or_else(|| SecurityError::Backend("the OS keyring is not configured".into()))
            }
            SecretBackend::EncryptedVault => {
                if !self.vault.is_unlocked() {
                    self.vault.unlock()?;
                }
                Ok(Arc::clone(&self.vault) as Arc<dyn SecretStore>)
            }
            other => Err(SecurityError::Backend(format!("secret backend {other:?} is not available in this build"))),
        }
    }

    async fn load_refs(&self, project: Option<ProjectId>) -> SecurityResult<Vec<SecretReference>> {
        let p = project.map(|p| p.to_string());
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM secret_refs WHERE (?1 IS NULL OR project_id IS NULL OR project_id = ?1) ORDER BY name, scope_key",
                )?;
                let rows = stmt.query_map(params![p], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::with_capacity(docs.len());
        for d in docs {
            out.push(serde_json::from_str(&d)?);
        }
        Ok(out)
    }

    async fn save_ref(&self, r: &SecretReference) -> SecurityResult<()> {
        let (key, project) = scope_key(&r.scope);
        let doc = serde_json::to_string(r)?;
        let (id, name, created) = (r.id.to_string(), r.name.clone(), r.created_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO secret_refs (id, name, scope_key, project_id, doc, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET doc = excluded.doc",
                    params![id, name, key, project.map(|p| p.to_string()), doc, created],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Lists references (Global + the given project's when `project` is set; all otherwise).
    pub async fn list(&self, project: Option<ProjectId>) -> SecurityResult<Vec<SecretReference>> {
        self.load_refs(project).await
    }

    /// One reference.
    pub async fn get_ref(&self, id: SecretRefId) -> SecurityResult<Option<SecretReference>> {
        let ids = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| Ok(c.query_row("SELECT doc FROM secret_refs WHERE id = ?1", [ids], |r| r.get(0)).optional()?))
            .await?;
        Ok(doc.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    async fn find(&self, name: &str, scope: &SecretScope) -> SecurityResult<Option<SecretReference>> {
        let (key, _) = scope_key(scope);
        let name = name.to_owned();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM secret_refs WHERE name = ?1 AND scope_key = ?2", params![name, key], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        Ok(doc.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    /// Creates or rotates a secret (`name` unique per scope).
    pub async fn set(&self, name: &str, scope: SecretScope, value: Zeroizing<String>, by: Actor) -> SecurityResult<SecretReference> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot write secrets directly".into()));
        }
        if self.locked() {
            return Err(SecurityError::VaultLocked);
        }
        let name = validate_name(name)?;
        if value.is_empty() {
            return Err(SecurityError::Invalid("secret value must not be empty".into()));
        }
        if value.len() > MAX_VALUE_LEN {
            return Err(SecurityError::Invalid(format!("secret value too large (max {MAX_VALUE_LEN} bytes)")));
        }
        let existing = self.find(&name, &scope).await?;
        let t = now();
        let mut reference = match existing {
            Some(mut r) => {
                r.rotated_at = Some(t);
                r
            }
            None => {
                let id = SecretRefId::new();
                SecretReference {
                    id,
                    name: name.clone(),
                    scope,
                    backend: SecretBackend::EncryptedVault,
                    locator: id.to_string(),
                    created_at: t,
                    rotated_at: None,
                    last_accessed_at: None,
                }
            }
        };
        // Choose a backend: keyring first in Auto mode, vault otherwise / as fallback.
        let previous_backend = reference.backend;
        let mut stored = false;
        if self.choice == SecretBackendChoice::Auto && value.len() <= stores::MAX_KEYRING_VALUE {
            if let Ok(k) = self.store_for(SecretBackend::OsKeyring) {
                let (k, loc, v) = (k, reference.locator.clone(), value.clone());
                match tokio::task::spawn_blocking(move || k.put(&loc, &v)).await {
                    Ok(Ok(())) => {
                        reference.backend = SecretBackend::OsKeyring;
                        stored = true;
                    }
                    Ok(Err(e)) => tracing::warn!(error = %e, "OS keyring write failed; storing the secret in the encrypted vault"),
                    Err(e) => tracing::warn!(error = %e, "keyring task failed; storing the secret in the encrypted vault"),
                }
            }
        }
        if !stored {
            let v = self.store_for(SecretBackend::EncryptedVault)?;
            let (loc, val) = (reference.locator.clone(), value.clone());
            tokio::task::spawn_blocking(move || v.put(&loc, &val)).await.map_err(|e| SecurityError::Backend(e.to_string()))??;
            reference.backend = SecretBackend::EncryptedVault;
        }
        // Remove a stale copy when the backend changed on rotation.
        if previous_backend != reference.backend && reference.rotated_at.is_some() {
            if let Ok(old) = self.store_for(previous_backend) {
                let loc = reference.locator.clone();
                let _ = tokio::task::spawn_blocking(move || old.delete(&loc)).await;
            }
        }
        redact::register_secret(&value);
        self.save_ref(&reference).await?;
        self.audit
            .record(
                NewAudit::new(by, "secrets.set", reference.name.clone(), AuditOutcome::Executed)
                    .capability(Capability::Secrets)
                    .project(scope_key(&reference.scope).1)
                    .details(serde_json::json!({ "secretId": reference.id, "backend": reference.backend, "rotated": reference.rotated_at.is_some() })),
            )
            .await;
        tracing::info!(secret_id = %reference.id, name = %reference.name, "secret stored");
        Ok(reference)
    }

    /// Deletes a secret (value + reference).
    pub async fn delete(&self, id: SecretRefId, by: Actor) -> SecurityResult<bool> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot delete secrets".into()));
        }
        let Some(r) = self.get_ref(id).await? else { return Ok(false) };
        // Deleting a keyring value works while locked; vault values need the key only to rewrite the file.
        if let Ok(store) = self.store_for(r.backend) {
            let loc = r.locator.clone();
            match tokio::task::spawn_blocking(move || store.delete(&loc)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "could not delete secret value from its backend"),
                Err(e) => tracing::warn!(error = %e, "secret delete task failed"),
            }
        }
        let ids = id.to_string();
        self.db.call(move |c| Ok(c.execute("DELETE FROM secret_refs WHERE id = ?1", [ids])?)).await?;
        self.audit
            .record(
                NewAudit::new(by, "secrets.delete", r.name.clone(), AuditOutcome::Executed)
                    .capability(Capability::Secrets)
                    .project(scope_key(&r.scope).1)
                    .details(serde_json::json!({ "secretId": id })),
            )
            .await;
        Ok(true)
    }

    /// Resolves a secret value for an engine inside the core. Audited; the
    /// value is registered with the redactor. Never exposed over RPC.
    pub async fn resolve(&self, id: SecretRefId, by: Actor, ctx: AccessContext) -> SecurityResult<Zeroizing<String>> {
        let r = self.get_ref(id).await?.ok_or_else(|| SecurityError::SecretNotFound(id.to_string()))?;
        let denied = |reason: &str| SecurityError::Forbidden(reason.to_owned());
        let result: SecurityResult<Zeroizing<String>> = async {
            if self.locked() {
                return Err(SecurityError::VaultLocked);
            }
            if let Some(p) = scope_key(&r.scope).1 {
                if ctx.project_id != Some(p) {
                    return Err(denied("secret belongs to another project (§67)"));
                }
            }
            let store = self.store_for(r.backend)?;
            let loc = r.locator.clone();
            let value = tokio::task::spawn_blocking(move || store.get(&loc))
                .await
                .map_err(|e| SecurityError::Backend(e.to_string()))??
                .ok_or_else(|| SecurityError::SecretNotFound(format!("value of {} is missing from its backend", r.name)))?;
            Ok(value)
        }
        .await;
        let outcome = if result.is_ok() { AuditOutcome::Allowed } else { AuditOutcome::Denied };
        self.audit
            .record(
                NewAudit::new(by, "secrets.read", r.name.clone(), outcome)
                    .capability(Capability::Secrets)
                    .project(ctx.project_id)
                    .details(serde_json::json!({ "secretId": id, "error": result.as_ref().err().map(ToString::to_string) })),
            )
            .await;
        let value = result?;
        redact::register_secret(&value);
        let mut updated = r;
        updated.last_accessed_at = Some(now());
        self.save_ref(&updated).await?;
        Ok(value)
    }

    /// Resolves by name: project scope first, then global.
    pub async fn resolve_by_name(&self, name: &str, by: Actor, ctx: AccessContext) -> SecurityResult<Zeroizing<String>> {
        let mut candidate = None;
        if let Some(p) = ctx.project_id {
            candidate = self.find(name, &SecretScope::Project { project_id: p }).await?;
        }
        if candidate.is_none() {
            candidate = self.find(name, &SecretScope::Global).await?;
        }
        let r = candidate.ok_or_else(|| SecurityError::SecretNotFound(name.to_owned()))?;
        self.resolve(r.id, by, ctx).await
    }

    /// Locks the vault: drops the cached key; every read/write is refused
    /// (`Secrets` capability denied) until [`unlock`](Self::unlock).
    pub async fn lock(&self, by: Actor) -> SecurityResult<VaultStatus> {
        self.vault.lock();
        self.emergency.set_vault_locked(true).await?;
        self.audit.record(NewAudit::new(by, "secrets.lock", "vault", AuditOutcome::Executed)).await;
        tracing::warn!("secret vault locked");
        self.status().await
    }

    /// Unlocks the vault.
    pub async fn unlock(&self, by: Actor) -> SecurityResult<VaultStatus> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot unlock the vault".into()));
        }
        if self.memory.is_none() {
            let v = Arc::clone(&self.vault);
            // Only touch the key when a vault exists or vault mode is forced.
            let needs_key = self.choice != SecretBackendChoice::Auto || self.has_vault_secrets().await?;
            if needs_key {
                tokio::task::spawn_blocking(move || v.unlock()).await.map_err(|e| SecurityError::Backend(e.to_string()))??;
            }
        }
        self.emergency.set_vault_locked(false).await?;
        self.audit.record(NewAudit::new(by, "secrets.unlock", "vault", AuditOutcome::Executed)).await;
        self.status().await
    }

    async fn has_vault_secrets(&self) -> SecurityResult<bool> {
        Ok(self.load_refs(None).await?.iter().any(|r| r.backend == SecretBackend::EncryptedVault))
    }

    /// Status.
    pub async fn status(&self) -> SecurityResult<VaultStatus> {
        let n: i64 = self.db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM secret_refs", [], |r| r.get(0))?)).await?;
        Ok(VaultStatus { locked: self.locked(), backend: self.choice, secret_count: u32::try_from(n).unwrap_or(u32::MAX) })
    }

    /// Registers every stored value with the redactor (startup), so values
    /// are scrubbed from logs/events even before they are first resolved.
    /// Skipped while locked. Returns how many values were registered.
    pub async fn preload_redaction(&self) -> SecurityResult<usize> {
        if self.locked() {
            return Ok(0);
        }
        let refs = self.load_refs(None).await?;
        let mut n = 0;
        for r in &refs {
            let Ok(store) = self.store_for(r.backend) else { continue };
            let loc = r.locator.clone();
            if let Ok(Ok(Some(v))) = tokio::task::spawn_blocking(move || store.get(&loc)).await {
                redact::register_secret(&v);
                n += 1;
            }
        }
        if n > 0 {
            self.audit
                .record(
                    NewAudit::new(Actor::System, "secrets.preloadRedaction", "vault", AuditOutcome::Executed)
                        .details(serde_json::json!({ "count": n })),
                )
                .await;
        }
        Ok(n)
    }

    /// Last access of a reference (for tests/UI).
    pub async fn last_accessed(&self, id: SecretRefId) -> SecurityResult<Option<Timestamp>> {
        Ok(self.get_ref(id).await?.and_then(|r| r.last_accessed_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{AgentId, UserId};
    use orkestr_events::EventBus;
    use orkestr_platform::{CredentialStore, MemoryCredentialStore};

    struct Fx {
        _dir: tempfile::TempDir,
        svc: SecretService,
        audit: Arc<AuditLog>,
    }

    async fn fx(choice: SecretBackendChoice) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::migrations()).await.unwrap();
        let audit = Arc::new(AuditLog::new(db.clone()));
        let emergency = Arc::new(EmergencyController::open(db.clone(), EventBus::in_memory(16)).await.unwrap());
        let creds: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
        let keyring: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(Arc::clone(&creds), "test"));
        let vault = Arc::new(EncryptedFileVault::new(dir.path().join("vault"), Some(creds), "test"));
        let svc = SecretService::new(db, Some(keyring), vault, choice, Arc::clone(&audit), emergency);
        Fx { _dir: dir, svc, audit }
    }

    fn user() -> Actor {
        Actor::User(UserId::new())
    }

    #[tokio::test]
    async fn set_resolve_rotate_delete_and_audit() {
        for choice in [SecretBackendChoice::Auto, SecretBackendChoice::Vault, SecretBackendChoice::VaultFileKey, SecretBackendChoice::Memory] {
            let f = fx(choice).await;
            let value = format!("sk-test-value-{choice:?}-0123456789");
            let r = f.svc.set("OPENAI_API_KEY", SecretScope::Global, Zeroizing::new(value.clone()), user()).await.unwrap();
            let expected_backend = if choice == SecretBackendChoice::Auto { SecretBackend::OsKeyring } else { SecretBackend::EncryptedVault };
            if choice != SecretBackendChoice::Memory {
                assert_eq!(r.backend, expected_backend, "{choice:?}");
            }
            assert_eq!(f.svc.list(None).await.unwrap().len(), 1);
            let got = f.svc.resolve(r.id, Actor::System, AccessContext::default()).await.unwrap();
            assert_eq!(got.as_str(), value);
            assert!(f.svc.last_accessed(r.id).await.unwrap().is_some());
            assert!(!redact::redact(&format!("leak {value}")).contains(&value), "stored values are redacted");
            // Rotation keeps the id.
            let r2 = f.svc.set("OPENAI_API_KEY", SecretScope::Global, Zeroizing::new("rotated-value-123".into()), user()).await.unwrap();
            assert_eq!(r2.id, r.id);
            assert!(r2.rotated_at.is_some());
            assert_eq!(f.svc.resolve_by_name("OPENAI_API_KEY", Actor::System, AccessContext::default()).await.unwrap().as_str(), "rotated-value-123");
            assert!(f.svc.delete(r.id, user()).await.unwrap());
            assert!(!f.svc.delete(r.id, user()).await.unwrap());
            assert!(f.svc.resolve(r.id, Actor::System, AccessContext::default()).await.is_err());
            let actions: Vec<String> = f.audit.query(Default::default()).await.unwrap().into_iter().map(|r| r.event.action).collect();
            for a in ["secrets.set", "secrets.read", "secrets.delete"] {
                assert!(actions.iter().any(|x| x == a), "{choice:?} missing audit {a}");
            }
            // Values never reach the audit log.
            let all = serde_json::to_string(&f.audit.query(Default::default()).await.unwrap().iter().map(|r| r.event.clone()).collect::<Vec<_>>()).unwrap();
            assert!(!all.contains(&value));
        }
    }

    #[tokio::test]
    async fn scope_checks_and_validation() {
        let f = fx(SecretBackendChoice::Memory).await;
        let p = ProjectId::new();
        let r = f.svc.set("DB_URL", SecretScope::Project { project_id: p }, Zeroizing::new("postgres://x".into()), user()).await.unwrap();
        assert!(matches!(
            f.svc.resolve(r.id, Actor::System, AccessContext { project_id: Some(ProjectId::new()) }).await,
            Err(SecurityError::Forbidden(_))
        ));
        assert!(f.svc.resolve(r.id, Actor::System, AccessContext { project_id: Some(p) }).await.is_ok());
        assert_eq!(f.svc.list(Some(ProjectId::new())).await.unwrap().len(), 0);
        assert_eq!(f.svc.list(Some(p)).await.unwrap().len(), 1);
        for bad in ["", "1abc", "has space", &"x".repeat(200)] {
            assert!(f.svc.set(bad, SecretScope::Global, Zeroizing::new("v".into()), user()).await.is_err(), "{bad}");
        }
        assert!(f.svc.set("EMPTY", SecretScope::Global, Zeroizing::new(String::new()), user()).await.is_err());
        assert!(matches!(
            f.svc.set("X", SecretScope::Global, Zeroizing::new("v".into()), Actor::Agent(AgentId::new())).await,
            Err(SecurityError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn lock_blocks_access_until_unlock() {
        let f = fx(SecretBackendChoice::Vault).await;
        let r = f.svc.set("TOKEN", SecretScope::Global, Zeroizing::new("tok-123456789".into()), user()).await.unwrap();
        let st = f.svc.lock(user()).await.unwrap();
        assert!(st.locked);
        assert!(matches!(f.svc.resolve(r.id, Actor::System, AccessContext::default()).await, Err(SecurityError::VaultLocked)));
        assert!(matches!(f.svc.set("B", SecretScope::Global, Zeroizing::new("v".into()), user()).await, Err(SecurityError::VaultLocked)));
        assert_eq!(f.svc.preload_redaction().await.unwrap(), 0);
        let st = f.svc.unlock(user()).await.unwrap();
        assert!(!st.locked);
        assert_eq!(st.secret_count, 1);
        assert_eq!(f.svc.resolve(r.id, Actor::System, AccessContext::default()).await.unwrap().as_str(), "tok-123456789");
        assert_eq!(f.svc.preload_redaction().await.unwrap(), 1);
        let denied = f.audit.query(Default::default()).await.unwrap();
        assert!(denied.iter().any(|r| r.event.action == "secrets.read" && r.event.outcome == AuditOutcome::Denied));
    }
}
