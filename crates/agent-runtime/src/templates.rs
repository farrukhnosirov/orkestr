//! Agent templates (§6): validation, persistence, built-in (official)
//! templates, import/export (Marketplace foundation).
//!
//! Built-ins ship as JSON resources in `crates/agent-runtime/templates/` in
//! the very same [`TemplateExport`] envelope that `templates.export` writes,
//! so every built-in is also a working import example. They are seeded on
//! start (inserted when missing, refreshed when their version changes) and are
//! read-only: users duplicate them to customise.

use std::collections::BTreeSet;

use orkestr_domain::{
    now, AgentRole, AgentTemplate, AgentTemplateId, PermissionRule, PermissionScope, TemplateSource, Timestamp,
};
use orkestr_protocol::methods::templates::{
    TemplateDraft, TemplateExport, TemplateOrigin, TEMPLATE_EXPORT_FORMAT, TEMPLATE_SCHEMA_VERSION,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::Db;

use crate::error::{AgentError, AgentResult};
use crate::tools::is_known_tool;

/// Built-in template resources (`file name`, JSON).
pub const BUILTIN_FILES: [(&str, &str); 11] = [
    ("architect.json", include_str!("../templates/architect.json")),
    ("backend.json", include_str!("../templates/backend.json")),
    ("frontend.json", include_str!("../templates/frontend.json")),
    ("database.json", include_str!("../templates/database.json")),
    ("qa.json", include_str!("../templates/qa.json")),
    ("security.json", include_str!("../templates/security.json")),
    ("devops.json", include_str!("../templates/devops.json")),
    ("uiux.json", include_str!("../templates/uiux.json")),
    ("product-manager.json", include_str!("../templates/product-manager.json")),
    ("reviewer.json", include_str!("../templates/reviewer.json")),
    ("researcher.json", include_str!("../templates/researcher.json")),
];

/// Max size of an import document.
pub const MAX_IMPORT_BYTES: usize = 256 * 1024;
const MAX_NAME: usize = 80;
const MAX_DESCRIPTION: usize = 500;
const MAX_PROMPT: usize = 32_000;
const MAX_PERMISSIONS: usize = 64;
const MAX_FALLBACKS: usize = 8;
/// Turn limit bounds (never unlimited, §64).
pub const MAX_TURNS_LIMIT: u32 = 500;

fn invalid(m: impl Into<String>) -> AgentError {
    AgentError::Invalid(m.into())
}

fn is_semver(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 6 && p.chars().all(|c| c.is_ascii_digit()))
}

/// Validates and normalizes a draft (trims text, deduplicates tools, always
/// offers `submit_result`, permissions are stored scope-less = `Global` and
/// re-scoped to the agent at spawn).
pub fn validate_draft(mut d: TemplateDraft) -> AgentResult<TemplateDraft> {
    d.name = d.name.trim().to_owned();
    if d.name.is_empty() || d.name.chars().count() > MAX_NAME || d.name.chars().any(char::is_control) {
        return Err(invalid(format!("name must be 1..={MAX_NAME} printable characters")));
    }
    d.description = d.description.trim().to_owned();
    if d.description.chars().count() > MAX_DESCRIPTION {
        return Err(invalid(format!("description is limited to {MAX_DESCRIPTION} characters")));
    }
    if let AgentRole::Custom(r) = &d.role {
        let r = r.trim();
        if r.is_empty() || r.chars().count() > 40 {
            return Err(invalid("custom role must be 1..=40 characters"));
        }
        d.role = AgentRole::Custom(r.to_owned());
    }
    if d.system_prompt.trim().is_empty() || d.system_prompt.chars().count() > MAX_PROMPT {
        return Err(invalid(format!("system prompt must be 1..={MAX_PROMPT} characters")));
    }
    let mut seen = BTreeSet::new();
    let mut tools = Vec::new();
    for t in &d.tools {
        let t = t.trim();
        if !is_known_tool(t) {
            return Err(invalid(format!("unknown tool `{t}`")));
        }
        if seen.insert(t.to_owned()) {
            tools.push(t.to_owned());
        }
    }
    if !seen.contains("submit_result") {
        tools.push("submit_result".to_owned());
    }
    d.tools = tools;
    if d.max_turns == 0 || d.max_turns > MAX_TURNS_LIMIT {
        return Err(invalid(format!("maxTurns must be 1..={MAX_TURNS_LIMIT}")));
    }
    d.version = d.version.trim().to_owned();
    if !is_semver(&d.version) {
        return Err(invalid("version must look like MAJOR.MINOR.PATCH (e.g. 1.0.0)"));
    }
    if d.fallback_models.len() > MAX_FALLBACKS {
        return Err(invalid(format!("at most {MAX_FALLBACKS} fallback models")));
    }
    for m in d.preferred_model.iter().chain(d.fallback_models.iter()) {
        if m.model.trim().is_empty() || m.model.len() > 200 {
            return Err(invalid("model ids must be 1..=200 characters"));
        }
    }
    if let Some(b) = &d.budget {
        if b.max_cost_usd.is_some_and(|c| !c.is_finite() || c < 0.0) {
            return Err(invalid("budget.maxCostUsd must be a non-negative amount"));
        }
        if !(b.warn_at.is_finite() && b.warn_at > 0.0 && b.warn_at <= 1.0) {
            return Err(invalid("budget.warnAt must be in (0, 1]"));
        }
        if b.max_duration_secs == Some(0) || b.max_tokens == Some(0) {
            return Err(invalid("budget limits must be positive (omit a limit to leave it open)"));
        }
    }
    if d.permissions.len() > MAX_PERMISSIONS {
        return Err(invalid(format!("at most {MAX_PERMISSIONS} permission rules")));
    }
    for p in &mut d.permissions {
        if let Some(r) = &p.resource {
            if r.len() > 1024 {
                return Err(invalid("permission resource pattern too long (max 1024)"));
            }
        }
        orkestr_security::permissions::compile_pattern(p.resource.as_deref())
            .map_err(|e| invalid(format!("invalid permission pattern: {e}")))?;
        p.scope = PermissionScope::Global;
    }
    if let Some(a) = &d.avatar {
        if !(a.scale.is_finite() && (0.25..=4.0).contains(&a.scale)) || a.robot.trim().is_empty() || a.robot.len() > 80
        {
            return Err(invalid("avatar.robot must be 1..=80 chars and avatar.scale within 0.25..4"));
        }
    }
    Ok(d)
}

fn template_from(id: AgentTemplateId, d: TemplateDraft, source: TemplateSource, created: Timestamp) -> AgentTemplate {
    AgentTemplate {
        id,
        name: d.name,
        role: d.role,
        description: d.description,
        system_prompt: d.system_prompt,
        preferred_model: d.preferred_model,
        fallback_models: d.fallback_models,
        tools: d.tools,
        permissions: d.permissions,
        memory_access: d.memory_access,
        budget: d.budget,
        max_turns: d.max_turns,
        worktree_policy: d.worktree_policy,
        quality_gates: d.quality_gates,
        voice: d.voice,
        avatar: d.avatar,
        visual_state: d.visual_state,
        source,
        version: d.version,
        created_at: created,
        updated_at: now(),
    }
}

/// The editable fields of a template.
#[must_use]
pub fn draft_of(t: &AgentTemplate) -> TemplateDraft {
    TemplateDraft {
        name: t.name.clone(),
        role: t.role.clone(),
        description: t.description.clone(),
        system_prompt: t.system_prompt.clone(),
        preferred_model: t.preferred_model.clone(),
        fallback_models: t.fallback_models.clone(),
        tools: t.tools.clone(),
        permissions: t.permissions.clone(),
        memory_access: t.memory_access,
        budget: t.budget.clone(),
        max_turns: t.max_turns,
        worktree_policy: t.worktree_policy,
        quality_gates: t.quality_gates.clone(),
        voice: t.voice.clone(),
        avatar: t.avatar.clone(),
        visual_state: t.visual_state,
        version: t.version.clone(),
    }
}

/// Parses an export envelope (strict format/version check).
pub fn parse_export(json: &str) -> AgentResult<TemplateExport> {
    if json.len() > MAX_IMPORT_BYTES {
        return Err(invalid(format!("template file is larger than {MAX_IMPORT_BYTES} bytes")));
    }
    let doc: TemplateExport =
        serde_json::from_str(json).map_err(|e| invalid(format!("not a valid Orkestr agent template: {e}")))?;
    if doc.format != TEMPLATE_EXPORT_FORMAT {
        return Err(invalid(format!("unsupported format `{}` (expected `{TEMPLATE_EXPORT_FORMAT}`)", doc.format)));
    }
    if doc.schema_version == 0 || doc.schema_version > TEMPLATE_SCHEMA_VERSION {
        return Err(invalid(format!(
            "unsupported schemaVersion {} (this Orkestr reads up to {TEMPLATE_SCHEMA_VERSION})",
            doc.schema_version
        )));
    }
    Ok(doc)
}

/// The built-in templates (`id`, draft), validated.
pub fn builtin_templates() -> AgentResult<Vec<(AgentTemplateId, TemplateDraft)>> {
    BUILTIN_FILES
        .iter()
        .map(|(file, json)| {
            let doc = parse_export(json).map_err(|e| AgentError::Internal(format!("built-in {file}: {e}")))?;
            let id = doc
                .origin
                .as_ref()
                .map(|o| o.id)
                .ok_or_else(|| AgentError::Internal(format!("built-in {file} has no origin id")))?;
            Ok((id, validate_draft(doc.template).map_err(|e| AgentError::Internal(format!("built-in {file}: {e}")))?))
        })
        .collect()
}

fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        "agent".into()
    } else {
        s.chars().take(48).collect()
    }
}

/// Built-in templates are read-only.
#[must_use]
pub fn is_read_only(t: &AgentTemplate) -> bool {
    matches!(t.source, TemplateSource::Builtin | TemplateSource::Official)
}

/// Template persistence and operations (cheap to clone).
#[derive(Debug, Clone)]
pub struct TemplateService {
    db: Db,
}

impl TemplateService {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn put(&self, t: &AgentTemplate) -> AgentResult<()> {
        let doc = serde_json::to_string(t)?;
        let role = serde_json::to_string(&t.role)?;
        let source = serde_json::to_value(t.source)?.as_str().unwrap_or("Custom").to_owned();
        let t = t.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_templates (id, name, role, source, version, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, role = excluded.role, source = excluded.source,
                        version = excluded.version, updated_at = excluded.updated_at, doc = excluded.doc",
                    params![
                        t.id.to_string(),
                        t.name,
                        role,
                        source,
                        t.version,
                        t.created_at.to_rfc3339(),
                        t.updated_at.to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn find(&self, id: AgentTemplateId) -> AgentResult<Option<AgentTemplate>> {
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM agent_templates WHERE id = ?1", [id.to_string()], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        doc.map(|d| serde_json::from_str(&d).map_err(AgentError::from)).transpose()
    }

    /// Inserts missing built-ins and refreshes those whose version changed.
    /// Returns how many rows were written.
    pub async fn seed_builtins(&self) -> AgentResult<u32> {
        let mut written = 0;
        for (id, draft) in builtin_templates()? {
            match self.find(id).await? {
                Some(existing) if existing.version == draft.version && existing.source == TemplateSource::Builtin => {}
                Some(existing) => {
                    self.put(&template_from(id, draft, TemplateSource::Builtin, existing.created_at)).await?;
                    written += 1;
                }
                None => {
                    self.put(&template_from(id, draft, TemplateSource::Builtin, now())).await?;
                    written += 1;
                }
            }
        }
        if written > 0 {
            tracing::info!(written, "built-in agent templates seeded");
        }
        Ok(written)
    }

    /// Lists templates: built-ins first (seed order), then by name.
    pub async fn list(
        &self,
        role: Option<AgentRole>,
        source: Option<TemplateSource>,
    ) -> AgentResult<Vec<AgentTemplate>> {
        let docs: Vec<String> = self
            .db
            .call(|c| {
                let mut stmt = c.prepare("SELECT doc FROM agent_templates")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let order: Vec<AgentTemplateId> = builtin_templates()?.into_iter().map(|(id, _)| id).collect();
        let mut out: Vec<AgentTemplate> = docs
            .iter()
            .map(|d| serde_json::from_str(d).map_err(AgentError::from))
            .collect::<AgentResult<Vec<_>>>()?
            .into_iter()
            .filter(|t: &AgentTemplate| role.as_ref().is_none_or(|r| &t.role == r))
            .filter(|t| source.is_none_or(|s| t.source == s))
            .collect();
        out.sort_by(|a, b| {
            let ra = order.iter().position(|id| *id == a.id).unwrap_or(usize::MAX);
            let rb = order.iter().position(|id| *id == b.id).unwrap_or(usize::MAX);
            ra.cmp(&rb).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// One template.
    pub async fn get(&self, id: AgentTemplateId) -> AgentResult<AgentTemplate> {
        self.find(id).await?.ok_or_else(|| AgentError::NotFound(format!("template {id}")))
    }

    /// Creates a `Custom` template.
    pub async fn create(&self, draft: TemplateDraft) -> AgentResult<AgentTemplate> {
        let t = template_from(AgentTemplateId::new(), validate_draft(draft)?, TemplateSource::Custom, now());
        self.put(&t).await?;
        tracing::info!(template_id = %t.id, name = %t.name, "agent template created");
        Ok(t)
    }

    /// Replaces a non-built-in template's fields.
    pub async fn update(&self, id: AgentTemplateId, draft: TemplateDraft) -> AgentResult<AgentTemplate> {
        let existing = self.get(id).await?;
        if is_read_only(&existing) {
            return Err(AgentError::Forbidden(
                "built-in templates are read-only — duplicate the template to customise it".into(),
            ));
        }
        let t = template_from(id, validate_draft(draft)?, existing.source, existing.created_at);
        self.put(&t).await?;
        Ok(t)
    }

    /// Deletes a non-built-in template (running agents keep their snapshot).
    pub async fn delete(&self, id: AgentTemplateId) -> AgentResult<AgentTemplateId> {
        let existing = self.get(id).await?;
        if is_read_only(&existing) {
            return Err(AgentError::Forbidden("built-in templates cannot be deleted".into()));
        }
        self.db
            .call(move |c| {
                c.execute("DELETE FROM agent_templates WHERE id = ?1", [id.to_string()])?;
                Ok(())
            })
            .await?;
        Ok(id)
    }

    /// Exports a template as a pretty-printed envelope + suggested file name.
    pub async fn export(&self, id: AgentTemplateId) -> AgentResult<(String, String)> {
        let t = self.get(id).await?;
        let doc = TemplateExport {
            format: TEMPLATE_EXPORT_FORMAT.into(),
            schema_version: TEMPLATE_SCHEMA_VERSION,
            exported_at: Some(now()),
            origin: Some(TemplateOrigin { id: t.id, source: t.source }),
            template: draft_of(&t),
        };
        Ok((serde_json::to_string_pretty(&doc)?, format!("{}.agent.json", slug(&t.name))))
    }

    /// Imports an envelope as a new `Custom` template. Requested permission
    /// rules are kept only when `accept_permissions` (never silently, §48).
    pub async fn import(
        &self,
        json: &str,
        accept_permissions: bool,
    ) -> AgentResult<(AgentTemplate, Vec<PermissionRule>)> {
        let doc = parse_export(json)?;
        let mut draft = doc.template;
        let dropped = if accept_permissions { Vec::new() } else { std::mem::take(&mut draft.permissions) };
        let t = self.create(draft).await?;
        tracing::info!(template_id = %t.id, dropped = dropped.len(), "agent template imported");
        Ok((t, dropped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{Capability, PermissionEffect, WorktreePolicy};

    async fn svc() -> TemplateService {
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::schema::agent_migrations()).await.unwrap();
        TemplateService::new(db)
    }

    #[test]
    fn builtins_parse_validate_and_cover_every_role() {
        let all = builtin_templates().unwrap();
        assert_eq!(all.len(), 11);
        let roles: Vec<AgentRole> = all.iter().map(|(_, d)| d.role.clone()).collect();
        for r in [
            AgentRole::Architect,
            AgentRole::Backend,
            AgentRole::Frontend,
            AgentRole::Database,
            AgentRole::Qa,
            AgentRole::Security,
            AgentRole::DevOps,
            AgentRole::UiUx,
            AgentRole::ProductManager,
            AgentRole::Reviewer,
            AgentRole::Researcher,
        ] {
            assert!(roles.contains(&r), "missing built-in for {r:?}");
        }
        let ids: BTreeSet<_> = all.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids.len(), 11, "ids are unique");
        for (_, d) in &all {
            assert!(d.tools.contains(&"submit_result".to_owned()));
            assert!(d.system_prompt.len() > 300, "{} has a real prompt", d.name);
            assert!(d.max_turns >= 10);
            // Built-ins only narrow permissions (never Allow beyond the approval mode).
            assert!(d.permissions.iter().all(|p| p.effect != PermissionEffect::Allow), "{}", d.name);
            assert!(d
                .permissions
                .iter()
                .any(|p| p.capability == Capability::Network && p.effect == PermissionEffect::Deny));
            // Read-only roles never get write tools.
            if d.worktree_policy == WorktreePolicy::ReadOnlyMain {
                assert!(!d
                    .tools
                    .iter()
                    .any(|t| matches!(t.as_str(), "write_file" | "edit_file" | "run_command" | "git_commit")));
            }
        }
    }

    #[tokio::test]
    async fn seed_is_idempotent_and_builtins_are_read_only() {
        let s = svc().await;
        assert_eq!(s.seed_builtins().await.unwrap(), 11);
        assert_eq!(s.seed_builtins().await.unwrap(), 0);
        let list = s.list(None, None).await.unwrap();
        assert_eq!(list.len(), 11);
        assert_eq!(list[0].role, AgentRole::Architect, "seed order");
        let b = list.iter().find(|t| t.role == AgentRole::Backend).unwrap().clone();
        assert!(matches!(s.update(b.id, draft_of(&b)).await, Err(AgentError::Forbidden(_))));
        assert!(matches!(s.delete(b.id).await, Err(AgentError::Forbidden(_))));
        assert_eq!(s.list(Some(AgentRole::Qa), None).await.unwrap().len(), 1);
        assert_eq!(s.list(None, Some(TemplateSource::Custom)).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn crud_validation_and_import_export_roundtrip() {
        let s = svc().await;
        s.seed_builtins().await.unwrap();
        let backend = s.list(Some(AgentRole::Backend), None).await.unwrap().remove(0);
        // Duplicate = create from the built-in's draft.
        let mut d = draft_of(&backend);
        d.name = "  Payroll Backend  ".into();
        d.tools = vec!["read_file".into(), "read_file".into()];
        let t = s.create(d.clone()).await.unwrap();
        assert_eq!(t.name, "Payroll Backend");
        assert_eq!(t.tools, vec!["read_file".to_owned(), "submit_result".to_owned()], "dedup + submit_result");
        assert_eq!(t.source, TemplateSource::Custom);
        // Validation.
        let mut bad = d.clone();
        bad.tools = vec!["rm_everything".into()];
        assert!(matches!(s.create(bad).await, Err(AgentError::Invalid(m)) if m.contains("unknown tool")));
        let mut bad = d.clone();
        bad.max_turns = 0;
        assert!(s.create(bad).await.is_err());
        let mut bad = d.clone();
        bad.version = "v1".into();
        assert!(s.create(bad).await.is_err());
        let mut bad = d.clone();
        bad.name = String::new();
        assert!(s.create(bad).await.is_err());
        // Update.
        let mut upd = draft_of(&t);
        upd.description = "custom".into();
        upd.version = "1.1.0".into();
        let u = s.update(t.id, upd).await.unwrap();
        assert_eq!((u.description.as_str(), u.version.as_str(), u.created_at), ("custom", "1.1.0", t.created_at));
        // Export → import.
        let (json, file) = s.export(backend.id).await.unwrap();
        assert_eq!(file, "backend-engineer.agent.json");
        let (imported, dropped) = s.import(&json, false).await.unwrap();
        assert_ne!(imported.id, backend.id);
        assert_eq!(imported.source, TemplateSource::Custom);
        assert!(imported.permissions.is_empty(), "permissions are never imported silently");
        assert_eq!(dropped.len(), backend.permissions.len());
        assert_eq!(imported.system_prompt, backend.system_prompt);
        let (accepted, dropped) = s.import(&json, true).await.unwrap();
        assert_eq!(accepted.permissions, backend.permissions);
        assert!(dropped.is_empty());
        // Bad imports.
        assert!(s.import("{}", false).await.is_err());
        assert!(s.import(&json.replace("orkestr.agent-template", "other"), false).await.is_err());
        assert!(s.import(&json.replace("\"schemaVersion\": 1", "\"schemaVersion\": 99"), false).await.is_err());
        // Delete.
        assert_eq!(s.delete(t.id).await.unwrap(), t.id);
        assert!(matches!(s.get(t.id).await, Err(AgentError::NotFound(_))));
    }
}
