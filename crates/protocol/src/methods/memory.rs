//! `memory.*`, `decisions.*`, `knowledge.*` — memory, decision log and
//! knowledge base (§20–21).
//!
//! Handlers live in `crates/core/src/rpc/memory.rs`; the engine is
//! `orkestr-memory`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    Actor, Decision, DecisionAlternative, DecisionId, DecisionStatus, KnowledgeKind, KnowledgeSource,
    KnowledgeSourceId, MemoryItem, MemoryItemId, MemoryKind, MemoryScope, ProjectId, TaskId,
};

use super::method;

/// Max title length (characters) of memory items and decisions.
pub const MAX_TITLE_LEN: usize = 200;
/// Max memory body size in bytes (larger content belongs in a knowledge source).
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Max number of tags per memory item.
pub const MAX_TAGS: usize = 32;
/// Max tag length (characters).
pub const MAX_TAG_LEN: usize = 40;
/// Max rule key length (characters).
pub const MAX_KEY_LEN: usize = 120;

// ---------------------------------------------------------------------------
// memory.*
// ---------------------------------------------------------------------------

/// Params of `memory.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListMemoryParams {
    /// Scope to list.
    pub scope: MemoryScope,
    /// Only these kinds (all when absent/empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kinds: Option<Vec<MemoryKind>>,
    /// Only items carrying this tag (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tag: Option<String>,
    /// Only pinned items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned_only: Option<bool>,
    /// Max items (default 500, max 2000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `memory.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryListResult {
    /// Items: pinned first, then most recently updated.
    pub items: Vec<MemoryItem>,
}

/// Params of `memory.get` / `memory.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryIdParams {
    /// Item id.
    pub id: MemoryItemId,
}

/// Result carrying one memory item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryItemResult {
    /// The item.
    pub item: MemoryItem,
}

/// Params of `memory.create` — emits `MemoryUpdated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateMemoryParams {
    /// Scope (Global or Project; Agent is accepted for private agent memory).
    pub scope: MemoryScope,
    /// Kind.
    pub kind: MemoryKind,
    /// Title (1..=200 chars).
    pub title: String,
    /// Markdown body (≤ 64 KiB).
    #[serde(default)]
    pub content: String,
    /// Tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
    /// Origin (`user`, `agent:<id>`, `task:<id>`, a file or URL). Defaults to `user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
    /// Pinned (always included in context).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned: Option<bool>,
    /// Rule key for project-over-global overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key: Option<String>,
}

/// Partial update of a memory item. For `source` and `key` an empty string clears.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryPatch {
    /// New kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<MemoryKind>,
    /// New title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// New body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content: Option<String>,
    /// New tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
    /// New source (`""` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
    /// Pin / unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned: Option<bool>,
    /// New rule key (`""` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key: Option<String>,
}

/// Params of `memory.update` — emits `MemoryUpdated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateMemoryParams {
    /// Item id.
    pub id: MemoryItemId,
    /// Changes.
    pub patch: MemoryPatch,
}

/// Result of `memory.delete` — emits `MemoryUpdated` with `deleted: true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteMemoryResult {
    /// Deleted id.
    pub id: MemoryItemId,
}

/// Params of `memory.search` (SQLite FTS5, bm25 ranking over title + body + tags).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchMemoryParams {
    /// Free-text query (words are prefix-matched; all words must match, with an
    /// any-word fallback when nothing matches all).
    pub query: String,
    /// Project whose memory is searched. Absent ⇒ global memory only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Also search global memory when `projectId` is set (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_global: Option<bool>,
    /// Only these kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kinds: Option<Vec<MemoryKind>>,
    /// Max hits (default 50, max 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemorySearchHit {
    /// The item.
    pub item: MemoryItem,
    /// Relevance (higher is better; derived from bm25).
    pub score: f64,
    /// Body excerpt; matches are wrapped in `\u0002` … `\u0003`.
    pub snippet: String,
}

/// Result of `memory.search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchMemoryResult {
    /// Hits, best first.
    pub hits: Vec<MemorySearchHit>,
}

/// Params of `memory.effectiveRules`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EffectiveRulesParams {
    /// Project (absent ⇒ only global rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Kinds treated as rules (default: Convention, Preference, QualityStandard,
    /// ProviderPreference, Infrastructure, Architecture, Requirement, Mistake).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kinds: Option<Vec<MemoryKind>>,
}

/// Where an effective rule comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RuleOrigin {
    /// Global memory.
    Global,
    /// Project memory.
    Project,
}

/// One resolved rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EffectiveRule {
    /// Resolution key (`key` or `title:<normalized title>`).
    pub key: String,
    /// The winning item.
    pub item: MemoryItem,
    /// Origin of the winning item.
    pub origin: RuleOrigin,
    /// The global item this project item overrides, if any.
    pub overrides: Option<MemoryItemId>,
}

/// Result of `memory.effectiveRules`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EffectiveRulesResult {
    /// Rules: pinned first, then project before global, then by title.
    pub rules: Vec<EffectiveRule>,
}

// ---------------------------------------------------------------------------
// decisions.*
// ---------------------------------------------------------------------------

/// Params of `decisions.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListDecisionsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Only this status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<DecisionStatus>,
    /// Max entries (default 200, max 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `decisions.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionListResult {
    /// Decisions, newest first.
    pub decisions: Vec<Decision>,
}

/// Params of `decisions.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionIdParams {
    /// Decision id.
    pub id: DecisionId,
}

/// Result carrying one decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionResult {
    /// The decision.
    pub decision: Decision,
}

/// Params of `decisions.create` — emits `DecisionRecorded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateDecisionParams {
    /// Project.
    pub project_id: ProjectId,
    /// Title.
    pub title: String,
    /// The decision.
    pub decision: String,
    /// Context / problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    /// Alternatives considered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub alternatives: Option<Vec<DecisionAlternative>>,
    /// Reasoning summary (never hidden chain-of-thought, §21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reasoning_summary: Option<String>,
    /// Consequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub consequences: Option<String>,
    /// Status (default Accepted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<DecisionStatus>,
    /// Related tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_ids: Option<Vec<TaskId>>,
    /// Related files (project-relative paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub files: Option<Vec<String>>,
    /// Who decided (default: the system on behalf of the local user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decided_by: Option<Actor>,
}

/// Partial update of a decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionPatch {
    /// New title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// New decision text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decision: Option<String>,
    /// New context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    /// New alternatives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub alternatives: Option<Vec<DecisionAlternative>>,
    /// New reasoning summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reasoning_summary: Option<String>,
    /// New consequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub consequences: Option<String>,
    /// New status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<DecisionStatus>,
    /// New related tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_ids: Option<Vec<TaskId>>,
    /// New related files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub files: Option<Vec<String>>,
    /// Superseding decision (same project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub superseded_by: Option<DecisionId>,
}

/// Params of `decisions.update` — emits `DecisionRecorded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateDecisionParams {
    /// Decision id.
    pub id: DecisionId,
    /// Changes.
    pub patch: DecisionPatch,
}

// ---------------------------------------------------------------------------
// knowledge.*
// ---------------------------------------------------------------------------

/// Params of `knowledge.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListKnowledgeParams {
    /// Project (absent ⇒ global sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// With `projectId`: also list global sources (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_global: Option<bool>,
}

/// Result of `knowledge.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeListResult {
    /// Sources, newest first.
    pub sources: Vec<KnowledgeSource>,
}

/// Params of `knowledge.add`. The location is ingested immediately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddKnowledgeParams {
    /// Project (absent ⇒ global knowledge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Absolute path of a file or folder (markdown, text, code docs).
    pub location: String,
    /// Display name (default: file/folder name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Kind (default inferred from the location).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<KnowledgeKind>,
}

/// Params of `knowledge.remove` / `knowledge.reindex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeIdParams {
    /// Source id.
    pub id: KnowledgeSourceId,
}

/// Result carrying one knowledge source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeSourceResult {
    /// The source (check `status`/`error`: unsupported formats are recorded as `Failed`).
    pub source: KnowledgeSource,
}

/// Result of `knowledge.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveKnowledgeResult {
    /// Removed id.
    pub id: KnowledgeSourceId,
}

/// Params of `knowledge.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchKnowledgeParams {
    /// Free-text query.
    pub query: String,
    /// Project (absent ⇒ global only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Also search global sources when `projectId` is set (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_global: Option<bool>,
    /// Max hits (default 30, max 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// One knowledge search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeHit {
    /// Source.
    pub source_id: KnowledgeSourceId,
    /// Source name.
    pub source_name: String,
    /// File path relative to the source folder (the file name for a single-file source).
    pub path: String,
    /// Heading path, e.g. `Install › Windows`.
    pub heading_path: String,
    /// Chunk ordinal within the file.
    pub ordinal: u32,
    /// Excerpt; matches are wrapped in `\u0002` … `\u0003`.
    pub snippet: String,
    /// Relevance (higher is better).
    pub score: f64,
}

/// Result of `knowledge.search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchKnowledgeResult {
    /// Hits, best first.
    pub hits: Vec<KnowledgeHit>,
}

// ---------------------------------------------------------------------------
// Markers
// ---------------------------------------------------------------------------

method!(
    /// `memory.list`.
    List, "memory.list", ListMemoryParams, MemoryListResult
);
method!(
    /// `memory.get`.
    Get, "memory.get", MemoryIdParams, MemoryItemResult
);
method!(
    /// `memory.create` — emits `MemoryUpdated`.
    Create, "memory.create", CreateMemoryParams, MemoryItemResult
);
method!(
    /// `memory.update` — emits `MemoryUpdated`.
    Update, "memory.update", UpdateMemoryParams, MemoryItemResult
);
method!(
    /// `memory.delete` — emits `MemoryUpdated { deleted: true }`.
    Delete, "memory.delete", MemoryIdParams, DeleteMemoryResult
);
method!(
    /// `memory.search` — FTS5 bm25.
    Search, "memory.search", SearchMemoryParams, SearchMemoryResult
);
method!(
    /// `memory.effectiveRules` — project rules override global rules by key.
    EffectiveRules, "memory.effectiveRules", EffectiveRulesParams, EffectiveRulesResult
);
method!(
    /// `decisions.list`.
    DecisionsList, "decisions.list", ListDecisionsParams, DecisionListResult
);
method!(
    /// `decisions.get`.
    DecisionsGet, "decisions.get", DecisionIdParams, DecisionResult
);
method!(
    /// `decisions.create` — emits `DecisionRecorded`.
    DecisionsCreate, "decisions.create", CreateDecisionParams, DecisionResult
);
method!(
    /// `decisions.update` — emits `DecisionRecorded`.
    DecisionsUpdate, "decisions.update", UpdateDecisionParams, DecisionResult
);
method!(
    /// `knowledge.list`.
    KnowledgeList, "knowledge.list", ListKnowledgeParams, KnowledgeListResult
);
method!(
    /// `knowledge.add` — registers and ingests a file/folder.
    KnowledgeAdd, "knowledge.add", AddKnowledgeParams, KnowledgeSourceResult
);
method!(
    /// `knowledge.remove` — removes the source and its chunks (never files on disk).
    KnowledgeRemove, "knowledge.remove", KnowledgeIdParams, RemoveKnowledgeResult
);
method!(
    /// `knowledge.reindex` — re-reads the location and rebuilds its chunks.
    KnowledgeReindex, "knowledge.reindex", KnowledgeIdParams, KnowledgeSourceResult
);
method!(
    /// `knowledge.search` — FTS5 bm25 over chunks.
    KnowledgeSearch, "knowledge.search", SearchKnowledgeParams, SearchKnowledgeResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_params_minimal_json() {
        let p: CreateMemoryParams =
            serde_json::from_str(r#"{"scope":{"scope":"Global"},"kind":"Convention","title":"Use tabs"}"#).unwrap();
        assert_eq!(p.title, "Use tabs");
        assert!(p.content.is_empty());
        assert_eq!(p.scope, MemoryScope::Global);
    }

    #[test]
    fn project_scope_json_shape() {
        let id = ProjectId::new();
        let v = serde_json::to_value(MemoryScope::Project { project_id: id }).unwrap();
        assert_eq!(v["scope"], "Project");
        assert_eq!(v["projectId"], id.to_string());
    }
}
