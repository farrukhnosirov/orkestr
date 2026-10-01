//! Memory, knowledge base and decision log (§20, §21).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{Actor, Timestamp};
use crate::ids::{AgentId, DecisionId, KnowledgeSourceId, MemoryItemId, ProjectId, TaskId};

/// Scope a stored memory item belongs to. Project rules take precedence over
/// global ones when both apply (§20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum MemoryScope {
    /// Global memory (user preferences, conventions).
    Global,
    /// Project memory.
    Project {
        /// Owning project.
        project_id: ProjectId,
    },
    /// Private memory of one agent.
    Agent {
        /// Owning agent.
        agent_id: AgentId,
    },
}

/// What memory an agent template may read (resolved against the agent's project at spawn).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MemoryAccess {
    /// No memory.
    None,
    /// Own project memory only.
    ProjectOnly,
    /// Own project + global memory (default).
    #[default]
    ProjectAndGlobal,
    /// Project + global + the agent's private memory.
    Full,
}

/// Kind of memory item (§20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MemoryKind {
    /// Requirement.
    Requirement,
    /// Architecture fact.
    Architecture,
    /// Business logic.
    BusinessLogic,
    /// Coding convention.
    Convention,
    /// DB/schema fact.
    Schema,
    /// API contract.
    Api,
    /// Decision summary (full entry in the decision log).
    Decision,
    /// Mistake to avoid.
    Mistake,
    /// TODO.
    Todo,
    /// Known issue.
    KnownIssue,
    /// Agent run summary.
    AgentSummary,
    /// Preference (global memory).
    Preference,
    /// Free note.
    Note,
}

/// A memory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryItem {
    /// Id.
    pub id: MemoryItemId,
    /// Scope.
    pub scope: MemoryScope,
    /// Kind.
    pub kind: MemoryKind,
    /// Title.
    pub title: String,
    /// Content (markdown).
    pub content: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Where it came from (task id, file, url, user).
    pub source: Option<String>,
    /// Pinned items are always included in context.
    pub pinned: bool,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
}

/// Kind of knowledge source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KnowledgeKind {
    /// Markdown files.
    Markdown,
    /// PDF.
    Pdf,
    /// Generic documentation folder.
    Docs,
    /// API documentation.
    ApiDocs,
    /// Company rules.
    CompanyRules,
    /// Schemas.
    Schema,
    /// External URL.
    Url,
    /// Other.
    Other,
}

/// Indexing state of a knowledge source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum IndexStatus {
    /// Not indexed yet.
    Pending,
    /// Indexing.
    Indexing,
    /// Indexed.
    Ready,
    /// Failed.
    Failed,
}

/// A knowledge base source (§20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeSource {
    /// Id.
    pub id: KnowledgeSourceId,
    /// Project, or global when `None`.
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: KnowledgeKind,
    /// Path or URL.
    pub location: String,
    /// Snapshot of the content in the object store.
    pub object_id: Option<String>,
    /// Index status.
    pub status: IndexStatus,
    /// Last indexing time.
    pub indexed_at: Option<Timestamp>,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Status of a decision record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DecisionStatus {
    /// Proposed.
    Proposed,
    /// Accepted.
    Accepted,
    /// Rejected.
    Rejected,
    /// Superseded by another decision.
    Superseded,
}

/// An alternative considered (§8 consensus, §21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionAlternative {
    /// Title.
    pub title: String,
    /// Summary.
    pub summary: String,
    /// Who proposed it (model/agent name).
    pub proposed_by: Option<String>,
    /// Why it was not chosen.
    pub rejected_reason: Option<String>,
}

/// ADR-like decision log entry (§21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Decision {
    /// Id.
    pub id: DecisionId,
    /// Project.
    pub project_id: ProjectId,
    /// Title.
    pub title: String,
    /// Context / problem.
    pub context: String,
    /// The decision.
    pub decision: String,
    /// Alternatives.
    pub alternatives: Vec<DecisionAlternative>,
    /// Reasoning summary (never hidden chain-of-thought).
    pub reasoning_summary: String,
    /// Consequences.
    pub consequences: String,
    /// Status.
    pub status: DecisionStatus,
    /// Related tasks.
    pub task_ids: Vec<TaskId>,
    /// Related files.
    pub files: Vec<String>,
    /// Who decided.
    pub decided_by: Actor,
    /// Superseding decision.
    pub superseded_by: Option<DecisionId>,
    /// Creation time.
    pub created_at: Timestamp,
}
