//! Working-tree scan for the `workspace:` stream with the §2.1 exclusion rules.
//!
//! Read-only: never runs Git, never touches `.git/`.

use std::path::{Path, PathBuf};

/// Files larger than this are excluded (§2.1 rule 5).
pub const MAX_FILE: u64 = 512 * 1024 * 1024;

/// A file found by the scan.
#[derive(Debug, Clone)]
pub struct LocalFile {
    /// Relative POSIX path.
    pub path: String,
    /// Absolute path.
    pub abs: PathBuf,
    /// Size.
    pub size: u64,
    /// Modification time (ns since epoch, best effort).
    pub mtime_ns: i64,
    /// Executable bit.
    pub exec: bool,
}

const HARD_DIRS: &[&str] = &[".git", "node_modules", "target", ".orkestr"];
const JUNK: &[&str] = &["Thumbs.db", ".DS_Store", "desktop.ini"];

/// Whether a file name matches the default secret patterns (§2.1 rule 2).
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower == ".env" || (lower.starts_with(".env.") && lower != ".env.example"))
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || (lower.starts_with("credentials") && lower.ends_with(".json"))
        || lower == ".npmrc"
}

/// Whether a relative path is excluded by the hard rules (used when applying
/// remote manifests too, so a hostile manifest cannot write into `.git/`).
#[must_use]
pub fn is_excluded_path(rel: &str) -> bool {
    let mut segs = rel.split('/').peekable();
    while let Some(s) = segs.next() {
        if segs.peek().is_some() {
            if HARD_DIRS.contains(&s) {
                return true;
            }
        } else if JUNK.contains(&s) || is_secret_name(s) {
            return true;
        }
    }
    false
}

/// Scans `root` (gitignore + `.orkestrignore` aware), sorted by path.
#[must_use]
pub fn scan(root: &Path) -> Vec<LocalFile> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(false)
        .require_git(false)
        .follow_links(false)
        .add_custom_ignore_filename(".orkestrignore")
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir()) && HARD_DIRS.contains(&name.as_ref()))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let Some(rel) = rel.to_str() else { continue };
        let rel = rel.replace('\\', "/");
        if rel.is_empty() || is_excluded_path(&rel) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE {
            continue;
        }
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        out.push(LocalFile {
            path: rel,
            abs: entry.path().to_path_buf(),
            size: meta.len(),
            mtime_ns,
            exec: is_exec(&meta),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(unix)]
fn is_exec(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for (p, c) in [
            ("src/main.rs", "fn main(){}"),
            (".git/config", "x"),
            ("target/debug/a", "x"),
            ("web/node_modules/m/index.js", "x"),
            (".env", "SECRET=1"),
            (".env.example", "SECRET="),
            ("build/out.o", "x"),
            (".gitignore", "build/\n"),
            ("notes.tmp", "x"),
            (".orkestrignore", "*.tmp\n"),
        ] {
            let f = r.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        }
        let paths: Vec<_> = scan(r).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec![".env.example", ".gitignore", ".orkestrignore", "src/main.rs"]);
        assert!(is_excluded_path(".git/hooks/pre-commit"));
        assert!(!is_excluded_path("src/target.rs"));
    }
}
