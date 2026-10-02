//! # orkestr-context
//!
//! Context optimizer, basic version (MASTER_SPEC §20): never send the whole
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
//! * [`bundle`] — the budget packer and prompt renderer.
//!
//! Engines call [`FileIndex::build`] from a blocking context (it reads files).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bundle;
pub mod index;
pub mod rank;
pub mod symbols;
pub mod tokens;

pub use bundle::{build_bundle, ContextBundle, ContextFile, ContextNote, ContextRequest, Excerpt};
pub use index::{FileIndex, FileReader, FsReader, IndexOptions, IndexedFile, MemReader};
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
