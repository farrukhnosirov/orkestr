//! Project file index and safe file reading.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::symbols::{extract_symbols, Language, Symbol};
use crate::tokens::{estimate_code_tokens, estimate_tokens};
use crate::ContextError;

/// Extensions of files that are indexed (lowercase).
pub const INDEXED_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "py", "pyi", "go", "cs", "java", "md", "markdown",
    "mdx", "txt", "toml", "json", "yaml", "yml", "sql", "graphql", "proto", "html", "css", "scss", "sh", "ps1", "kt",
    "rb", "php", "c", "h", "cpp", "hpp", "swift", "vue", "svelte",
];

/// Index limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexOptions {
    /// Max files indexed.
    pub max_files: usize,
    /// Larger files are indexed by path only (no symbols / sample).
    pub max_file_bytes: u64,
    /// Characters of content kept for ranking.
    pub sample_chars: usize,
    /// Max folder depth.
    pub max_depth: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self { max_files: 20_000, max_file_bytes: 512 * 1024, sample_chars: 4096, max_depth: 32 }
    }
}

/// One indexed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedFile {
    /// Path relative to the root, `/`-separated.
    pub path: String,
    /// Language.
    pub language: Language,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Estimated tokens of the whole file.
    pub est_tokens: u32,
    /// Extracted symbols / headings.
    pub symbols: Vec<Symbol>,
    /// First characters of the content (ranking only).
    #[serde(skip)]
    pub sample: String,
}

impl IndexedFile {
    /// Indexes in-memory content.
    #[must_use]
    pub fn from_content(path: &str, content: &str, sample_chars: usize) -> Self {
        let language = Language::from_path(path);
        let est_tokens = if language.is_code() { estimate_code_tokens(content) } else { estimate_tokens(content) };
        Self {
            path: normalize(path),
            language,
            size_bytes: content.len() as u64,
            est_tokens,
            symbols: extract_symbols(language, content),
            sample: content.chars().take(sample_chars).collect(),
        }
    }
}

/// The index of one project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileIndex {
    /// Indexed files, sorted by path.
    pub files: Vec<IndexedFile>,
    /// Files skipped (binary, unreadable).
    pub skipped: u32,
    /// True when `max_files` cut the walk short.
    pub truncated: bool,
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

impl FileIndex {
    /// Walks `root` (honouring `.gitignore`, skipping hidden files) and indexes
    /// supported text files. Blocking.
    pub fn build(root: &Path, opts: IndexOptions) -> Result<Self, ContextError> {
        if !root.is_dir() {
            return Err(ContextError::InvalidRoot(root.display().to_string()));
        }
        let mut index = FileIndex::default();
        let mut paths: Vec<PathBuf> = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .require_git(false)
            .follow_links(false)
            .max_depth(Some(opts.max_depth))
            .build();
        for entry in walker {
            let Ok(entry) = entry else {
                index.skipped += 1;
                continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            if ext.as_deref().is_some_and(|e| INDEXED_EXTENSIONS.contains(&e)) {
                paths.push(path);
            }
        }
        paths.sort();
        if paths.len() > opts.max_files {
            paths.truncate(opts.max_files);
            index.truncated = true;
        }
        for path in paths {
            let rel = normalize(&path.strip_prefix(root).unwrap_or(&path).to_string_lossy());
            let Ok(meta) = std::fs::metadata(&path) else {
                index.skipped += 1;
                continue;
            };
            if meta.len() > opts.max_file_bytes {
                let language = Language::from_path(&rel);
                #[allow(clippy::cast_possible_truncation)]
                let est = (meta.len() / 3).min(u64::from(u32::MAX)) as u32;
                index.files.push(IndexedFile {
                    path: rel,
                    language,
                    size_bytes: meta.len(),
                    est_tokens: est,
                    symbols: Vec::new(),
                    sample: String::new(),
                });
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) if !bytes.iter().take(8192).any(|b| *b == 0) => {
                    let text = String::from_utf8_lossy(&bytes);
                    index.files.push(IndexedFile::from_content(&rel, &text, opts.sample_chars));
                }
                _ => index.skipped += 1,
            }
        }
        tracing::debug!(root = %root.display(), files = index.files.len(), skipped = index.skipped, "context index built");
        Ok(index)
    }

    /// Indexes in-memory `(path, content)` pairs (tests, editors with unsaved buffers).
    #[must_use]
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let opts = IndexOptions::default();
        let mut files: Vec<IndexedFile> =
            sources.into_iter().map(|(p, c)| IndexedFile::from_content(p, c, opts.sample_chars)).collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Self { files, skipped: 0, truncated: false }
    }

    /// Looks a file up by relative path.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&IndexedFile> {
        let p = normalize(path);
        self.files.binary_search_by(|f| f.path.as_str().cmp(&p)).ok().and_then(|i| self.files.get(i))
    }
}

/// Reads file contents for excerpts.
pub trait FileReader: Send + Sync {
    /// Content of a project-relative path, `None` when unavailable or unsafe.
    fn read(&self, path: &str) -> Option<String>;
}

/// Reads files under a root, refusing absolute paths, `..`, and anything that
/// resolves (e.g. through a symlink) outside the root.
#[derive(Debug, Clone)]
pub struct FsReader {
    root: PathBuf,
    max_bytes: u64,
}

impl FsReader {
    /// Reader confined to `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), max_bytes: 2 * 1024 * 1024 }
    }

    fn resolve(&self, rel: &str) -> Option<PathBuf> {
        let rel_path = Path::new(rel);
        if rel.is_empty() || rel_path.is_absolute() || rel.starts_with('\\') || rel.starts_with('/') {
            return None;
        }
        if rel_path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
            return None;
        }
        let root = std::fs::canonicalize(&self.root).ok()?;
        let full = std::fs::canonicalize(root.join(rel_path)).ok()?;
        full.starts_with(&root).then_some(full)
    }
}

impl FileReader for FsReader {
    fn read(&self, path: &str) -> Option<String> {
        let full = self.resolve(path)?;
        let meta = std::fs::metadata(&full).ok()?;
        if !meta.is_file() || meta.len() > self.max_bytes {
            return None;
        }
        let bytes = std::fs::read(full).ok()?;
        if bytes.iter().take(8192).any(|b| *b == 0) {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// In-memory reader.
#[derive(Debug, Clone, Default)]
pub struct MemReader(pub HashMap<String, String>);

impl MemReader {
    /// From `(path, content)` pairs.
    #[must_use]
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self(sources.into_iter().map(|(p, c)| (normalize(p), c.to_owned())).collect())
    }
}

impl FileReader for MemReader {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(&normalize(path)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_index_respecting_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn compute() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# Readme\n").unwrap();
        std::fs::write(root.join("target/gen.rs"), "fn ignored() {}").unwrap();
        std::fs::write(root.join("logo.png"), [0u8, 1]).unwrap();
        std::fs::write(root.join("bin.rs"), [b'f', 0u8, b'n']).unwrap();
        let idx = FileIndex::build(root, IndexOptions::default()).unwrap();
        let paths: Vec<_> = idx.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["README.md", "src/lib.rs"]);
        assert_eq!(idx.skipped, 1, "binary file skipped");
        assert_eq!(idx.get("src\\lib.rs").unwrap().symbols[0].name, "compute");
        assert!(FileIndex::build(&root.join("nope"), IndexOptions::default()).is_err());

        let limited = FileIndex::build(root, IndexOptions { max_files: 1, ..IndexOptions::default() }).unwrap();
        assert!(limited.truncated);
        assert_eq!(limited.files.len(), 1);
    }

    #[test]
    fn fs_reader_is_confined() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "top secret").unwrap();
        let r = FsReader::new(&root);
        assert_eq!(r.read("src/a.rs").as_deref(), Some("fn a() {}"));
        assert_eq!(r.read("./src/a.rs").as_deref(), Some("fn a() {}"));
        assert_eq!(r.read("../secret.txt"), None);
        assert_eq!(r.read("src/../../secret.txt"), None);
        assert_eq!(r.read(&dir.path().join("secret.txt").to_string_lossy()), None);
        assert_eq!(r.read("/etc/passwd"), None);
        assert_eq!(r.read("src"), None, "directories are not files");
        assert_eq!(r.read("missing.rs"), None);
    }

    #[test]
    fn mem_reader_and_sources() {
        let idx = FileIndex::from_sources([("b.ts", "export function b() {}"), ("a.py", "def a(): pass")]);
        assert_eq!(idx.files[0].path, "a.py");
        let r = MemReader::from_sources([("x/y.rs", "fn y() {}")]);
        assert!(r.read("x\\y.rs").is_some());
    }
}
