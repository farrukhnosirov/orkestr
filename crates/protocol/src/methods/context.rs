//! `context.*` — Context Optimizer preview and explanation (§20, §68.40).
//!
//! Handlers live in `crates/core/src/rpc/context.rs`; the engine is
//! `orkestr-context`. A preview shows exactly what an agent working on the
//! task would receive: every item with its reasons and token count, within a
//! model-aware budget.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{MemoryKind, ModelRef, TaskId};

use super::method;

/// Why an item is in (or was considered for) the context. Same JSON shape as
/// `orkestr_context::Reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ContextReason {
    /// The task text names this file.
    Mentioned,
    /// The task already touched this file.
    Focus,
    /// Task terms match the path / symbols / content.
    TermMatch {
        /// Matched terms.
        terms: Vec<String>,
    },
    /// Defines a symbol the task names.
    Defines {
        /// Symbol.
        symbol: String,
    },
    /// A relevant file imports this one.
    ImportedBy {
        /// The relevant file.
        file: String,
    },
    /// This file imports a relevant file.
    Imports {
        /// The relevant file.
        file: String,
    },
    /// Uses a symbol defined in a relevant file.
    References {
        /// Symbol.
        symbol: String,
        /// Defining file.
        file: String,
    },
    /// Changed recently / often.
    RecentChange {
        /// Commits in the history window.
        commits: u32,
    },
    /// Project or global rule.
    Rule {
        /// `project` or `global`.
        origin: String,
        /// Overrides a global rule.
        overrides_global: bool,
    },
    /// Pinned.
    Pinned,
    /// Relevant memory.
    Memory {
        /// Relevance in percent.
        relevance_pct: u32,
        /// Tags shared with the task.
        tags: Vec<String>,
    },
    /// Mistake to avoid.
    Mistake,
    /// Known issue.
    KnownIssue,
}

/// Kind of a context item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ContextItemKind {
    /// The task text.
    Task,
    /// Handoff from previous agents.
    Handoff,
    /// Rule.
    Rule,
    /// Memory item.
    Memory,
    /// File (whole or excerpts).
    File,
}

/// What the optimizer did with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ContextDecision {
    /// Whole file.
    Whole,
    /// Excerpts.
    Excerpt,
    /// Included.
    Included,
    /// Did not fit the budget.
    OmittedBudget,
    /// Unreadable.
    OmittedUnreadable,
    /// Below the file cutoff.
    BelowCutoff,
    /// Duplicate (overridden rule / repeated memory).
    Duplicate,
}

/// A line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextLineRange {
    /// First line (1-based).
    pub start_line: u32,
    /// Last line (inclusive).
    pub end_line: u32,
}

/// One item of a context preview / explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextItem {
    /// Kind.
    pub kind: ContextItemKind,
    /// Path (files) or `rule:<id>` / `memory:<id>`; `task` / `handoff`.
    pub source: String,
    /// Display title (file path, memory title).
    pub title: String,
    /// Memory kind for rules / memory.
    pub memory_kind: Option<MemoryKind>,
    /// Estimated tokens (0 when omitted).
    pub tokens: u32,
    /// Decision.
    pub decision: ContextDecision,
    /// Reasons.
    pub reasons: Vec<ContextReason>,
    /// Ranking score (files, memory).
    pub score: Option<f64>,
    /// BM25 + mention part of the score (files).
    pub base_score: Option<f64>,
    /// Neighbour / reference part (files).
    pub graph_score: Option<f64>,
    /// Churn multiplier (files).
    pub churn_boost: Option<f64>,
    /// Included line ranges (files).
    pub ranges: Vec<ContextLineRange>,
}

/// Budget used for a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextBudgetInfo {
    /// Model the budget was computed for.
    pub model: Option<ModelRef>,
    /// Its context window (absent when unknown).
    pub context_window: Option<u32>,
    /// Output tokens reserved.
    pub reserved_output: u32,
    /// Estimated system prompt + tools.
    pub system_tokens: u32,
    /// Configured cap.
    pub cap: u32,
    /// Bundle budget.
    pub budget: u32,
    /// Tokens used.
    pub used: u32,
}

/// Index / signal facts behind a preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextIndexInfo {
    /// Indexed files.
    pub files: u32,
    /// Import edges.
    pub import_edges: u32,
    /// Commits in the churn window (0 without git).
    pub churn_commits: u32,
    /// Index hit the file limit.
    pub truncated: bool,
    /// The project has a root folder.
    pub has_root: bool,
}

/// Params of `context.preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextPreviewParams {
    /// Task.
    pub task_id: TaskId,
    /// Model to budget for (default: the task's model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Bundle cap override (default: the agent runtime's budget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub token_budget: Option<u32>,
    /// Max files (default 8, max 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_files: Option<u32>,
}

/// Result of `context.preview`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextPreviewResult {
    /// Task.
    pub task_id: TaskId,
    /// Included items in prompt order (task, handoff, rules, memory, files).
    pub items: Vec<ContextItem>,
    /// Number of considered items left out.
    pub omitted: u32,
    /// Budget.
    pub budget: ContextBudgetInfo,
    /// Index facts.
    pub index: ContextIndexInfo,
    /// How the handoff history was compressed (`None`, `Deterministic`, `Model`), when there is one.
    pub handoff_compression: Option<String>,
    /// The rendered prompt section.
    pub rendered: String,
}

/// Params of `context.explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextExplainParams {
    /// Task.
    pub task_id: TaskId,
    /// Only this file / source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
    /// Model to budget for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Bundle cap override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub token_budget: Option<u32>,
    /// Max files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_files: Option<u32>,
}

/// Result of `context.explain`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextExplainResult {
    /// Task.
    pub task_id: TaskId,
    /// Every considered item (included and rejected) with its scoring breakdown.
    pub items: Vec<ContextItem>,
    /// Budget.
    pub budget: ContextBudgetInfo,
}

method!(
    /// `context.preview` — what an agent on this task would receive.
    Preview, "context.preview", ContextPreviewParams, ContextPreviewResult
);
method!(
    /// `context.explain` — every candidate with scores, reasons and decisions.
    Explain, "context.explain", ContextExplainParams, ContextExplainResult
);
