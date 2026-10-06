//! # orkestr-security
//!
//! Security subsystem of the Orkestr core (MASTER_SPEC §23, §24, §52;
//! contract §6; permission model `docs/security/permission-model.md`):
//!
//! * [`guard`] — [`DangerousActionGuard`]: classifies shell commands
//!   (PowerShell / cmd / bash / sh) and structured actions into
//!   `Safe | Dangerous{category} | Forbidden{category}`.
//! * [`paths`] — [`confine`]: filesystem confinement (traversal, absolute,
//!   UNC, `\\?\`, drive-relative, device names, ADS, symlink/junction escapes).
//! * [`permissions`] — [`RuleEngine`] (`check(actor, capability, resource,
//!   ctx) -> Allow | Ask | Deny`) and persisted rules / agent mode overrides.
//! * [`approvals`] — [`ApprovalService`]: request / wait / resolve / expire.
//! * [`secrets`] — [`SecretService`] over the OS keyring and an encrypted
//!   file vault; lock/unlock; audit; redaction registration.
//! * [`redact`] — process-global redaction (registered secret values + patterns).
//! * [`audit`] — append-only, hash-chained audit log.
//! * [`emergency`] — [`EmergencyController`]: cancellation-token registry and
//!   emergency flags (Stop All / project / agent, disable FullAuto, disable
//!   network tools, vault locked).
//!
//! [`Security`] wires everything over one database and event bus; the core
//! constructs it once and exposes it to RPC handlers.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod approvals;
pub mod audit;
pub mod emergency;
pub mod error;
pub mod guard;
pub mod paths;
pub mod permissions;
pub mod redact;
pub mod secrets;

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_domain::{Actor, AgentId, ProjectId, UserId};
use orkestr_events::EventBus;
use orkestr_platform::CredentialStore;
use orkestr_storage::rusqlite::OptionalExtension;
use orkestr_storage::{Db, Migration};

pub use approvals::{ApprovalDecision, ApprovalFilter, ApprovalRequest, ApprovalService, CancelScope};
pub use audit::{AuditLog, AuditQuery, AuditRecord, NewAudit};
pub use emergency::{EmergencyController, EmergencyFlags, SharedFlags, StopReport};
pub use error::{SecurityError, SecurityResult};
pub use guard::{Action, DangerClass, DangerousActionGuard, GuardConfig, ShellKind};
pub use paths::{confine, confine_relative, PathError};
pub use permissions::{
    AskReason, CheckContext, Decision, DenyReason, PermissionEngine, PermissionService, Resource, RuleEngine,
};
pub use redact::{GlobalRedactor, Redactor, StreamRedactor};
pub use secrets::{AccessContext, SecretBackendChoice, SecretService, SecretStore, VaultStatus};

/// All security migrations, in order (the core appends them to its list).
#[must_use]
pub fn migrations() -> Vec<Migration> {
    vec![permissions::MIGRATION, audit::MIGRATION, emergency::MIGRATION, approvals::MIGRATION, secrets::MIGRATION]
}

/// Configuration for [`Security::open`].
pub struct SecurityConfig {
    /// Migrated database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Orkestr data dir (vault location; protected from agents).
    pub data_dir: PathBuf,
    /// OS credential store (keyring). `None` disables keyring use entirely.
    pub credentials: Option<Arc<dyn CredentialStore>>,
    /// Secret value backend preference.
    pub secret_backend: SecretBackendChoice,
    /// Guard configuration (data dir is added to the protected paths).
    pub guard: GuardConfig,
}

impl std::fmt::Debug for SecurityConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityConfig")
            .field("data_dir", &self.data_dir)
            .field("secret_backend", &self.secret_backend)
            .finish_non_exhaustive()
    }
}

/// Result of an emergency stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyOutcome {
    /// Tokens cancelled.
    pub report: StopReport,
    /// Pending approvals cancelled.
    pub approvals_cancelled: u32,
}

/// The wired security subsystem.
pub struct Security {
    /// Guard.
    pub guard: Arc<DangerousActionGuard>,
    /// Permission engine.
    pub engine: Arc<RuleEngine>,
    /// Rules / modes.
    pub permissions: Arc<PermissionService>,
    /// Approvals.
    pub approvals: Arc<ApprovalService>,
    /// Secrets.
    pub secrets: Arc<SecretService>,
    /// Audit log.
    pub audit: Arc<AuditLog>,
    /// Emergency controls.
    pub emergency: Arc<EmergencyController>,
    /// The local user (resolver of approvals in local sessions).
    pub local_user: UserId,
}

impl std::fmt::Debug for Security {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Security").field("local_user", &self.local_user).finish_non_exhaustive()
    }
}

async fn load_or_create_local_user(db: &Db) -> SecurityResult<UserId> {
    let id = db
        .call(|c| {
            let existing: Option<String> = c
                .query_row("SELECT value FROM security_meta WHERE key = 'local_user_id'", [], |r| r.get(0))
                .optional()?;
            if let Some(v) = existing {
                return UserId::parse(&v).map_err(|e| orkestr_storage::StorageError::Corrupt(e.to_string()));
            }
            let id = UserId::new();
            c.execute("INSERT INTO security_meta (key, value) VALUES ('local_user_id', ?1)", [id.to_string()])?;
            Ok(id)
        })
        .await?;
    Ok(id)
}

impl Security {
    /// Wires the subsystem. The database must already contain [`migrations`].
    pub async fn open(cfg: SecurityConfig) -> SecurityResult<Self> {
        let mut guard_cfg = cfg.guard;
        if !guard_cfg.protected_paths.contains(&cfg.data_dir) {
            guard_cfg.protected_paths.push(cfg.data_dir.clone());
        }
        let guard = Arc::new(DangerousActionGuard::with_config(guard_cfg));
        let audit = Arc::new(AuditLog::new(cfg.db.clone()));
        let emergency = Arc::new(EmergencyController::open(cfg.db.clone(), cfg.events.clone()).await?);
        let engine = Arc::new(RuleEngine::new(Arc::clone(&guard), emergency.shared_flags()));
        let permissions =
            Arc::new(PermissionService::open(cfg.db.clone(), Arc::clone(&engine), Arc::clone(&audit)).await?);
        let approvals = Arc::new(ApprovalService::open(cfg.db.clone(), cfg.events.clone(), Arc::clone(&audit)).await?);
        let prefix = secrets::stores::data_dir_hash(&cfg.data_dir);
        let use_keyring = matches!(cfg.secret_backend, SecretBackendChoice::Auto | SecretBackendChoice::Vault);
        let creds = if use_keyring { cfg.credentials.clone() } else { None };
        let keyring: Option<Arc<dyn SecretStore>> =
            creds.clone().map(|c| Arc::new(secrets::KeyringStore::new(c, prefix.clone())) as Arc<dyn SecretStore>);
        let vault = Arc::new(secrets::EncryptedFileVault::new(cfg.data_dir.join("vault"), creds, prefix));
        let secrets = Arc::new(SecretService::new(
            cfg.db.clone(),
            keyring,
            vault,
            cfg.secret_backend,
            Arc::clone(&audit),
            Arc::clone(&emergency),
        ));
        let local_user = load_or_create_local_user(&cfg.db).await?;
        Ok(Self { guard, engine, permissions, approvals, secrets, audit, emergency, local_user })
    }

    /// The local user as an actor.
    #[must_use]
    pub fn user(&self) -> Actor {
        Actor::User(self.local_user)
    }

    async fn audit_emergency(
        &self,
        by: Actor,
        action: &str,
        resource: String,
        project: Option<ProjectId>,
        details: serde_json::Value,
    ) {
        self.audit
            .record(
                NewAudit::new(by, action, resource, orkestr_domain::AuditOutcome::Executed)
                    .project(project)
                    .details(details),
            )
            .await;
    }

    /// Stop All AI (+ cancel every pending approval).
    pub async fn stop_all(&self, by: Actor) -> SecurityResult<EmergencyOutcome> {
        let report = self.emergency.stop_all(by).await?;
        let approvals_cancelled = self.approvals.cancel_scope(CancelScope::All, "emergency stop").await.unwrap_or(0);
        self.audit_emergency(
            by,
            "emergency.stopAll",
            "all".into(),
            None,
            serde_json::json!({ "report": report, "approvalsCancelled": approvals_cancelled }),
        )
        .await;
        Ok(EmergencyOutcome { report, approvals_cancelled })
    }

    /// Stop one project's AI work (+ its pending approvals).
    pub async fn stop_project(&self, project: ProjectId, by: Actor) -> SecurityResult<EmergencyOutcome> {
        let report = self.emergency.stop_project(project, by).await?;
        let approvals_cancelled =
            self.approvals.cancel_scope(CancelScope::Project(project), "emergency stop").await.unwrap_or(0);
        self.audit_emergency(
            by,
            "emergency.stopProject",
            format!("project:{project}"),
            Some(project),
            serde_json::json!({ "report": report }),
        )
        .await;
        Ok(EmergencyOutcome { report, approvals_cancelled })
    }

    /// Stop one agent (+ its pending approvals).
    pub async fn stop_agent(&self, agent: AgentId, by: Actor) -> SecurityResult<EmergencyOutcome> {
        let report = self.emergency.stop_agent(agent, by).await?;
        let approvals_cancelled =
            self.approvals.cancel_scope(CancelScope::Agent(agent), "emergency stop").await.unwrap_or(0);
        self.audit_emergency(
            by,
            "emergency.stopAgent",
            format!("agent:{agent}"),
            None,
            serde_json::json!({ "report": report }),
        )
        .await;
        Ok(EmergencyOutcome { report, approvals_cancelled })
    }

    /// Sets/clears `fullAutoDisabled` (audited).
    pub async fn disable_full_auto(&self, disabled: bool, by: Actor) -> SecurityResult<EmergencyFlags> {
        let flags = self.emergency.set_full_auto_disabled(disabled).await?;
        self.audit_emergency(
            by,
            "emergency.disableFullAuto",
            "all".into(),
            None,
            serde_json::json!({ "disabled": disabled }),
        )
        .await;
        Ok(flags)
    }

    /// Sets/clears `networkToolsDisabled` (audited).
    pub async fn disable_network_tools(&self, disabled: bool, by: Actor) -> SecurityResult<EmergencyFlags> {
        let flags = self.emergency.set_network_tools_disabled(disabled).await?;
        self.audit_emergency(
            by,
            "emergency.disableNetworkTools",
            "all".into(),
            None,
            serde_json::json!({ "disabled": disabled }),
        )
        .await;
        Ok(flags)
    }

    /// Re-arms emergency controls (audited).
    pub async fn reset_emergency(&self, by: Actor) -> SecurityResult<EmergencyFlags> {
        let flags = self.emergency.reset().await?;
        self.audit_emergency(by, "emergency.reset", "all".into(), None, serde_json::Value::Null).await;
        Ok(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{ApprovalStatus, Capability, RiskLevel};
    use orkestr_platform::MemoryCredentialStore;

    #[tokio::test]
    async fn open_and_stop_all_cancels_approvals() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("o.db")).unwrap();
        db.migrate(migrations()).await.unwrap();
        let sec = Security::open(SecurityConfig {
            db: db.clone(),
            events: EventBus::in_memory(64),
            data_dir: dir.path().to_path_buf(),
            credentials: Some(Arc::new(MemoryCredentialStore::default())),
            secret_backend: SecretBackendChoice::Memory,
            guard: GuardConfig::default(),
        })
        .await
        .unwrap();
        // Data dir is protected from agents.
        let cmd = format!("rm -f {}", dir.path().join("orkestr.db").display());
        assert!(sec.guard.classify(&cmd, ShellKind::Bash).is_forbidden());
        let a = sec
            .approvals
            .request(ApprovalRequest::new(Capability::Network, "fetch", "mode", RiskLevel::Medium))
            .await
            .unwrap();
        let token = sec.emergency.register_agent(AgentId::new(), None).unwrap();
        let out = sec.stop_all(sec.user()).await.unwrap();
        assert_eq!(out.report.agents_stopped, 1);
        assert_eq!(out.approvals_cancelled, 1);
        assert!(token.is_cancelled());
        assert_eq!(sec.approvals.get(a.id).await.unwrap().unwrap().status, ApprovalStatus::Cancelled);
        // Local user id is stable.
        let user = sec.local_user;
        assert_eq!(load_or_create_local_user(&db).await.unwrap(), user);
        assert!(sec
            .audit
            .query(AuditQuery::default())
            .await
            .unwrap()
            .iter()
            .any(|r| r.event.action == "emergency.stopAll"));
    }

    #[tokio::test]
    async fn migrations_apply_and_revert() {
        let db = Db::open_in_memory().unwrap();
        let n = migrations().len();
        db.migrate(migrations()).await.unwrap();
        let reverted = db.call(move |c| orkestr_storage::Migrator::rollback_last(c, n)).await.unwrap();
        assert_eq!(reverted.len(), n);
    }
}
