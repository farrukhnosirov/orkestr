//! Persisted permission rules and agent approval-mode overrides.
//!
//! Rules live in `permission_rules`; every change reloads the engine's
//! immutable snapshot and is audited. Only users create rules (hard policy
//! P1) — the RPC layer passes the local user as the actor.

use std::sync::Arc;

use orkestr_domain::{
    now, Actor, AgentId, ApprovalMode, AuditOutcome, PermissionRule, PermissionRuleRecord, PermissionScope, ProjectId, TaskId,
    Timestamp,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};

use super::engine::{compile_pattern, RuleEngine};
use crate::audit::{AuditLog, NewAudit};
use crate::error::{SecurityError, SecurityResult};

/// Migration creating the rules and mode-override tables.
pub const MIGRATION: Migration = Migration {
    id: "security_0001_permissions",
    owner: "security",
    up: r"
CREATE TABLE security_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
CREATE TABLE permission_rules (
    id TEXT PRIMARY KEY NOT NULL,
    capability TEXT NOT NULL,
    effect TEXT NOT NULL,
    resource TEXT,
    scope_kind TEXT NOT NULL,
    scope_id TEXT,
    doc TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE INDEX permission_rules_scope ON permission_rules(scope_kind, scope_id);
CREATE TABLE agent_approval_modes (
    agent_id TEXT PRIMARY KEY NOT NULL,
    mode TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
",
    down: r"
DROP TABLE IF EXISTS agent_approval_modes;
DROP TABLE IF EXISTS permission_rules;
DROP TABLE IF EXISTS security_meta;
",
};

/// Filter for listing rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleFilter {
    /// Rules that apply to this project (Global + Project scope).
    pub project_id: Option<ProjectId>,
    /// Also include this agent's rules.
    pub agent_id: Option<AgentId>,
    /// Also include this task's rules.
    pub task_id: Option<TaskId>,
}

/// Maximum rules (sanity limit).
pub const MAX_RULES: usize = 10_000;

/// Rule persistence + engine reload.
pub struct PermissionService {
    db: Db,
    engine: Arc<RuleEngine>,
    audit: Arc<AuditLog>,
}

impl std::fmt::Debug for PermissionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionService").finish_non_exhaustive()
    }
}

fn scope_parts(scope: &PermissionScope) -> (&'static str, Option<String>) {
    match scope {
        PermissionScope::Global => ("global", None),
        PermissionScope::Project { project_id } => ("project", Some(project_id.to_string())),
        PermissionScope::Agent { agent_id } => ("agent", Some(agent_id.to_string())),
        PermissionScope::Task { task_id } => ("task", Some(task_id.to_string())),
    }
}

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

impl PermissionService {
    /// Creates the service and loads rules into the engine.
    pub async fn open(db: Db, engine: Arc<RuleEngine>, audit: Arc<AuditLog>) -> SecurityResult<Self> {
        let svc = Self { db, engine, audit };
        svc.reload().await?;
        Ok(svc)
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Arc<RuleEngine> {
        &self.engine
    }

    async fn load_all(&self) -> SecurityResult<Vec<PermissionRuleRecord>> {
        let docs: Vec<String> = self
            .db
            .call(|c| {
                let mut stmt = c.prepare("SELECT doc FROM permission_rules ORDER BY created_at, id")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::with_capacity(docs.len());
        for d in docs {
            match serde_json::from_str::<PermissionRuleRecord>(&d) {
                Ok(r) => out.push(r),
                Err(e) => tracing::error!(error = %e, "corrupt permission rule row skipped"),
            }
        }
        Ok(out)
    }

    /// Reloads the engine snapshot from the database.
    pub async fn reload(&self) -> SecurityResult<()> {
        let rules = self.load_all().await?;
        let n = rules.len();
        self.engine.set_rules(rules);
        tracing::debug!(rules = n, "permission rules loaded");
        Ok(())
    }

    /// Lists rules. With a filter: Global rules plus the rules of the given
    /// project/agent/task.
    pub async fn list(&self, filter: &RuleFilter) -> SecurityResult<Vec<PermissionRuleRecord>> {
        let all = self.load_all().await?;
        if filter == &RuleFilter::default() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|r| match r.rule.scope {
                PermissionScope::Global => true,
                PermissionScope::Project { project_id } => filter.project_id == Some(project_id),
                PermissionScope::Agent { agent_id } => filter.agent_id == Some(agent_id),
                PermissionScope::Task { task_id } => filter.task_id == Some(task_id),
            })
            .collect())
    }

    /// Creates (`id = None`) or replaces a rule.
    pub async fn set(
        &self,
        id: Option<String>,
        rule: PermissionRule,
        note: Option<String>,
        expires_at: Option<Timestamp>,
        by: Actor,
    ) -> SecurityResult<PermissionRuleRecord> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot change permission rules (P1)".into()));
        }
        compile_pattern(rule.resource.as_deref()).map_err(SecurityError::Invalid)?;
        if let Some(r) = &rule.resource {
            if r.len() > 1024 {
                return Err(SecurityError::Invalid("resource pattern too long (max 1024)".into()));
            }
        }
        let note = note.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        if note.as_ref().is_some_and(|n| n.chars().count() > 500) {
            return Err(SecurityError::Invalid("note too long (max 500)".into()));
        }
        let existing_created = match &id {
            Some(id) => Some(
                self.get(id).await?.ok_or_else(|| SecurityError::NotFound(format!("permission rule {id}")))?.created_at,
            ),
            None => None,
        };
        if id.is_none() && self.load_all().await?.len() >= MAX_RULES {
            return Err(SecurityError::Conflict(format!("too many rules (max {MAX_RULES})")));
        }
        let record = PermissionRuleRecord {
            id: id.unwrap_or_else(uuid_v7_string),
            rule,
            note,
            created_at: existing_created.unwrap_or_else(now),
            expires_at,
        };
        let doc = serde_json::to_string(&record)?;
        let (kind, scope_id) = scope_parts(&record.rule.scope);
        let rec = record.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO permission_rules (id, capability, effect, resource, scope_kind, scope_id, doc, created_at, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(id) DO UPDATE SET capability = excluded.capability, effect = excluded.effect,
                       resource = excluded.resource, scope_kind = excluded.scope_kind, scope_id = excluded.scope_id,
                       doc = excluded.doc, expires_at = excluded.expires_at",
                    params![
                        rec.id,
                        enum_str(&rec.rule.capability),
                        enum_str(&rec.rule.effect),
                        rec.rule.resource,
                        kind,
                        scope_id,
                        doc,
                        rec.created_at.to_rfc3339(),
                        rec.expires_at.map(|e| e.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .await?;
        self.reload().await?;
        self.audit
            .record(
                NewAudit::new(by, "permissions.set", record.rule.resource.clone().unwrap_or_else(|| "*".into()), AuditOutcome::Executed)
                    .capability(record.rule.capability)
                    .project(match record.rule.scope {
                        PermissionScope::Project { project_id } => Some(project_id),
                        _ => None,
                    })
                    .details(serde_json::to_value(&record).unwrap_or_default()),
            )
            .await;
        Ok(record)
    }

    /// One rule.
    pub async fn get(&self, id: &str) -> SecurityResult<Option<PermissionRuleRecord>> {
        let id = id.to_owned();
        let doc: Option<String> = self
            .db
            .call(move |c| Ok(c.query_row("SELECT doc FROM permission_rules WHERE id = ?1", [id], |r| r.get(0)).optional()?))
            .await?;
        doc.map(|d| serde_json::from_str(&d).map_err(|e| SecurityError::Storage(StorageError::Json(e)))).transpose()
    }

    /// Deletes a rule. Returns whether it existed.
    pub async fn delete(&self, id: &str, by: Actor) -> SecurityResult<bool> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot change permission rules (P1)".into()));
        }
        let existing = self.get(id).await?;
        let id_owned = id.to_owned();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM permission_rules WHERE id = ?1", [id_owned])?)).await?;
        if n > 0 {
            self.reload().await?;
            if let Some(rec) = existing {
                self.audit
                    .record(
                        NewAudit::new(by, "permissions.delete", rec.rule.resource.clone().unwrap_or_else(|| "*".into()), AuditOutcome::Executed)
                            .capability(rec.rule.capability)
                            .details(serde_json::json!({ "ruleId": rec.id })),
                    )
                    .await;
            }
        }
        Ok(n > 0)
    }

    /// The approval-mode override of an agent.
    pub async fn agent_mode(&self, agent: AgentId) -> SecurityResult<Option<ApprovalMode>> {
        let id = agent.to_string();
        let mode: Option<String> = self
            .db
            .call(move |c| Ok(c.query_row("SELECT mode FROM agent_approval_modes WHERE agent_id = ?1", [id], |r| r.get(0)).optional()?))
            .await?;
        Ok(mode.and_then(|m| serde_json::from_value(serde_json::Value::String(m)).ok()))
    }

    /// Sets (`Some`) or clears (`None`) an agent's approval-mode override.
    pub async fn set_agent_mode(&self, agent: AgentId, mode: Option<ApprovalMode>, by: Actor) -> SecurityResult<()> {
        if matches!(by, Actor::Agent(_) | Actor::Manager) {
            return Err(SecurityError::Forbidden("agents cannot change approval modes (P1)".into()));
        }
        let id = agent.to_string();
        let m = mode.map(|m| enum_str(&m));
        let ts = now().to_rfc3339();
        self.db
            .call(move |c| {
                match m {
                    Some(m) => c.execute(
                        "INSERT INTO agent_approval_modes (agent_id, mode, updated_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(agent_id) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
                        params![id, m, ts],
                    )?,
                    None => c.execute("DELETE FROM agent_approval_modes WHERE agent_id = ?1", [id])?,
                };
                Ok(())
            })
            .await?;
        self.audit
            .record(
                NewAudit::new(by, "permissions.setMode", format!("agent:{agent}"), AuditOutcome::Executed)
                    .details(serde_json::json!({ "mode": mode })),
            )
            .await;
        Ok(())
    }

    /// Effective approval mode: agent override, else project mode, capped by
    /// the emergency `fullAutoDisabled` flag.
    pub async fn effective_mode(&self, project_mode: ApprovalMode, agent: Option<AgentId>) -> SecurityResult<ApprovalMode> {
        let base = match agent {
            Some(a) => self.agent_mode(a).await?.unwrap_or(project_mode),
            None => project_mode,
        };
        Ok(self.engine.effective_mode(base))
    }
}

/// A fresh UUID v7 string (via a domain id type).
pub(crate) fn uuid_v7_string() -> String {
    orkestr_domain::EventId::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emergency::SharedFlags;
    use crate::guard::DangerousActionGuard;
    use crate::permissions::{CheckContext, PermissionEngine, Resource};
    use orkestr_domain::{Capability, PermissionEffect, UserId};

    async fn svc() -> PermissionService {
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::migrations()).await.unwrap();
        let engine = Arc::new(RuleEngine::new(Arc::new(DangerousActionGuard::new()), Arc::new(SharedFlags::default())));
        PermissionService::open(db.clone(), engine, Arc::new(AuditLog::new(db))).await.unwrap()
    }

    fn user() -> Actor {
        Actor::User(UserId::new())
    }

    #[tokio::test]
    async fn crud_reloads_engine_and_audits() {
        let s = svc().await;
        let p = ProjectId::new();
        let rule = PermissionRule {
            capability: Capability::Network,
            effect: PermissionEffect::Allow,
            resource: Some("https://api.example.com/*".into()),
            scope: PermissionScope::Project { project_id: p },
        };
        let rec = s.set(None, rule.clone(), Some(" api ".into()), None, user()).await.unwrap();
        assert_eq!(rec.note.as_deref(), Some("api"));
        assert_eq!(s.engine.rule_count(), 1);
        let ctx = CheckContext { project_id: Some(p), ..Default::default() };
        let a = Actor::Agent(AgentId::new());
        assert!(s.engine.check(&a, Capability::Network, &Resource::Url { url: "https://api.example.com/v1".into() }, &ctx).is_allow());

        // Update keeps id and created_at.
        let mut changed = rule.clone();
        changed.effect = PermissionEffect::Deny;
        let rec2 = s.set(Some(rec.id.clone()), changed, None, None, user()).await.unwrap();
        assert_eq!(rec2.id, rec.id);
        assert_eq!(rec2.created_at, rec.created_at);
        assert!(s.engine.check(&a, Capability::Network, &Resource::Url { url: "https://api.example.com/v1".into() }, &ctx).is_deny());

        assert_eq!(s.list(&RuleFilter::default()).await.unwrap().len(), 1);
        assert_eq!(s.list(&RuleFilter { project_id: Some(ProjectId::new()), ..Default::default() }).await.unwrap().len(), 0);
        assert!(s.delete(&rec.id, user()).await.unwrap());
        assert!(!s.delete(&rec.id, user()).await.unwrap());
        assert_eq!(s.engine.rule_count(), 0);
        let audit = s.audit.query(Default::default()).await.unwrap();
        assert!(audit.iter().any(|r| r.event.action == "permissions.delete"));
        assert!(audit.iter().filter(|r| r.event.action == "permissions.set").count() == 2);
    }

    #[tokio::test]
    async fn validation_and_agent_restrictions() {
        let s = svc().await;
        let bad = PermissionRule { capability: Capability::Write, effect: PermissionEffect::Allow, resource: Some("[".into()), scope: PermissionScope::Global };
        assert!(matches!(s.set(None, bad, None, None, user()).await, Err(SecurityError::Invalid(_))));
        let ok = PermissionRule { capability: Capability::Write, effect: PermissionEffect::Allow, resource: None, scope: PermissionScope::Global };
        assert!(matches!(s.set(None, ok.clone(), None, None, Actor::Agent(AgentId::new())).await, Err(SecurityError::Forbidden(_))));
        assert!(matches!(s.set(None, ok, None, None, Actor::Manager).await, Err(SecurityError::Forbidden(_))));
        assert!(matches!(s.set(Some("nope".into()), PermissionRule { capability: Capability::Read, effect: PermissionEffect::Allow, resource: None, scope: PermissionScope::Global }, None, None, user()).await, Err(SecurityError::NotFound(_))));
    }

    #[tokio::test]
    async fn agent_mode_overrides() {
        let s = svc().await;
        let a = AgentId::new();
        assert_eq!(s.effective_mode(ApprovalMode::Ask, Some(a)).await.unwrap(), ApprovalMode::Ask);
        s.set_agent_mode(a, Some(ApprovalMode::FullAuto), user()).await.unwrap();
        assert_eq!(s.agent_mode(a).await.unwrap(), Some(ApprovalMode::FullAuto));
        assert_eq!(s.effective_mode(ApprovalMode::Ask, Some(a)).await.unwrap(), ApprovalMode::FullAuto);
        assert_eq!(s.effective_mode(ApprovalMode::SemiAuto, None).await.unwrap(), ApprovalMode::SemiAuto);
        s.set_agent_mode(a, None, user()).await.unwrap();
        assert_eq!(s.agent_mode(a).await.unwrap(), None);
        assert!(s.set_agent_mode(a, Some(ApprovalMode::FullAuto), Actor::Agent(a)).await.is_err());
    }
}
