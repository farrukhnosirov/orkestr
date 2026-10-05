//! The full Context Optimizer (§20, §68.40).
//!
//! Pipeline, all deterministic (ties break on path / id):
//!
//! 1. **Seeds** — BM25 ranking ([`rank_files`]) of the task text plus explicit
//!    path mentions and *focus* paths (files the task already touched).
//! 2. **Neighbours** — for the strongest seeds, files they import and files
//!    importing them ([`ImportGraph`]), and files that *use* symbols the seed
//!    defines and the task talks about (symbol reference search over the
//!    per-file identifier index).
//! 3. **Recency / churn** — a git-history multiplier ([`ChurnStats::boost`])
//!    on already relevant files.
//! 4. **Rules** — project rules take precedence over global ones with the same
//!    key / title; pinned first, then project before global.
//! 5. **Memory** — FTS relevance × kind weight (mistakes and known issues
//!    weigh most) + tag matches with the task, deduplicated against rules.
//! 6. **Packing** — model-aware token budget ([`ModelBudget`]); small files
//!    whole, larger files as excerpts (relevant ranges ± context lines around
//!    matched / referenced symbols).
//!
//! Every included or rejected item carries [`Reason`]s and a [`Decision`], so
//! the UI can explain *why* something is in the prompt.

use std::collections::{BTreeMap, BTreeSet};

use orkestr_domain::{MemoryItem, MemoryKind, MemoryScope};
use serde::{Deserialize, Serialize};

use crate::bundle::{ContextBundle, ContextFile, ContextNote, Excerpt};
use crate::imports::ImportGraph;
use crate::index::{FileIndex, FileReader, IndexedFile};
use crate::rank::{rank_files, terms, MENTION_BOOST};
use crate::signals::ChurnStats;
use crate::symbols::Language;
use crate::tokens::{estimate_code_tokens, estimate_tokens, truncate_to_tokens};

/// Rendering overhead per section header.
pub const SECTION_OVERHEAD: u32 = 6;
/// Rendering overhead per note.
pub const NOTE_OVERHEAD: u32 = 8;
/// Rendering overhead per excerpt (header line + fences).
pub const EXCERPT_OVERHEAD: u32 = 24;
/// Max tokens of one rule / memory note.
pub const NOTE_CAP: u32 = 300;
/// Files up to this size are included whole.
pub const WHOLE_FILE_TOKENS: u32 = 600;
/// Lines of context before an anchor line.
pub const WINDOW_BEFORE: usize = 3;
/// Lines of context after an anchor line.
pub const WINDOW_AFTER: usize = 12;
/// Max anchors per file.
pub const MAX_ANCHORS: usize = 6;
/// Seeds whose neighbours are expanded.
pub const MAX_SEEDS: usize = 6;
/// Max neighbours added per seed and relation.
pub const MAX_NEIGHBOURS: usize = 4;
/// Max symbols per seed used for reference search.
pub const MAX_REF_SYMBOLS: usize = 3;
/// Max referencing files per symbol.
pub const MAX_REFS: usize = 4;
/// Max graph (neighbour + reference) score, as a fraction of the best BM25 score.
pub const GRAPH_CAP: f64 = 0.6;
/// Rejected candidates kept in the explanation.
pub const MAX_EXPLAINED_REJECTS: usize = 40;

/// Why an item is in (or was considered for) the context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Reason {
    /// The task text names this file.
    Mentioned,
    /// The task already touched this file (focus path).
    Focus,
    /// Task terms match the path / symbols / content.
    TermMatch {
        /// Matched terms (sorted).
        terms: Vec<String>,
    },
    /// Defines a symbol the task names.
    Defines {
        /// Symbol name.
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
    /// This file uses a symbol defined in a relevant file.
    References {
        /// Symbol name.
        symbol: String,
        /// File defining it.
        file: String,
    },
    /// Changed recently / often in git history.
    RecentChange {
        /// Commits in the history window.
        commits: u32,
    },
    /// A project or global rule.
    Rule {
        /// `project` or `global`.
        origin: String,
        /// Overrides a global rule with the same key.
        overrides_global: bool,
    },
    /// Pinned memory / rule.
    Pinned,
    /// Memory relevant to the task.
    Memory {
        /// Search relevance in percent (0..=100).
        relevance_pct: u32,
        /// Tags shared with the task.
        tags: Vec<String>,
    },
    /// A recorded mistake to avoid.
    Mistake,
    /// A known issue.
    KnownIssue,
}

impl Reason {
    /// Short English summary (logs, the rendered prompt).
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Mentioned => "mentioned in task".into(),
            Self::Focus => "already touched by this task".into(),
            Self::TermMatch { terms } => format!("matches: {}", terms.join(", ")),
            Self::Defines { symbol } => format!("defines `{symbol}`"),
            Self::ImportedBy { file } => format!("imported by {file}"),
            Self::Imports { file } => format!("imports {file}"),
            Self::References { symbol, file } => format!("uses `{symbol}` from {file}"),
            Self::RecentChange { commits } => format!("recently changed ({commits} commits)"),
            Self::Rule { origin, overrides_global } => {
                if *overrides_global {
                    format!("{origin} rule (overrides global)")
                } else {
                    format!("{origin} rule")
                }
            }
            Self::Pinned => "pinned".into(),
            Self::Memory { relevance_pct, tags } => {
                if tags.is_empty() {
                    format!("relevant memory ({relevance_pct}%)")
                } else {
                    format!("relevant memory ({relevance_pct}%; tags {})", tags.join(", "))
                }
            }
            Self::Mistake => "mistake to avoid".into(),
            Self::KnownIssue => "known issue".into(),
        }
    }
}

/// What the packer did with a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Decision {
    /// Included whole.
    Whole,
    /// Included as excerpts.
    Excerpt,
    /// Included (notes).
    Included,
    /// Relevant but did not fit the budget.
    OmittedBudget,
    /// Could not be read.
    OmittedUnreadable,
    /// Ranked below the file cutoff.
    BelowCutoff,
    /// Duplicate of another item (same rule key / memory title).
    Duplicate,
}

/// One considered file with its scoring breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidate {
    /// Path.
    pub path: String,
    /// Final score.
    pub score: f64,
    /// BM25 + mention part.
    pub base_score: f64,
    /// Neighbour / reference part.
    pub graph_score: f64,
    /// Churn multiplier applied.
    pub churn_boost: f64,
    /// Reasons.
    pub reasons: Vec<Reason>,
    /// Decision.
    pub decision: Decision,
    /// Tokens used (0 when omitted).
    pub tokens: u32,
}

/// One considered rule / memory note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteCandidate {
    /// `rule:<id>` / `memory:<id>`.
    pub source: String,
    /// Title.
    pub title: String,
    /// Kind.
    pub kind: MemoryKind,
    /// Ranking score.
    pub score: f64,
    /// Reasons.
    pub reasons: Vec<Reason>,
    /// Decision.
    pub decision: Decision,
    /// Tokens used.
    pub tokens: u32,
}

/// The bundle plus the explanation of every decision.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPlan {
    /// The packed context.
    pub bundle: ContextBundle,
    /// Files considered (included first, in bundle order; then rejected by score).
    pub files: Vec<FileCandidate>,
    /// Rules and memory considered.
    pub notes: Vec<NoteCandidate>,
}

/// Model-aware budget: the context window minus reserved output and the
/// system prompt, with a safety margin for estimation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBudget {
    /// Model context window (tokens).
    pub context_window: u32,
    /// Output tokens to reserve (0 ⇒ min(window / 4, 8192)).
    pub reserve_output: u32,
    /// Estimated system prompt + tool schema tokens.
    pub system_tokens: u32,
    /// Configured cap of the context bundle (0 ⇒ none).
    pub cap: u32,
}

impl ModelBudget {
    /// Safety margin applied to the free window.
    pub const SAFETY: f64 = 0.85;

    /// Output reservation actually used.
    #[must_use]
    pub fn reserved_output(&self) -> u32 {
        if self.reserve_output > 0 {
            self.reserve_output
        } else {
            (self.context_window / 4).min(8192)
        }
    }

    /// Token budget of the context bundle.
    #[must_use]
    pub fn effective(&self) -> u32 {
        let free = self.context_window.saturating_sub(self.reserved_output()).saturating_sub(self.system_tokens);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let safe = (f64::from(free) * Self::SAFETY).floor() as u32;
        if self.cap == 0 {
            safe
        } else {
            safe.min(self.cap)
        }
    }
}

/// A memory hit with its search relevance.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryInput {
    /// Item.
    pub item: MemoryItem,
    /// Search relevance in 0..1 (0 when unknown).
    pub relevance: f64,
}

/// A rule candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleInput {
    /// Item (origin derives from its scope).
    pub item: MemoryItem,
    /// Resolution key (`key` or normalized title when absent).
    pub key: Option<String>,
}

/// Optimizer input.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OptimizeRequest {
    /// Task / goal text.
    pub task: String,
    /// Token budget of the bundle (see [`ModelBudget::effective`]).
    pub token_budget: u32,
    /// Rule candidates (project and global; precedence is applied here).
    pub rules: Vec<RuleInput>,
    /// Memory candidates.
    pub memory: Vec<MemoryInput>,
    /// Concise handoff text (§57).
    pub handoff: Option<String>,
    /// Max files (default 12 when 0).
    pub max_files: usize,
    /// Files the task already touched (treated as strong seeds).
    pub focus_paths: Vec<String>,
}

/// Optional repository signals.
#[derive(Debug, Clone, Copy, Default)]
pub struct Signals<'a> {
    /// Import graph (built from the index when absent).
    pub graph: Option<&'a ImportGraph>,
    /// Git churn.
    pub churn: Option<&'a ChurnStats>,
}

fn pct(budget: u32, p: u32) -> u32 {
    u32::try_from(u64::from(budget) * u64::from(p) / 100).unwrap_or(u32::MAX)
}

fn norm_title(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn is_project(item: &MemoryItem) -> bool {
    !matches!(item.scope, MemoryScope::Global)
}

fn kind_weight(kind: MemoryKind) -> f64 {
    match kind {
        MemoryKind::Mistake | MemoryKind::KnownIssue => 1.5,
        MemoryKind::Decision | MemoryKind::Architecture | MemoryKind::Api | MemoryKind::Schema => 1.2,
        MemoryKind::BusinessLogic | MemoryKind::Requirement | MemoryKind::Convention => 1.1,
        MemoryKind::AgentSummary => 0.9,
        MemoryKind::Note | MemoryKind::Todo => 0.8,
        _ => 1.0,
    }
}

struct PackedNote {
    note: ContextNote,
    cand: NoteCandidate,
}

fn pack_notes(
    items: Vec<(MemoryItem, f64, Vec<Reason>, &'static str)>,
    cap: u32,
    remaining: &mut u32,
    omitted: &mut Vec<String>,
) -> Vec<PackedNote> {
    let mut out = Vec::new();
    let mut used = 0u32;
    for (item, score, reasons, prefix) in items {
        let body = truncate_to_tokens(item.content.trim(), NOTE_CAP);
        let tokens = estimate_tokens(&item.title) + estimate_tokens(&body) + NOTE_OVERHEAD;
        let source = format!("{prefix}:{}", item.id);
        let fits = used + tokens <= cap && tokens <= *remaining;
        let cand = NoteCandidate {
            source: source.clone(),
            title: item.title.clone(),
            kind: item.kind,
            score,
            reasons: reasons.clone(),
            decision: if fits { Decision::Included } else { Decision::OmittedBudget },
            tokens: if fits { tokens } else { 0 },
        };
        if !fits {
            omitted.push(source);
            out.push(PackedNote {
                note: ContextNote {
                    source: String::new(),
                    kind: item.kind,
                    title: String::new(),
                    body: String::new(),
                    tokens: 0,
                    reasons: Vec::new(),
                },
                cand,
            });
            continue;
        }
        used += tokens;
        *remaining -= tokens;
        out.push(PackedNote {
            note: ContextNote { source, kind: item.kind, title: item.title.clone(), body, tokens, reasons },
            cand,
        });
    }
    out
}

/// A ranked note: item, score, reasons.
type RankedNote = (MemoryItem, f64, Vec<Reason>);

/// Applies rule precedence: one rule per key, project over global; order
/// pinned → project → global → input order.
fn resolve_rules(rules: &[RuleInput]) -> (Vec<RankedNote>, Vec<NoteCandidate>) {
    let key_of = |r: &RuleInput| r.key.clone().unwrap_or_else(|| format!("title:{}", norm_title(&r.item.title)));
    let mut by_key: BTreeMap<String, (usize, &RuleInput)> = BTreeMap::new();
    let mut dups = Vec::new();
    let mut overridden: BTreeSet<String> = BTreeSet::new();
    for (i, r) in rules.iter().enumerate() {
        let k = key_of(r);
        match by_key.get(&k) {
            None => {
                by_key.insert(k, (i, r));
            }
            Some((_, existing)) => {
                let (keep_new, loser) = match (is_project(&existing.item), is_project(&r.item)) {
                    (false, true) => (true, *existing),
                    _ => (false, r),
                };
                if is_project(&r.item) != is_project(&existing.item) {
                    overridden.insert(k.clone());
                }
                dups.push(NoteCandidate {
                    source: format!("rule:{}", loser.item.id),
                    title: loser.item.title.clone(),
                    kind: loser.item.kind,
                    score: 0.0,
                    reasons: vec![Reason::Rule {
                        origin: if is_project(&loser.item) { "project".into() } else { "global".into() },
                        overrides_global: false,
                    }],
                    decision: Decision::Duplicate,
                    tokens: 0,
                });
                if keep_new {
                    by_key.insert(k, (i, r));
                }
            }
        }
    }
    let mut kept: Vec<(String, usize, &RuleInput)> = by_key.into_iter().map(|(k, (i, r))| (k, i, r)).collect();
    kept.sort_by(|a, b| {
        b.2.item
            .pinned
            .cmp(&a.2.item.pinned)
            .then_with(|| is_project(&b.2.item).cmp(&is_project(&a.2.item)))
            .then_with(|| a.1.cmp(&b.1))
    });
    let out = kept
        .into_iter()
        .map(|(k, _, r)| {
            let mut reasons = vec![Reason::Rule {
                origin: if is_project(&r.item) { "project".into() } else { "global".into() },
                overrides_global: is_project(&r.item) && overridden.contains(&k),
            }];
            if r.item.pinned {
                reasons.push(Reason::Pinned);
            }
            match r.item.kind {
                MemoryKind::Mistake => reasons.push(Reason::Mistake),
                MemoryKind::KnownIssue => reasons.push(Reason::KnownIssue),
                _ => {}
            }
            (r.item.clone(), if r.item.pinned { 2.0 } else { 1.0 }, reasons)
        })
        .collect();
    (out, dups)
}

/// Ranks memory: relevance × kind weight + tag matches (+ pinned), deduped
/// against rules and by title.
fn rank_memory(
    memory: &[MemoryInput],
    query: &BTreeSet<String>,
    exclude_ids: &BTreeSet<String>,
) -> (Vec<RankedNote>, Vec<NoteCandidate>) {
    let mut seen_ids: BTreeSet<String> = exclude_ids.clone();
    let mut seen_titles: BTreeSet<String> = BTreeSet::new();
    let mut ranked = Vec::new();
    let mut dups = Vec::new();
    let n = memory.len().max(1);
    for (i, m) in memory.iter().enumerate() {
        let id = m.item.id.to_string();
        let title = norm_title(&m.item.title);
        if !seen_ids.insert(id) || !seen_titles.insert(title) {
            dups.push(NoteCandidate {
                source: format!("memory:{}", m.item.id),
                title: m.item.title.clone(),
                kind: m.item.kind,
                score: 0.0,
                reasons: Vec::new(),
                decision: Decision::Duplicate,
                tokens: 0,
            });
            continue;
        }
        // Unknown relevance: keep the caller's order.
        #[allow(clippy::cast_precision_loss)]
        let relevance = if m.relevance > 0.0 { m.relevance.min(1.0) } else { 1.0 - (i as f64 / n as f64) * 0.5 };
        let tags: Vec<String> = m
            .item
            .tags
            .iter()
            .filter(|t| terms(t).iter().any(|x| query.contains(x)))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let mut score = relevance * kind_weight(m.item.kind) + (0.25 * tags.len() as f64).min(0.5);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mut reasons = vec![Reason::Memory { relevance_pct: (relevance * 100.0).round() as u32, tags }];
        match m.item.kind {
            MemoryKind::Mistake => reasons.push(Reason::Mistake),
            MemoryKind::KnownIssue => reasons.push(Reason::KnownIssue),
            _ => {}
        }
        if m.item.pinned {
            score += 1.0;
            reasons.push(Reason::Pinned);
        }
        ranked.push((m.item.clone(), score, reasons, i));
    }
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.3.cmp(&b.3)));
    (ranked.into_iter().map(|(it, s, r, _)| (it, s, r)).collect(), dups)
}

#[derive(Debug, Clone, Default)]
struct Cand {
    base: f64,
    graph: f64,
    churn: f64,
    reasons: Vec<Reason>,
    /// Extra anchor names (referenced symbols).
    anchors: BTreeSet<String>,
}

impl Cand {
    fn score(&self) -> f64 {
        (self.base + self.graph) * self.churn.max(1.0)
    }
    fn add_reason(&mut self, r: Reason) {
        if !self.reasons.contains(&r) {
            self.reasons.push(r);
        }
    }
}

/// Scores files: seeds, focus paths, neighbours, references, churn.
fn score_files(
    index: &FileIndex,
    req: &OptimizeRequest,
    graph: &ImportGraph,
    churn: Option<&ChurnStats>,
    query: &BTreeSet<String>,
) -> Vec<(String, Cand)> {
    let max_files = if req.max_files == 0 { 12 } else { req.max_files };
    let ranked = rank_files(index, &req.task, (max_files * 4).clamp(16, 200));
    let mut cands: BTreeMap<String, Cand> = BTreeMap::new();
    let top_bm25 = ranked.iter().filter(|r| !r.mentioned).map(|r| r.score).fold(0.0_f64, f64::max).max(1.0);
    for r in &ranked {
        let c = cands.entry(r.path.clone()).or_default();
        c.base = r.score;
        if r.mentioned {
            c.add_reason(Reason::Mentioned);
        }
        if !r.matched_terms.is_empty() {
            c.add_reason(Reason::TermMatch { terms: r.matched_terms.clone() });
        }
        // The file *defining* a symbol the task names outranks its users.
        if let Some(meta) = index.get(&r.path) {
            let defined = meta
                .symbols
                .iter()
                .find(|s| s.name.len() >= 4 && query.contains(&s.name.to_lowercase()))
                .map(|s| s.name.clone());
            if let Some(symbol) = defined {
                c.base += top_bm25;
                c.add_reason(Reason::Defines { symbol });
            }
        }
    }
    for p in &req.focus_paths {
        let p = p.replace('\\', "/");
        if index.get(&p).is_some() {
            let c = cands.entry(p).or_default();
            c.base = c.base.max(MENTION_BOOST / 2.0);
            c.add_reason(Reason::Focus);
        }
    }

    // Seeds: strongest candidates so far (deterministic order).
    let mut seeds: Vec<(String, f64)> = cands.iter().map(|(p, c)| (p.clone(), c.base)).collect();
    seeds.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    seeds.truncate(MAX_SEEDS.min(max_files));
    for (seed, seed_score) in &seeds {
        let strength = seed_score.min(top_bm25);
        for (rel, weight, list) in
            [("imports", 0.5, graph.imports_of(seed)), ("importedBy", 0.35, graph.importers_of(seed))]
        {
            for n in list.into_iter().take(MAX_NEIGHBOURS) {
                let c = cands.entry(n.to_owned()).or_default();
                c.graph += strength * weight;
                if rel == "imports" {
                    c.add_reason(Reason::ImportedBy { file: seed.clone() });
                } else {
                    c.add_reason(Reason::Imports { file: seed.clone() });
                }
            }
        }
        // Symbol reference search: symbols the seed defines that the task names.
        let Some(meta) = index.get(seed) else { continue };
        let mut syms: Vec<&str> = meta
            .symbols
            .iter()
            .filter(|s| s.name.len() >= 4 && terms(&s.name).iter().any(|t| query.contains(t)))
            .map(|s| s.name.as_str())
            .collect();
        syms.dedup();
        for sym in syms.into_iter().take(MAX_REF_SYMBOLS) {
            if let Some(c) = cands.get_mut(seed.as_str()) {
                c.anchors.insert(sym.to_owned());
            }
            let users: Vec<&IndexedFile> =
                index.files.iter().filter(|f| f.path != *seed && f.uses_ident(sym)).take(MAX_REFS).collect();
            for u in users {
                let c = cands.entry(u.path.clone()).or_default();
                c.graph += strength * 0.3;
                c.anchors.insert(sym.to_owned());
                c.add_reason(Reason::References { symbol: sym.to_owned(), file: seed.clone() });
            }
        }
    }
    // Neighbourhood can lift a file, but never above the files that define the work.
    let graph_cap = top_bm25 * GRAPH_CAP;
    for c in cands.values_mut() {
        c.graph = c.graph.min(graph_cap);
    }
    if let Some(churn) = churn {
        for (p, c) in &mut cands {
            let b = churn.boost(p);
            c.churn = b;
            if b > 1.05 {
                let commits = churn.files.get(p).map_or(0, |f| f.commits);
                c.add_reason(Reason::RecentChange { commits });
            }
        }
    }
    let mut out: Vec<(String, Cand)> = cands.into_iter().filter(|(_, c)| c.score() > 0.0).collect();
    out.sort_by(|a, b| b.1.score().total_cmp(&a.1.score()).then_with(|| a.0.cmp(&b.0)));
    out
}

fn excerpt_tokens(lang: Language, text: &str) -> u32 {
    (if lang.is_code() { estimate_code_tokens(text) } else { estimate_tokens(text) }) + EXCERPT_OVERHEAD
}

/// Anchor lines (0-based): definitions of / lines using the extra anchor
/// symbols, then symbol definitions sharing a query term, then lines
/// containing a query term.
fn anchors(
    lines: &[&str],
    symbols: &[crate::symbols::Symbol],
    query: &BTreeSet<String>,
    extra: &BTreeSet<String>,
) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let push = |l: usize, out: &mut Vec<usize>| {
        if out.len() < MAX_ANCHORS && !out.contains(&l) {
            out.push(l);
        }
    };
    for s in symbols {
        if extra.contains(&s.name) {
            push((s.line as usize).saturating_sub(1), &mut out);
        }
    }
    if !extra.is_empty() {
        for (i, line) in lines.iter().enumerate() {
            if out.len() >= MAX_ANCHORS {
                break;
            }
            if extra.iter().any(|e| line.contains(e.as_str())) {
                push(i, &mut out);
            }
        }
    }
    for s in symbols {
        if terms(&s.name).iter().any(|t| query.contains(t)) {
            push((s.line as usize).saturating_sub(1), &mut out);
        }
    }
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= MAX_ANCHORS {
            break;
        }
        if terms(line).iter().any(|t| query.contains(t)) {
            push(i, &mut out);
        }
    }
    out
}

/// Merged `[start, end]` 0-based inclusive windows around anchors.
#[must_use]
pub fn windows(anchors: &[usize], line_count: usize) -> Vec<(usize, usize)> {
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

fn reason_text(reasons: &[Reason]) -> String {
    let parts: Vec<String> = reasons.iter().take(3).map(Reason::summary).collect();
    if parts.is_empty() {
        "relevant".into()
    } else {
        parts.join("; ")
    }
}

/// Runs the optimizer.
#[must_use]
pub fn optimize(
    index: &FileIndex,
    reader: &dyn FileReader,
    req: &OptimizeRequest,
    signals: Signals<'_>,
) -> ContextPlan {
    let budget = req.token_budget;
    let mut remaining = budget;
    let mut omitted = Vec::new();
    let query: BTreeSet<String> = terms(&req.task).into_iter().collect();

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

    let mut note_cands: Vec<NoteCandidate> = Vec::new();
    let (rules_ranked, rule_dups) = resolve_rules(&req.rules);
    let rule_ids: BTreeSet<String> = rules_ranked.iter().map(|r| r.0.id.to_string()).collect();
    let mut rules = Vec::new();
    if !rules_ranked.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
        let items = rules_ranked.into_iter().map(|(i, s, r)| (i, s, r, "rule")).collect();
        for p in pack_notes(items, pct(budget, 20), &mut remaining, &mut omitted) {
            if p.cand.decision == Decision::Included {
                rules.push(p.note);
            }
            note_cands.push(p.cand);
        }
    }
    note_cands.extend(rule_dups);
    let (mem_ranked, mem_dups) = rank_memory(&req.memory, &query, &rule_ids);
    let mut memory = Vec::new();
    if !mem_ranked.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
        let items = mem_ranked.into_iter().map(|(i, s, r)| (i, s, r, "memory")).collect();
        for p in pack_notes(items, pct(budget, 15), &mut remaining, &mut omitted) {
            if p.cand.decision == Decision::Included {
                memory.push(p.note);
            }
            note_cands.push(p.cand);
        }
    }
    note_cands.extend(mem_dups);

    let built;
    let graph = match signals.graph {
        Some(g) => g,
        None => {
            built = ImportGraph::build(index);
            &built
        }
    };
    let max_files = if req.max_files == 0 { 12 } else { req.max_files };
    let scored = score_files(index, req, graph, signals.churn, &query);
    let mut files = Vec::new();
    let mut file_cands: Vec<FileCandidate> = Vec::new();
    if !scored.is_empty() && remaining > SECTION_OVERHEAD {
        remaining -= SECTION_OVERHEAD;
    }
    for (rank, (path, cand)) in scored.into_iter().enumerate() {
        let mut fc = FileCandidate {
            path: path.clone(),
            score: cand.score(),
            base_score: cand.base,
            graph_score: cand.graph,
            churn_boost: cand.churn.max(1.0),
            reasons: cand.reasons.clone(),
            decision: Decision::BelowCutoff,
            tokens: 0,
        };
        if rank >= max_files {
            if file_cands.len() < max_files + MAX_EXPLAINED_REJECTS {
                file_cands.push(fc);
            }
            continue;
        }
        let Some(meta) = index.get(&path) else { continue };
        let Some(content) = reader.read(&path) else {
            omitted.push(format!("file:{path}"));
            fc.decision = Decision::OmittedUnreadable;
            file_cands.push(fc);
            continue;
        };
        let reason = reason_text(&cand.reasons);
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let whole = excerpt_tokens(meta.language, &content);
        if whole <= WHOLE_FILE_TOKENS.max(EXCERPT_OVERHEAD + 1) && whole <= remaining {
            remaining -= whole;
            fc.decision = Decision::Whole;
            fc.tokens = whole;
            file_cands.push(fc);
            files.push(ContextFile {
                path,
                language: meta.language,
                reason,
                score: cand.score(),
                whole_file: true,
                excerpts: vec![make_excerpt(&lines, 0, lines.len() - 1)],
                tokens: whole,
                reasons: cand.reasons,
            });
            continue;
        }
        let mut anchor_lines = anchors(&lines, &meta.symbols, &query, &cand.anchors);
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
            omitted.push(format!("file:{path}"));
            fc.decision = Decision::OmittedBudget;
            file_cands.push(fc);
            continue;
        }
        fc.decision = Decision::Excerpt;
        fc.tokens = used;
        file_cands.push(fc);
        files.push(ContextFile {
            path,
            language: meta.language,
            reason,
            score: cand.score(),
            whole_file: false,
            excerpts,
            tokens: used,
            reasons: cand.reasons,
        });
    }
    // Explanation order: included first (bundle order), then the rest by score.
    file_cands.sort_by_key(|c| !matches!(c.decision, Decision::Whole | Decision::Excerpt));

    ContextPlan {
        bundle: ContextBundle {
            task,
            rules,
            memory,
            files,
            handoff,
            total_tokens: budget.saturating_sub(remaining),
            budget,
            omitted,
        },
        files: file_cands,
        notes: note_cands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::MemReader;
    use orkestr_domain::{now, MemoryItemId, ProjectId};

    fn mem(title: &str, body: &str, kind: MemoryKind, scope: MemoryScope) -> MemoryItem {
        MemoryItem {
            id: MemoryItemId::new(),
            scope,
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

    fn big(name: &str, at: usize) -> String {
        let mut s = String::new();
        for i in 0..400 {
            if i == at {
                s.push_str(&format!("pub fn {name}(x: u32) -> u32 {{\n"));
            } else {
                s.push_str(&format!("    let pad_{i} = {i}; // filler line {i}\n"));
            }
        }
        s
    }

    fn project() -> Vec<(String, String)> {
        vec![
            ("src/lib.rs".into(), "pub mod payroll;\npub mod report;\npub mod ui;\n".into()),
            ("src/payroll.rs".into(), big("compute_overtime", 200)),
            (
                "src/report.rs".into(),
                "use crate::payroll::compute_overtime;\n\npub fn monthly() -> u32 { compute_overtime(3) }\n".into(),
            ),
            ("src/export.rs".into(), "pub fn csv() { let v = compute_overtime(1); }\n".into()),
            ("src/ui.rs".into(), "pub fn render_button() {}\n".into()),
            ("docs/notes.md".into(), "# Unrelated\nNothing here.\n".into()),
        ]
    }

    fn setup() -> (FileIndex, MemReader) {
        let src = project();
        let pairs: Vec<(&str, &str)> = src.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        (FileIndex::from_sources(pairs.clone()), MemReader::from_sources(pairs))
    }

    #[test]
    fn neighbours_references_and_excerpts() {
        let (idx, reader) = setup();
        let req = OptimizeRequest {
            task: "Fix rounding in compute_overtime".into(),
            token_budget: 3000,
            max_files: 6,
            ..OptimizeRequest::default()
        };
        let plan = optimize(&idx, &reader, &req, Signals::default());
        let b = &plan.bundle;
        assert_eq!(b.files[0].path, "src/payroll.rs");
        assert!(!b.files[0].whole_file, "big file is excerpted");
        assert!(b.files[0].excerpts.iter().any(|e| e.start_line <= 201 && e.end_line >= 201));
        let report = b.files.iter().find(|f| f.path == "src/report.rs").expect("importer included");
        assert!(report.reasons.contains(&Reason::Imports { file: "src/payroll.rs".into() }), "{:?}", report.reasons);
        let export = plan.files.iter().find(|f| f.path == "src/export.rs").expect("reference found");
        assert!(export
            .reasons
            .contains(&Reason::References { symbol: "compute_overtime".into(), file: "src/payroll.rs".into() }));
        let lib = plan.files.iter().find(|f| f.path == "src/lib.rs").expect("mod parent is a neighbour");
        assert!(lib.reasons.contains(&Reason::Imports { file: "src/payroll.rs".into() }));
        assert!(b.files.iter().all(|f| f.path != "src/ui.rs" && f.path != "docs/notes.md"));
        assert!(b.total_tokens <= 3000);
        // Every included file appears once (dedupe) and the explanation lists it first.
        let paths: BTreeSet<&str> = b.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths.len(), b.files.len());
        assert_eq!(plan.files[0].path, b.files[0].path);
        // Deterministic.
        assert_eq!(optimize(&idx, &reader, &req, Signals::default()), plan);
    }

    #[test]
    fn churn_boosts_relevant_files_only() {
        let (idx, reader) = setup();
        let churn = ChurnStats::parse("\u{1e}100\n\nsrc/export.rs\nsrc/ui.rs\n\u{1e}99\n\nsrc/export.rs\nsrc/ui.rs\n");
        let req = OptimizeRequest { task: "compute_overtime".into(), token_budget: 4000, ..OptimizeRequest::default() };
        let plan = optimize(&idx, &reader, &req, Signals { graph: None, churn: Some(&churn) });
        let export = plan.files.iter().find(|f| f.path == "src/export.rs").unwrap();
        assert!(export.churn_boost > 1.05);
        assert!(export.reasons.iter().any(|r| matches!(r, Reason::RecentChange { commits: 2 })));
        assert!(plan.files.iter().all(|f| f.path != "src/ui.rs"), "churn alone never adds a file");
    }

    #[test]
    fn rules_precedence_and_memory_ranking() {
        let (idx, reader) = setup();
        let p = ProjectId::new();
        let mut global = mem("Money", "Use floats.", MemoryKind::Convention, MemoryScope::Global);
        global.key = Some("money".into());
        let mut project =
            mem("Money", "Use integer cents.", MemoryKind::Convention, MemoryScope::Project { project_id: p });
        project.key = Some("money".into());
        let other_global = mem("Tabs", "Use spaces.", MemoryKind::Convention, MemoryScope::Global);
        let mut pinned = mem("Pinned", "Always.", MemoryKind::Convention, MemoryScope::Global);
        pinned.pinned = true;
        let rules = vec![
            RuleInput { item: global.clone(), key: Some("money".into()) },
            RuleInput { item: other_global.clone(), key: None },
            RuleInput { item: project.clone(), key: Some("money".into()) },
            RuleInput { item: pinned.clone(), key: None },
        ];
        let note = mem("Overtime note", "Generic.", MemoryKind::Note, MemoryScope::Project { project_id: p });
        let mut mistake = mem(
            "Overtime rounding mistake",
            "Rounded down.",
            MemoryKind::Mistake,
            MemoryScope::Project { project_id: p },
        );
        mistake.tags = vec!["overtime".into(), "ui".into()];
        let issue =
            mem("Known overtime bug", "Off by one.", MemoryKind::KnownIssue, MemoryScope::Project { project_id: p });
        let dup = MemoryInput { item: note.clone(), relevance: 0.9 };
        let memory = vec![
            MemoryInput { item: note.clone(), relevance: 0.9 },
            MemoryInput { item: mistake.clone(), relevance: 0.7 },
            MemoryInput { item: issue.clone(), relevance: 0.5 },
            dup,
            MemoryInput { item: project.clone(), relevance: 1.0 }, // already a rule
        ];
        let req = OptimizeRequest {
            task: "Fix overtime rounding".into(),
            token_budget: 4000,
            rules,
            memory,
            ..OptimizeRequest::default()
        };
        let plan = optimize(&idx, &reader, &req, Signals::default());
        let b = &plan.bundle;
        let titles: Vec<(&str, &str)> = b.rules.iter().map(|r| (r.title.as_str(), r.body.as_str())).collect();
        assert_eq!(titles, vec![("Pinned", "Always."), ("Money", "Use integer cents."), ("Tabs", "Use spaces.")]);
        assert!(b.rules[1].reasons.contains(&Reason::Rule { origin: "project".into(), overrides_global: true }));
        assert!(plan
            .notes
            .iter()
            .any(|n| n.source == format!("rule:{}", global.id) && n.decision == Decision::Duplicate));
        let mem_titles: Vec<&str> = b.memory.iter().map(|m| m.title.as_str()).collect();
        // mistake: 0.7 × 1.5 + tag 0.25; known issue: 0.5 × 1.5; note: 0.9 × 0.8.
        assert_eq!(mem_titles, vec!["Overtime rounding mistake", "Known overtime bug", "Overtime note"]);
        assert!(b.memory[0].reasons.contains(&Reason::Mistake));
        assert!(matches!(&b.memory[0].reasons[0], Reason::Memory { tags, .. } if tags == &vec!["overtime".to_owned()]));
        assert!(b.memory[1].reasons.contains(&Reason::KnownIssue));
        assert_eq!(plan.notes.iter().filter(|n| n.decision == Decision::Duplicate).count(), 3);
    }

    #[test]
    fn model_budget() {
        let b = ModelBudget { context_window: 128_000, reserve_output: 0, system_tokens: 2_000, cap: 0 };
        assert_eq!(b.reserved_output(), 8192);
        assert_eq!(b.effective(), ((128_000 - 8192 - 2000) as f64 * 0.85).floor() as u32);
        let capped = ModelBudget { cap: 12_000, ..b };
        assert_eq!(capped.effective(), 12_000);
        let tiny = ModelBudget { context_window: 4096, reserve_output: 0, system_tokens: 3500, cap: 12_000 };
        assert_eq!(tiny.reserved_output(), 1024);
        assert_eq!(tiny.effective(), 0);
    }

    #[test]
    fn focus_paths_and_cutoff_explained() {
        let (idx, reader) = setup();
        let req = OptimizeRequest {
            task: "polish".into(),
            token_budget: 2000,
            max_files: 1,
            focus_paths: vec!["src\\ui.rs".into(), "missing.rs".into()],
            ..OptimizeRequest::default()
        };
        let plan = optimize(&idx, &reader, &req, Signals::default());
        assert_eq!(plan.bundle.files.len(), 1);
        assert_eq!(plan.bundle.files[0].path, "src/ui.rs");
        assert!(plan.bundle.files[0].reasons.contains(&Reason::Focus));
        // lib.rs imports ui.rs → neighbour, but cut by max_files = 1.
        assert!(plan.files.iter().any(|f| f.path == "src/lib.rs" && f.decision == Decision::BelowCutoff));
    }

    #[test]
    fn reason_serialization_shape() {
        let v = serde_json::to_value(Reason::References { symbol: "a".into(), file: "b".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "references", "symbol": "a", "file": "b"}));
        let v = serde_json::to_value(Reason::Rule { origin: "project".into(), overrides_global: true }).unwrap();
        assert_eq!(v["overridesGlobal"], true);
    }
}
