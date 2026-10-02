//! `templates.*` — agent templates (§6, agent-runtime-spec §2.1).
//!
//! Templates are persisted by `orkestr-agent-runtime`. Built-in (official)
//! templates are seeded on start and are read-only: duplicate them to edit.
//! Import/export uses a versioned JSON envelope ([`TemplateExport`]) — the
//! foundation of the Agent Marketplace (§6). Imported templates never carry
//! permission grants silently: requested permissions are dropped unless the
//! caller explicitly accepts them (`acceptPermissions: true`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentRole, AgentState, AgentTemplate, AgentTemplateId, AvatarConfig, Budget, MemoryAccess, ModelRef,
    PermissionRule, QualityGate, TemplateSource, Timestamp, VoiceProfile, WorktreePolicy,
};

use super::method;

/// `format` value of exported templates.
pub const TEMPLATE_EXPORT_FORMAT: &str = "orkestr.agent-template";
/// Current export schema version.
pub const TEMPLATE_SCHEMA_VERSION: u32 = 1;

/// Every user-editable field of a template (§6). Used by create, update,
/// import and export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TemplateDraft {
    /// Name (1..=80 chars).
    pub name: String,
    /// Role.
    pub role: AgentRole,
    /// Short description (≤ 500 chars).
    #[serde(default)]
    pub description: String,
    /// System prompt (1..=32 000 chars). Variables: `{{project.name}}`,
    /// `{{task.title}}`, `{{agent.role}}`.
    pub system_prompt: String,
    /// Preferred model (absent ⇒ routed automatically).
    #[serde(default)]
    pub preferred_model: Option<ModelRef>,
    /// Fallback chain, in order.
    #[serde(default)]
    pub fallback_models: Vec<ModelRef>,
    /// Allow-listed tool names (must be known runtime tools).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Requested permission rules; they become Agent-scope rules at spawn.
    #[serde(default)]
    pub permissions: Vec<PermissionRule>,
    /// Memory visibility.
    #[serde(default)]
    pub memory_access: MemoryAccess,
    /// Budget per run.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Max tool-loop turns per run (1..=500).
    pub max_turns: u32,
    /// Worktree policy.
    #[serde(default)]
    pub worktree_policy: WorktreePolicy,
    /// Quality gates.
    #[serde(default)]
    pub quality_gates: Vec<QualityGate>,
    /// Voice (§33).
    #[serde(default)]
    pub voice: Option<VoiceProfile>,
    /// Robot / avatar (§30).
    #[serde(default)]
    pub avatar: Option<AvatarConfig>,
    /// Initial robot visual state.
    #[serde(default)]
    pub visual_state: AgentState,
    /// Template version (semver `MAJOR.MINOR.PATCH`).
    pub version: String,
}

/// Origin metadata of an exported template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TemplateOrigin {
    /// Id of the template in the exporting installation.
    pub id: AgentTemplateId,
    /// Source in the exporting installation.
    pub source: TemplateSource,
}

/// Versioned export envelope (`templates.export` / `templates.import`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TemplateExport {
    /// Always `orkestr.agent-template`.
    pub format: String,
    /// Envelope schema version (currently 1).
    pub schema_version: u32,
    /// Export time.
    #[serde(default)]
    pub exported_at: Option<Timestamp>,
    /// Origin of the exported template.
    #[serde(default)]
    pub origin: Option<TemplateOrigin>,
    /// The template.
    pub template: TemplateDraft,
}

/// Params of `templates.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTemplatesParams {
    /// Only this role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub role: Option<AgentRole>,
    /// Only this source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<TemplateSource>,
}

/// Result of `templates.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTemplatesResult {
    /// Templates: built-ins first (by role), then the rest by name.
    pub templates: Vec<AgentTemplate>,
}

/// Params addressing one template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TemplateIdParams {
    /// Template.
    pub template_id: AgentTemplateId,
}

/// Result carrying one template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SingleTemplateResult {
    /// The template.
    pub template: AgentTemplate,
}

/// Params of `templates.create` (source becomes `Custom`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateTemplateParams {
    /// Fields.
    pub template: TemplateDraft,
}

/// Params of `templates.update` (built-ins are read-only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateTemplateParams {
    /// Template.
    pub template_id: AgentTemplateId,
    /// New fields (full replacement).
    pub template: TemplateDraft,
}

/// Result of `templates.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteTemplateResult {
    /// Deleted template.
    pub template_id: AgentTemplateId,
}

/// Result of `templates.export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportTemplateResult {
    /// Pretty-printed [`TemplateExport`] JSON.
    pub json: String,
    /// Suggested file name (`backend-engineer.agent.json`).
    pub file_name: String,
}

/// Params of `templates.import`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportTemplateParams {
    /// [`TemplateExport`] JSON (≤ 256 KiB).
    pub json: String,
    /// Keep the template's requested permission rules. Default false: they are
    /// dropped and reported in `droppedPermissions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub accept_permissions: Option<bool>,
}

/// Result of `templates.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportTemplateResult {
    /// The new template (fresh id, source `Custom`).
    pub template: AgentTemplate,
    /// Permission rules the file requested but that were not accepted.
    pub dropped_permissions: Vec<PermissionRule>,
}

method!(
    /// `templates.list`.
    List, "templates.list", ListTemplatesParams, ListTemplatesResult
);
method!(
    /// `templates.get`.
    Get, "templates.get", TemplateIdParams, SingleTemplateResult
);
method!(
    /// `templates.create`.
    Create, "templates.create", CreateTemplateParams, SingleTemplateResult
);
method!(
    /// `templates.update`.
    Update, "templates.update", UpdateTemplateParams, SingleTemplateResult
);
method!(
    /// `templates.delete`.
    Delete, "templates.delete", TemplateIdParams, DeleteTemplateResult
);
method!(
    /// `templates.export`.
    Export, "templates.export", TemplateIdParams, ExportTemplateResult
);
method!(
    /// `templates.import`.
    Import, "templates.import", ImportTemplateParams, ImportTemplateResult
);
