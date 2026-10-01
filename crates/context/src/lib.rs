//! # orkestr-context — **SKELETON** (Phase 2 fills it in)
//!
//! Context optimizer (MASTER_SPEC §20): never send the whole repo. Assemble
//! the task, relevant files and symbols, dependencies, project rules, relevant
//! memory and a concise handoff within a token budget.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{ProjectId, TaskId};
use serde::{Deserialize, Serialize};

/// What to build context for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRequest {
    /// Project.
    pub project_id: ProjectId,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Natural-language goal.
    pub goal: String,
    /// Token budget for the assembled context.
    pub token_budget: u32,
}

/// One piece of context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    /// Source (`file:src/a.rs#L10-40`, `memory:<id>`, `rule:…`).
    pub source: String,
    /// Content.
    pub content: String,
    /// Relevance 0..1.
    pub relevance: f32,
    /// Estimated tokens.
    pub tokens: u32,
}

/// Assembled context.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBundle {
    /// Items in priority order.
    pub items: Vec<ContextItem>,
    /// Total tokens.
    pub total_tokens: u32,
}

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Context optimizer.
#[async_trait]
pub trait ContextOptimizer: Send + Sync {
    /// Builds a bundle within the budget.
    async fn build(&self, request: &ContextRequest) -> Result<ContextBundle, ContextError>;
}

/// Cheap token estimate (≈ 4 chars per token) used until a tokenizer per
/// model family is wired in.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.chars().count().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Greedily keeps the most relevant items that fit in `budget`.
#[must_use]
pub fn pack(mut items: Vec<ContextItem>, budget: u32) -> ContextBundle {
    items.sort_by(|a, b| b.relevance.total_cmp(&a.relevance));
    let mut bundle = ContextBundle::default();
    for item in items {
        if bundle.total_tokens.saturating_add(item.tokens) <= budget {
            bundle.total_tokens += item.tokens;
            bundle.items.push(item);
        }
    }
    bundle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(r: f32, tokens: u32) -> ContextItem {
        ContextItem { source: format!("s{r}"), content: String::new(), relevance: r, tokens }
    }

    #[test]
    fn packing_respects_budget_and_relevance() {
        let b = pack(vec![item(0.1, 50), item(0.9, 60), item(0.5, 50)], 110);
        assert_eq!(b.total_tokens, 110);
        assert_eq!(b.items[0].relevance, 0.9);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
