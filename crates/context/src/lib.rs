//! # orkestr-context
//!
//! Context optimizer (MASTER_SPEC §20, §68.40): never send the whole
//! repository. Given a task it assembles a [`ContextBundle`] — task, project
//! rules, relevant memory, relevant file excerpts and a concise handoff —
//! within a token budget, deterministically.
//!
//! * [`index`] — [`FileIndex`]: project files (gitignore-aware walk) with
//!   language, size, token estimate and regex-extracted [`symbols`]
//!   (rs/ts/tsx/js/py/go/cs/java + markdown headings).
//! * [`rank`] — BM25 relevance of files for a task text over path tokens
//!   (×3), symbol tokens (×2) and a content sample (×1), identifier-aware
//!   tokenization (camelCase / snake_case), plus a strong boost for paths the
//!   task mentions explicitly.
//! * [`tokens`] — token estimator (≈ chars/4, ×1.25 for code).
//! * [`imports`] — import / dependency graph (rs, ts/js, py, go, cs).
//! * [`signals`] — recency / churn from VCS history.
//! * [`optimizer`] — the full pipeline: seeds, import neighbours, symbol
//!   references, churn, rule precedence, memory relevance, model-aware
//!   budgeting, excerpting and per-item explanations ([`ContextPlan`]).
//! * [`handoff`] — handoff compression (cheap model or deterministic).
//! * [`cache`] — cached indexes with `FileChanged` invalidation.
//! * [`bundle`] — bundle types, [`build_bundle`] and the prompt renderer.
//!
//! Engines call [`FileIndex::build`] from a blocking context (it reads files).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bundle;
pub mod cache;
pub mod handoff;
pub mod imports;
pub mod index;
pub mod optimizer;
pub mod rank;
pub mod signals;
pub mod symbols;
pub mod tokens;

pub use bundle::{build_bundle, ContextBundle, ContextFile, ContextNote, ContextRequest, Excerpt};
pub use cache::{global_cache, IndexCache, IndexedProject};
pub use handoff::{compress_deterministic, compress_history, CompressedHistory, CompressionMethod, HistoryEntry, Summarizer};
pub use imports::{extract_imports, ImportGraph};
pub use index::{FileIndex, FileReader, FsReader, IndexOptions, IndexedFile, MemReader};
pub use optimizer::{
    optimize, ContextPlan, Decision, FileCandidate, MemoryInput, ModelBudget, NoteCandidate, OptimizeRequest, Reason,
    RuleInput, Signals,
};
pub use signals::ChurnStats;
pub use rank::{rank_files, RankedFile};
pub use symbols::{extract_symbols, Language, Symbol, SymbolKind};
pub use tokens::{estimate_code_tokens, estimate_tokens};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// The project root is missing or not a directory.
    #[error("invalid project root: {0}")]
    InvalidRoot(String),
    /// I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
