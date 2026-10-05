//! Context bundle types, the compatibility entry point [`build_bundle`] and
//! the prompt renderer.
//!
//! Allocation (deterministic, all estimates in tokens of `budget`; see
//! [`crate::optimizer`] for the full pipeline):
//!
//! 1. **Task** — always included; truncated to 25 % of the budget.
//! 2. **Handoff** — up to 20 %.
//! 3. **Rules** (project over global, pinned first) — up to 20 %, each note
//!    capped at 300 tokens.
//! 4. **Memory** (relevance × kind weight, mistakes / known issues first) —
//!    up to 15 %, same cap.
//! 5. **Files** — everything left, in score order (task relevance, import
//!    neighbours, symbol references, churn): small files whole, larger files
//!    as excerpts around matching / referenced symbols (merged windows).
//!
//! Whatever does not fit is listed in `omitted`. Every item carries a fixed
//! rendering overhead so `estimate_tokens(bundle.render())` stays within the
//! budget.

use orkestr_domain::{MemoryItem, MemoryKind};
use serde::{Deserialize, Serialize};

use crate::index::{FileIndex, FileReader};
use crate::optimizer::{optimize, MemoryInput, OptimizeRequest, Reason, RuleInput, Signals};
use crate::symbols::Language;

/// What to build context for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextRequest {
    /// Task / goal text.
    pub task: String,
    /// Token budget of the whole bundle.
    pub token_budget: u32,
    /// Effective rules, in priority order (e.g. from `memory.effectiveRules`).
    pub rules: Vec<MemoryItem>,
    /// Relevant memory items, in priority order (e.g. from `memory.search`).
    pub memory: Vec<MemoryItem>,
    /// Concise handoff text (§57), if any.
    pub handoff: Option<String>,
    /// Max files considered (default 12 when 0).
    pub max_files: usize,
}

/// A rule or memory note in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextNote {
    /// Source (`memory:<id>`).
    pub source: String,
    /// Kind.
    pub kind: MemoryKind,
    /// Title.
    pub title: String,
    /// Body (possibly truncated).
    pub body: String,
    /// Estimated tokens incl. overhead.
    pub tokens: u32,
    /// Why it was included.
    #[serde(default)]
    pub reasons: Vec<Reason>,
}

/// A line range of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    /// First line (1-based, inclusive).
    pub start_line: u32,
    /// Last line (inclusive).
    pub end_line: u32,
    /// Text of the lines.
    pub text: String,
}

/// A file in the bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    /// Project-relative path.
    pub path: String,
    /// Language.
    pub language: Language,
    /// Why it was included.
    pub reason: String,
    /// Relevance score.
    pub score: f64,
    /// Whole file included.
    pub whole_file: bool,
    /// Excerpts (one covering the file when `whole_file`).
    pub excerpts: Vec<Excerpt>,
    /// Estimated tokens incl. overhead.
    pub tokens: u32,
    /// Structured reasons (see `reason` for the summary).
    #[serde(default)]
    pub reasons: Vec<Reason>,
}

/// The assembled context.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBundle {
    /// Task text (possibly truncated).
    pub task: String,
    /// Rules.
    pub rules: Vec<ContextNote>,
    /// Memory.
    pub memory: Vec<ContextNote>,
    /// Files in rank order.
    pub files: Vec<ContextFile>,
    /// Handoff.
    pub handoff: Option<String>,
    /// Estimated total tokens.
    pub total_tokens: u32,
    /// Budget the bundle was packed for.
    pub budget: u32,
    /// What did not fit (`file:<path>`, `memory:<id>`, `rule:<id>`).
    pub omitted: Vec<String>,
}

impl From<&ContextRequest> for OptimizeRequest {
    fn from(req: &ContextRequest) -> Self {
        OptimizeRequest {
            task: req.task.clone(),
            token_budget: req.token_budget,
            rules: req.rules.iter().map(|i| RuleInput { item: i.clone(), key: i.key.clone() }).collect(),
            memory: req.memory.iter().map(|i| MemoryInput { item: i.clone(), relevance: 0.0 }).collect(),
            handoff: req.handoff.clone(),
            max_files: req.max_files,
            focus_paths: Vec::new(),
        }
    }
}

/// Builds a bundle for `req` from `index`, reading file contents via `reader`
/// (full optimizer with the import graph built from the index; no churn).
#[must_use]
pub fn build_bundle(index: &FileIndex, reader: &dyn FileReader, req: &ContextRequest) -> ContextBundle {
    optimize(index, reader, &OptimizeRequest::from(req), Signals::default()).bundle
}

impl ContextBundle {
    /// Renders the bundle as prompt text (markdown).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# Task\n");
        out.push_str(&self.task);
        out.push_str("\n\n");
        if let Some(h) = &self.handoff {
            out.push_str("# Handoff\n");
            out.push_str(h);
            out.push_str("\n\n");
        }
        if !self.rules.is_empty() {
            out.push_str("# Project rules\n");
            for n in &self.rules {
                out.push_str(&format!("- **{}**: {}\n", n.title, n.body));
            }
            out.push('\n');
        }
        if !self.memory.is_empty() {
            out.push_str("# Relevant memory\n");
            for n in &self.memory {
                out.push_str(&format!("- [{:?}] **{}**: {}\n", n.kind, n.title, n.body));
            }
            out.push('\n');
        }
        if !self.files.is_empty() {
            out.push_str("# Relevant files\n");
            for f in &self.files {
                for ex in &f.excerpts {
                    let range = if f.whole_file {
                        "whole file".to_owned()
                    } else {
                        format!("lines {}-{}", ex.start_line, ex.end_line)
                    };
                    out.push_str(&format!(
                        "## {} ({range}) — {}\n```{}\n{}\n```\n",
                        f.path,
                        f.reason,
                        f.language.fence(),
                        ex.text
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::MemReader;
    use crate::tokens::estimate_tokens;
    use orkestr_domain::{now, MemoryItemId, MemoryScope};

    fn mem(title: &str, body: &str, kind: MemoryKind) -> MemoryItem {
        MemoryItem {
            id: MemoryItemId::new(),
            scope: MemoryScope::Global,
            kind,
            title: title.into(),
            content: body.into(),
            tags: vec![],
            source: None,
            pinned: false,
            key: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn big_file() -> String {
        let mut s = String::new();
        for i in 0..300 {
            if i == 150 {
                s.push_str("pub fn compute_overtime(hours: u32) -> u32 {\n");
            } else {
                s.push_str(&format!("    let filler_{i} = {i}; // padding line number {i}\n"));
            }
        }
        s
    }

    fn sources() -> Vec<(String, String)> {
        vec![
            ("src/payroll/overtime.rs".into(), big_file()),
            ("src/payroll/mod.rs".into(), "pub mod overtime;\n".into()),
            ("docs/payroll.md".into(), "# Payroll\nOvertime is paid at 1.5x.\n".into()),
            ("src/ui/app.tsx".into(), "export function App() {}\n".into()),
        ]
    }

    fn setup() -> (FileIndex, MemReader) {
        let src = sources();
        let pairs: Vec<(&str, &str)> = src.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        (FileIndex::from_sources(pairs.clone()), MemReader::from_sources(pairs))
    }

    #[test]
    fn packs_within_budget_deterministically() {
        let (idx, reader) = setup();
        let req = ContextRequest {
            task: "Fix compute_overtime rounding in payroll".into(),
            token_budget: 1500,
            rules: vec![mem("Money in cents", "Never use floats for money.", MemoryKind::Convention)],
            memory: vec![mem("Overtime bug", "Rounding was off by one.", MemoryKind::KnownIssue)],
            handoff: Some("Previous agent added tests; 1 failing.".into()),
            max_files: 0,
        };
        let b = build_bundle(&idx, &reader, &req);
        assert!(b.total_tokens <= 1500, "{}", b.total_tokens);
        assert_eq!(b.rules.len(), 1);
        assert_eq!(b.memory.len(), 1);
        assert!(b.handoff.is_some());
        assert_eq!(b.files[0].path, "src/payroll/overtime.rs");
        let f = &b.files[0];
        assert!(!f.whole_file, "large file is excerpted");
        assert!(
            f.excerpts.iter().any(|e| e.start_line <= 151 && e.end_line >= 151),
            "{:?}",
            f.excerpts.iter().map(|e| (e.start_line, e.end_line)).collect::<Vec<_>>()
        );
        assert!(f.excerpts[0].text.contains("compute_overtime"));
        assert!(b.files.iter().any(|f| f.path == "docs/payroll.md" && f.whole_file));
        assert!(b.files.iter().all(|f| f.path != "src/ui/app.tsx"));

        let rendered = b.render();
        assert!(estimate_tokens(&rendered) <= 1500, "rendered {} tokens", estimate_tokens(&rendered));
        assert!(rendered.contains("# Task") && rendered.contains("# Project rules") && rendered.contains("```rust"));

        // Deterministic.
        assert_eq!(build_bundle(&idx, &reader, &req), b);
    }

    #[test]
    fn tiny_budget_still_respected() {
        let (idx, reader) = setup();
        for budget in [0u32, 10, 50, 120, 300] {
            let req = ContextRequest {
                task: "overtime payroll ".repeat(50),
                token_budget: budget,
                rules: vec![mem("R", &"rule text ".repeat(200), MemoryKind::Convention)],
                memory: vec![],
                handoff: Some("h".repeat(5000)),
                max_files: 3,
            };
            let b = build_bundle(&idx, &reader, &req);
            assert!(b.total_tokens <= budget, "budget {budget}: {}", b.total_tokens);
            assert!(
                estimate_tokens(&b.render()) <= budget.max(1) + 4,
                "budget {budget}: rendered {}",
                estimate_tokens(&b.render())
            );
        }
    }

    #[test]
    fn omitted_items_are_reported() {
        let (idx, _) = setup();
        let reader = MemReader::default(); // nothing readable
        let req = ContextRequest { task: "payroll overtime".into(), token_budget: 4000, ..ContextRequest::default() };
        let b = build_bundle(&idx, &reader, &req);
        assert!(b.files.is_empty());
        assert!(b.omitted.iter().any(|o| o == "file:src/payroll/overtime.rs"));
    }

    #[test]
    fn window_merging() {
        use crate::optimizer::windows;
        assert_eq!(windows(&[10, 12], 100), vec![(7, 24)]);
        assert_eq!(windows(&[0, 50], 100), vec![(0, 12), (47, 62)]);
        assert_eq!(windows(&[98], 100), vec![(95, 99)]);
    }
}
