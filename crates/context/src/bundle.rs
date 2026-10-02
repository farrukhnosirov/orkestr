//! Budget packer and prompt renderer.
//!
//! Allocation (deterministic, all estimates in tokens of `budget`):
//!
//! 1. **Task** — always included; truncated to 25 % of the budget.
//! 2. **Handoff** — up to 20 %.
//! 3. **Rules** (effective project/global rules, pinned first) — up to 20 %,
//!    each note capped at 300 tokens.
//! 4. **Memory** (relevant items, pinned first) — up to 15 %, same cap.
//! 5. **Files** — everything left, in rank order: small files whole, larger
//!    files as excerpts around matching symbols / lines (merged windows).
//!
//! Whatever does not fit is listed in `omitted`. Every item carries a fixed
//! rendering overhead so `estimate_tokens(bundle.render())` stays within the
//! budget.

use std::collections::BTreeSet;

use orkestr_domain::{MemoryItem, MemoryKind};
use serde::{Deserialize, Serialize};

use crate::index::{FileIndex, FileReader};
use crate::rank::{rank_files, terms};
use crate::symbols::Language;
use crate::tokens::{estimate_code_tokens, estimate_tokens, truncate_to_tokens};

/// Rendering overhead per section header.
const SECTION_OVERHEAD: u32 = 6;
/// Rendering overhead per note.
const NOTE_OVERHEAD: u32 = 8;
/// Rendering overhead per excerpt (header line + fences).
const EXCERPT_OVERHEAD: u32 = 24;
/// Max tokens of one rule / memory note.
const NOTE_CAP: u32 = 300;
/// Files up to this size are included whole.
const WHOLE_FILE_TOKENS: u32 = 600;
/// Lines of context around an anchor line.
const WINDOW_BEFORE: usize = 3;
const WINDOW_AFTER: usize = 12;
/// Max anchors per file.
const MAX_ANCHORS: usize = 6;

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

fn pct(budget: u32, p: u32) -> u32 {
    u32::try_from(u64::from(budget) * u64::from(p) / 100).unwrap_or(u32::MAX)
}

fn notes(
    items: &[MemoryItem],
    cap: u32,
    remaining: &mut u32,
    omitted: &mut Vec<String>,
    prefix: &str,
) -> Vec<ContextNote> {
    let mut out = Vec::new();
    let mut used = 0u32;
    for item in items {
        let body = truncate_to_tokens(item.content.trim(), NOTE_CAP);
        let tokens = estimate_tokens(&item.title) + estimate_tokens(&body) + NOTE_OVERHEAD;
        if used + tokens > cap || tokens > *remaining {
            omitted.push(format!("{prefix}:{}", item.id));
            continue;
        }
        used += tokens;
        *remaining -= tokens;
        out.push(ContextNote {
            source: format!("memory:{}", item.id),
            kind: item.kind,
            title: item.title.clone(),
            body,
            tokens,
        });
    }
    out
}

fn excerpt_tokens(lang: Language, text: &str) -> u32 {
    (if lang.is_code() { estimate_code_tokens(text) } else { estimate_tokens(text) }) + EXCERPT_OVERHEAD
}

/// Anchor lines (0-based): symbol definitions whose name shares a query
/// term, then lines containing a query term.
fn anchors(content_lines: &[&str], symbols: &[crate::symbols::Symbol], query: &BTreeSet<String>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for s in symbols {
        if terms(&s.name).iter().any(|t| query.contains(t)) {
            let line = (s.line as usize).saturating_sub(1);
            if !out.contains(&line) {
                out.push(line);
            }
        }
        if out.len() >= MAX_ANCHORS {
            return out;
        }
    }
    for (i, line) in content_lines.iter().enumerate() {
        if out.len() >= MAX_ANCHORS {
            break;
        }
        if !out.contains(&i) && terms(line).iter().any(|t| query.contains(t)) {
            out.push(i);
        }
    }
    out
}

/// Merged `[start, end]` 0-based inclusive windows around anchors.
fn windows(anchors: &[usize], line_count: usize) -> Vec<(usize, usize)> {
    let mut ws: Vec<(usize, usize)> = anchors
        .iter()
        .map(|&a| (a.saturating_sub(WINDOW_BEFORE), (a + WINDOW_AFTER).min(line_count.saturating_sub(1))))
        .collect();
    ws.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ws {
        match merged.last_mut() {
            Some(last) if s <= last.1 + 1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

fn make_excerpt(lines: &[&str], start: usize, end: usize) -> Excerpt {
    Excerpt {
        start_line: u32::try_from(start + 1).unwrap_or(u32::MAX),
        end_line: u32::try_from(end + 1).unwrap_or(u32::MAX),
        text: lines.get(start..=end).map(|l| l.join("\n")).unwrap_or_default(),
    }
}

/// Builds a bundle for `req` from `index`, reading file contents via `reader`.
#[must_use]
pub fn build_bundle(index: &FileIndex, reader: &dyn FileReader, req: &ContextRequest) -> ContextBundle {
    let budget = req.token_budget;
    let mut remaining = budget;
    let mut omitted = Vec::new();

    let task = truncate_to_tokens(req.task.trim(), pct(budget, 25).max(1));
    let task_tokens = (estimate_tokens(&task) + SECTION_OVERHEAD).min(remaining);
    remaining -= task_tokens;

    let handoff = req.handoff.as_deref().map(str::trim).filter(|h| !h.is_empty()).and_then(|h| {
        let cap = pct(budget, 20).min(remaining.saturating_sub(SECTION_OVERHEAD));
        if cap < 16 {
            return None;
        }
        let text = truncate_to_tokens(h, cap);
        remaining = remaining.saturating_sub(estimate_tokens(&text) + SECTION_OVERHEAD);
        Some(text)
    });

    let mut rules = Vec::new();
    if !req.rules.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
        rules = notes(&req.rules, pct(budget, 20), &mut remaining, &mut omitted, "rule");
    }
    let mut memory = Vec::new();
    if !req.memory.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
        memory = notes(&req.memory, pct(budget, 15), &mut remaining, &mut omitted, "memory");
    }

    let max_files = if req.max_files == 0 { 12 } else { req.max_files };
    let ranked = rank_files(index, &req.task, max_files);
    let query: BTreeSet<String> = terms(&req.task).into_iter().collect();
    let mut files = Vec::new();
    if !ranked.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
    }
    for rf in ranked {
        let Some(meta) = index.get(&rf.path) else { continue };
        let Some(content) = reader.read(&rf.path) else {
            omitted.push(format!("file:{}", rf.path));
            continue;
        };
        let reason = if rf.mentioned {
            "mentioned in task".to_owned()
        } else {
            format!("matches: {}", rf.matched_terms.join(", "))
        };
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let whole = excerpt_tokens(meta.language, &content);
        if whole <= WHOLE_FILE_TOKENS.max(EXCERPT_OVERHEAD + 1) && whole <= remaining {
            remaining -= whole;
            files.push(ContextFile {
                path: rf.path,
                language: meta.language,
                reason,
                score: rf.score,
                whole_file: true,
                excerpts: vec![make_excerpt(&lines, 0, lines.len() - 1)],
                tokens: whole,
            });
            continue;
        }
        let mut anchor_lines = anchors(&lines, &meta.symbols, &query);
        if anchor_lines.is_empty() {
            anchor_lines.push(0);
        }
        let mut excerpts = Vec::new();
        let mut used = 0u32;
        for (s, e) in windows(&anchor_lines, lines.len()) {
            let mut ex = make_excerpt(&lines, s, e);
            let mut cost = excerpt_tokens(meta.language, &ex.text);
            // Shrink the window from the end until it fits (keep ≥ 1 line).
            let mut end = e;
            while cost > remaining && end > s {
                end -= 1;
                ex = make_excerpt(&lines, s, end);
                cost = excerpt_tokens(meta.language, &ex.text);
            }
            if cost > remaining {
                break;
            }
            remaining -= cost;
            used += cost;
            excerpts.push(ex);
        }
        if excerpts.is_empty() {
            omitted.push(format!("file:{}", rf.path));
            continue;
        }
        files.push(ContextFile {
            path: rf.path,
            language: meta.language,
            reason,
            score: rf.score,
            whole_file: false,
            excerpts,
            tokens: used,
        });
    }

    ContextBundle {
        task,
        rules,
        memory,
        files,
        handoff,
        total_tokens: budget.saturating_sub(remaining),
        budget,
        omitted,
    }
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
        assert_eq!(windows(&[10, 12], 100), vec![(7, 24)]);
        assert_eq!(windows(&[0, 50], 100), vec![(0, 12), (47, 62)]);
        assert_eq!(windows(&[98], 100), vec![(95, 99)]);
    }
}
