//! Handoff compression (§20, §57): long visible histories (agent messages,
//! previous handoffs, Manager chat) are compressed into a concise handoff
//! text before they enter a prompt.
//!
//! * With a [`Summarizer`] (a cheap model, when configured) the *middle* of
//!   the history is summarized; the first entry (the goal) and the most recent
//!   entries are kept verbatim.
//! * Without one — or when it fails, times out at the caller, or returns an
//!   over-budget answer — [`compress_deterministic`] keeps the goal, the most
//!   recent entries and one-line headlines of the middle.
//!
//! Only visible text is ever passed in: hidden reasoning is never stored or
//! summarized (§1.14).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::tokens::{estimate_tokens, truncate_to_tokens};

/// One visible history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Who wrote it (`user`, `assistant`, `system`, `handoff`, agent name).
    pub role: String,
    /// Visible text.
    pub text: String,
}

impl HistoryEntry {
    /// New entry.
    #[must_use]
    pub fn new(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self { role: role.into(), text: text.into() }
    }

    fn render(&self) -> String {
        format!("[{}] {}", self.role, self.text.trim())
    }
}

/// How the history was compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompressionMethod {
    /// Fits as is.
    None,
    /// Deterministic truncation.
    Deterministic,
    /// Summarized by a model.
    Model,
}

/// Result of compression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressedHistory {
    /// The handoff text.
    pub text: String,
    /// Method used.
    pub method: CompressionMethod,
    /// Entries in the input.
    pub original_entries: u32,
    /// Estimated tokens of the uncompressed history.
    pub original_tokens: u32,
    /// Estimated tokens of `text`.
    pub tokens: u32,
}

/// A (cheap) model that summarizes text.
#[async_trait]
pub trait Summarizer: Send + Sync {
    /// Summarizes `text` in at most ≈ `max_tokens` tokens. Errors are
    /// human-readable and fall back to deterministic compression.
    async fn summarize(&self, text: &str, max_tokens: u32) -> Result<String, String>;
}

/// Instruction given to summarizer models (also used by the mock provider to
/// recognise summarization requests).
pub const SUMMARIZE_INSTRUCTION: &str = "Summarize this work history into a concise handoff for the next agent: \
goal, what was done, decisions, current state, open issues. Facts only, no speculation, no hidden reasoning.";

fn render_all(entries: &[HistoryEntry]) -> String {
    entries.iter().map(HistoryEntry::render).collect::<Vec<_>>().join("\n")
}

fn headline(e: &HistoryEntry) -> String {
    let first = e.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let clipped: String = first.chars().take(100).collect();
    let ell = if first.chars().count() > 100 { "…" } else { "" };
    format!("- [{}] {clipped}{ell}", e.role)
}

/// Splits into (goal, middle, recent) where `recent` fits `recent_budget`.
fn split(entries: &[HistoryEntry], recent_budget: u32) -> (Option<&HistoryEntry>, &[HistoryEntry], &[HistoryEntry]) {
    let Some((first, rest)) = entries.split_first() else { return (None, &[], &[]) };
    let mut used = 0u32;
    let mut start = rest.len();
    while start > 0 {
        let t = estimate_tokens(&rest[start - 1].render()) + 1;
        if used + t > recent_budget {
            break;
        }
        used += t;
        start -= 1;
    }
    (Some(first), &rest[..start], &rest[start..])
}

fn finish(
    text: String,
    method: CompressionMethod,
    entries: &[HistoryEntry],
    original_tokens: u32,
    max: u32,
) -> CompressedHistory {
    let text = truncate_to_tokens(&text, max);
    CompressedHistory {
        tokens: estimate_tokens(&text),
        text,
        method,
        original_entries: u32::try_from(entries.len()).unwrap_or(u32::MAX),
        original_tokens,
    }
}

fn assemble(goal: Option<&HistoryEntry>, middle: &str, recent: &[HistoryEntry], max_tokens: u32) -> String {
    let mut out = String::new();
    if let Some(g) = goal {
        out.push_str("Goal: ");
        out.push_str(&truncate_to_tokens(&g.render(), (max_tokens / 5).max(8)));
        out.push('\n');
    }
    if !middle.trim().is_empty() {
        out.push_str("Earlier:\n");
        out.push_str(middle.trim_end());
        out.push('\n');
    }
    if !recent.is_empty() {
        out.push_str("Recent:\n");
        out.push_str(&render_all(recent));
    }
    out
}

/// Deterministic compression to `max_tokens`.
#[must_use]
pub fn compress_deterministic(entries: &[HistoryEntry], max_tokens: u32) -> CompressedHistory {
    let full = render_all(entries);
    let original_tokens = estimate_tokens(&full);
    if original_tokens <= max_tokens {
        return finish(full, CompressionMethod::None, entries, original_tokens, max_tokens);
    }
    let (goal, middle, recent) = split(entries, max_tokens / 2);
    // Headlines of the middle, newest kept when space runs out.
    let goal_tokens = goal.map_or(0, |g| estimate_tokens(&g.render()).min((max_tokens / 5).max(8)) + 4);
    let recent_tokens = estimate_tokens(&render_all(recent)) + 4;
    let mut budget = max_tokens.saturating_sub(goal_tokens + recent_tokens + 8);
    let mut lines: Vec<String> = Vec::new();
    for e in middle.iter().rev() {
        let h = headline(e);
        let t = estimate_tokens(&h) + 1;
        if t > budget {
            break;
        }
        budget -= t;
        lines.push(h);
    }
    let skipped = middle.len() - lines.len();
    lines.reverse();
    let mut mid = String::new();
    if skipped > 0 {
        mid.push_str(&format!("- … {skipped} earlier entries omitted\n"));
    }
    mid.push_str(&lines.join("\n"));
    finish(
        assemble(goal, &mid, recent, max_tokens),
        CompressionMethod::Deterministic,
        entries,
        original_tokens,
        max_tokens,
    )
}

/// Compresses with `summarizer` when given, else deterministically.
pub async fn compress_history(
    entries: &[HistoryEntry],
    max_tokens: u32,
    summarizer: Option<&dyn Summarizer>,
) -> CompressedHistory {
    let full = render_all(entries);
    let original_tokens = estimate_tokens(&full);
    if original_tokens <= max_tokens {
        return finish(full, CompressionMethod::None, entries, original_tokens, max_tokens);
    }
    let Some(summarizer) = summarizer else { return compress_deterministic(entries, max_tokens) };
    let (goal, middle, recent) = split(entries, max_tokens / 3);
    if middle.is_empty() {
        return compress_deterministic(entries, max_tokens);
    }
    let goal_tokens = goal.map_or(0, |g| estimate_tokens(&g.render()).min((max_tokens / 5).max(8)) + 4);
    let recent_tokens = estimate_tokens(&render_all(recent)) + 4;
    let summary_budget = max_tokens.saturating_sub(goal_tokens + recent_tokens + 8);
    if summary_budget < 32 {
        return compress_deterministic(entries, max_tokens);
    }
    match summarizer.summarize(&render_all(middle), summary_budget).await {
        Ok(s) if !s.trim().is_empty() && estimate_tokens(s.trim()) <= summary_budget + summary_budget / 10 => {
            let mid = truncate_to_tokens(s.trim(), summary_budget);
            finish(
                assemble(goal, &mid, recent, max_tokens),
                CompressionMethod::Model,
                entries,
                original_tokens,
                max_tokens,
            )
        }
        Ok(_) => {
            tracing::debug!("summarizer answer empty or over budget; deterministic handoff compression");
            compress_deterministic(entries, max_tokens)
        }
        Err(e) => {
            tracing::warn!(error = %e, "summarizer failed; deterministic handoff compression");
            compress_deterministic(entries, max_tokens)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(n: usize) -> Vec<HistoryEntry> {
        let mut v = vec![HistoryEntry::new("user", "Goal: implement overtime calculation with tests")];
        for i in 0..n {
            v.push(HistoryEntry::new(
                "assistant",
                format!(
                    "Step {i}: edited src/payroll.rs and ran cargo test; {} more detail",
                    "lorem ipsum ".repeat(20)
                ),
            ));
        }
        v
    }

    struct Fixed(Result<String, String>);
    #[async_trait]
    impl Summarizer for Fixed {
        async fn summarize(&self, _text: &str, _max: u32) -> Result<String, String> {
            self.0.clone()
        }
    }

    #[test]
    fn short_history_is_kept() {
        let h = history(1);
        let c = compress_deterministic(&h, 10_000);
        assert_eq!(c.method, CompressionMethod::None);
        assert!(c.text.contains("Step 0"));
    }

    #[test]
    fn deterministic_fits_and_keeps_goal_and_recent() {
        let h = history(60);
        let c = compress_deterministic(&h, 600);
        assert_eq!(c.method, CompressionMethod::Deterministic);
        assert!(c.tokens <= 600, "{}", c.tokens);
        assert!(c.text.starts_with("Goal: [user] Goal: implement overtime"));
        assert!(c.text.contains("Step 59"), "most recent kept");
        assert!(c.text.contains("earlier entries omitted"));
        assert!(c.original_tokens > 600);
        assert_eq!(c.original_entries, 61);
        assert_eq!(compress_deterministic(&h, 600), c, "deterministic");
        for budget in [0u32, 5, 40, 100] {
            assert!(compress_deterministic(&h, budget).tokens <= budget.max(1), "budget {budget}");
        }
    }

    #[tokio::test]
    async fn model_summary_used_and_fallbacks() {
        let h = history(60);
        let ok = Fixed(Ok("Did steps 0-57: payroll edits, tests green except rounding.".into()));
        let c = compress_history(&h, 800, Some(&ok)).await;
        assert_eq!(c.method, CompressionMethod::Model);
        assert!(c.text.contains("Did steps 0-57"));
        assert!(c.text.contains("Step 59"));
        assert!(c.tokens <= 800);

        let err = Fixed(Err("provider down".into()));
        assert_eq!(compress_history(&h, 800, Some(&err)).await.method, CompressionMethod::Deterministic);
        let huge = Fixed(Ok("x ".repeat(10_000)));
        assert_eq!(compress_history(&h, 800, Some(&huge)).await.method, CompressionMethod::Deterministic);
        assert_eq!(compress_history(&h, 800, None).await.method, CompressionMethod::Deterministic);
        assert_eq!(compress_history(&history(0), 800, Some(&ok)).await.method, CompressionMethod::None);
    }
}
