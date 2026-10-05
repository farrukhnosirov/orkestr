//! Incremental project scanner: walks the root (respecting `.gitignore`,
//! skipping dependency / build folders) and re-parses only files whose size
//! or modification time changed since the previous scan.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use super::facts::{self, FileFacts, Role};

/// Directories never scanned (dependencies, build output, caches, VCS).
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
    "vendor",
    "obj",
    ".next",
    ".nuxt",
    ".turbo",
    ".gradle",
    "coverage",
    ".idea",
    ".vscode",
];

/// Scan limits.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Max files walked (the model is marked truncated beyond it).
    pub max_files: usize,
    /// Files larger than this are classified but not parsed.
    pub max_file_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { max_files: 50_000, max_file_bytes: 512 * 1024 }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    len: u64,
    mtime: Option<SystemTime>,
    facts: Arc<FileFacts>,
}

/// Result of one scan.
#[derive(Debug, Clone, Default)]
pub struct ScanOutput {
    /// Every walked file (relative, `/` separators, sorted).
    pub files: Vec<String>,
    /// Facts of every classified file.
    pub facts: BTreeMap<String, Arc<FileFacts>>,
    /// Files parsed in this scan.
    pub parsed: u32,
    /// The walk hit `max_files`.
    pub truncated: bool,
    /// Duration.
    pub elapsed_ms: u64,
    /// Walk problems.
    pub warnings: Vec<String>,
}

/// Scanner of one project root with a per-file fact cache.
#[derive(Debug)]
pub struct Scanner {
    root: PathBuf,
    opts: ScanOptions,
    cache: HashMap<String, Entry>,
}

impl Scanner {
    /// New scanner.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, opts: ScanOptions) -> Self {
        Self { root: root.into(), opts, cache: HashMap::new() }
    }

    /// Root folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Cached entries.
    #[must_use]
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Scans; `full` drops the cache first.
    ///
    /// # Errors
    /// The root is missing or not a directory.
    pub fn scan(&mut self, full: bool) -> std::io::Result<ScanOutput> {
        let started = Instant::now();
        if !self.root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("project root {} is not a directory", self.root.display()),
            ));
        }
        if full {
            self.cache.clear();
        }
        let mut out = ScanOutput::default();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|e| {
                let is_dir = e.file_type().is_some_and(|t| t.is_dir());
                !(is_dir && e.depth() > 0 && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
            })
            .build();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    if out.warnings.len() < 20 {
                        out.warnings.push(format!("walk: {e}"));
                    }
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            if out.files.len() >= self.opts.max_files {
                out.truncated = true;
                break;
            }
            let Ok(rel) = entry.path().strip_prefix(&self.root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let role = facts::classify(&rel);
            out.files.push(rel.clone());
            if role == Role::Other {
                continue;
            }
            let meta = entry.metadata().ok();
            let len = meta.as_ref().map_or(0, std::fs::Metadata::len);
            let mtime = meta.as_ref().and_then(|m| m.modified().ok());
            seen.insert(rel.clone(), ());
            if let Some(cached) = self.cache.get(&rel) {
                if cached.len == len && cached.mtime == mtime && mtime.is_some() {
                    out.facts.insert(rel, Arc::clone(&cached.facts));
                    continue;
                }
            }
            let f = if role.needs_content() && len <= self.opts.max_file_bytes {
                match std::fs::read(entry.path()) {
                    Ok(bytes) => facts::extract(&rel, role, &String::from_utf8_lossy(&bytes)),
                    Err(e) => {
                        let mut f = facts::extract(&rel, role, "");
                        f.warnings.push(format!("{rel}: {e}"));
                        f
                    }
                }
            } else {
                facts::extract(&rel, role, "")
            };
            out.parsed += 1;
            let f = Arc::new(f);
            self.cache.insert(rel.clone(), Entry { len, mtime, facts: Arc::clone(&f) });
            out.facts.insert(rel, f);
        }
        // Forget deleted files.
        self.cache.retain(|k, _| seen.contains_key(k));
        out.files.sort();
        out.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_parsing_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("node_modules/x/package.json"), "{\"name\":\"x\"}").unwrap();
        std::fs::write(root.join("README.md"), "hi").unwrap();
        let mut s = Scanner::new(root, ScanOptions::default());
        let a = s.scan(false).unwrap();
        assert_eq!(a.files, vec!["Cargo.toml", "README.md", "src/main.rs"]);
        assert_eq!(a.parsed, 2);
        let b = s.scan(false).unwrap();
        assert_eq!(b.parsed, 0, "unchanged files come from the cache");
        std::fs::write(root.join("src/main.rs"), "fn main() { println!(\"changed\"); }").unwrap();
        let c = s.scan(false).unwrap();
        assert_eq!(c.parsed, 1);
        std::fs::remove_file(root.join("src/main.rs")).unwrap();
        let d = s.scan(false).unwrap();
        assert_eq!(s.cached(), 1);
        assert!(!d.facts.contains_key("src/main.rs"));
        let e = s.scan(true).unwrap();
        assert_eq!(e.parsed, 1);
        assert!(Scanner::new(root.join("missing"), ScanOptions::default()).scan(false).is_err());
    }

    #[test]
    fn truncates_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "").unwrap();
        }
        let mut s = Scanner::new(dir.path(), ScanOptions { max_files: 3, ..ScanOptions::default() });
        let out = s.scan(false).unwrap();
        assert!(out.truncated);
        assert_eq!(out.files.len(), 3);
    }
}
