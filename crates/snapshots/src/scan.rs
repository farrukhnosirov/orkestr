//! Workspace scanning (ignore rules, limits) and safe file writes/deletes.
//!
//! Ignore rules: `.gitignore` (+ `.git/info/exclude`, global excludes),
//! `.ignore`, `.orkestrignore`, plus default ignored directory names
//! (`.git`, `node_modules`, `target`). Symlinks are never followed nor
//! captured. Writes are atomic (temp file + rename) and refuse to go through
//! symlinked directories or outside the workspace root.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use orkestr_protocol::methods::snapshots::{ExcludedEntry, ExclusionReason};

use crate::error::{SnapshotError, SnapshotResult};

/// Regular file mode.
pub const MODE_FILE: u32 = 0o100_644;
/// Executable file mode.
pub const MODE_EXEC: u32 = 0o100_755;

/// Directory names never captured.
pub const DEFAULT_IGNORED_DIRS: &[&str] = &[".git", "node_modules", "target"];

/// Limits and ignore configuration.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Files larger than this are excluded (default 50 MiB).
    pub max_file_bytes: u64,
    /// Snapshot fails when captured bytes exceed this (default 4 GiB).
    pub max_total_bytes: u64,
    /// Snapshot fails when more files than this (default 200 000).
    pub max_files: usize,
    /// Directory/file names always ignored (default [`DEFAULT_IGNORED_DIRS`]).
    pub ignored_names: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 50 * 1024 * 1024,
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_files: 200_000,
            ignored_names: DEFAULT_IGNORED_DIRS.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

/// A file found by [`list_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// `/`-separated relative path.
    pub rel: String,
    /// Absolute path.
    pub abs: PathBuf,
    /// Size.
    pub size: u64,
    /// Mode.
    pub mode: u32,
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o111 != 0 {
        MODE_EXEC
    } else {
        MODE_FILE
    }
}

#[cfg(not(unix))]
fn mode_of(_meta: &std::fs::Metadata) -> u32 {
    MODE_FILE
}

/// Applies an executable bit (Unix only; no-op elsewhere).
#[cfg(unix)]
pub fn apply_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perm = if mode == MODE_EXEC { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(perm))
}

/// Applies an executable bit (Unix only; no-op elsewhere).
#[cfg(not(unix))]
pub fn apply_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

fn rel_string(root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Lists every non-ignored regular file under `root` (sorted by path) plus
/// the paths skipped for size/symlink/readability.
pub fn list_files(root: &Path, opts: &ScanOptions) -> SnapshotResult<(Vec<ScannedFile>, Vec<ExcludedEntry>)> {
    if !root.is_dir() {
        return Err(SnapshotError::NotFound(format!("workspace directory {} does not exist", root.display())));
    }
    let ignored = opts.ignored_names.clone();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .parents(true)
        .add_custom_ignore_filename(".orkestrignore")
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            !ignored.iter().any(|i| *i == name)
        })
        .build();
    let mut files = Vec::new();
    let mut excluded = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(error = %e, "snapshot walk error");
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(rel) = rel_string(root, entry.path()) else { continue };
        let Some(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            excluded.push(ExcludedEntry { path: rel, reason: ExclusionReason::Symlink });
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                excluded.push(ExcludedEntry { path: rel, reason: ExclusionReason::Unreadable });
                continue;
            }
        };
        if meta.len() > opts.max_file_bytes {
            excluded.push(ExcludedEntry { path: rel, reason: ExclusionReason::OverSizeLimit });
            continue;
        }
        files.push(ScannedFile { rel, abs: entry.path().to_path_buf(), size: meta.len(), mode: mode_of(&meta) });
        if files.len() > opts.max_files {
            return Err(SnapshotError::TooLarge(format!(
                "workspace has more than {} files; add ignore rules (.gitignore / .orkestrignore)",
                opts.max_files
            )));
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    excluded.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((files, excluded))
}

/// Current state of one file: `(blake3 hex, mode, size)`.
pub type FileState = (String, u32, u64);

/// Hashes a file's content (blake3 hex).
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Validates a manifest/client relative path and returns its components.
pub fn validate_rel(rel: &str) -> SnapshotResult<Vec<String>> {
    if rel.is_empty() || rel.contains('\0') || rel.contains('\\') || rel.starts_with('/') {
        return Err(SnapshotError::invalid(format!("invalid relative path `{rel}`")));
    }
    let parts: Vec<String> = rel.split('/').map(str::to_owned).collect();
    for p in &parts {
        if p.is_empty() || p == "." || p == ".." || p.contains(':') {
            return Err(SnapshotError::invalid(format!("invalid relative path `{rel}`")));
        }
    }
    Ok(parts)
}

/// Resolves `rel` under `root`, refusing `..`, absolute paths and any
/// existing symlink along the way (so writes cannot escape the workspace).
pub fn safe_target(root: &Path, rel: &str) -> SnapshotResult<PathBuf> {
    let parts = validate_rel(rel)?;
    let mut cur = root.to_path_buf();
    for (i, p) in parts.iter().enumerate() {
        cur.push(p);
        if let Ok(m) = std::fs::symlink_metadata(&cur) {
            if m.file_type().is_symlink() {
                return Err(SnapshotError::invalid(format!(
                    "refusing to write through symlink at {}",
                    parts[..=i].join("/")
                )));
            }
        }
    }
    Ok(cur)
}

/// Reads the current state of `rel` (None when missing / not a regular file).
pub fn current_state(root: &Path, rel: &str) -> SnapshotResult<Option<FileState>> {
    let path = safe_target(root, rel)?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => {
            let bytes = std::fs::read(&path)?;
            Ok(Some((hash_bytes(&bytes), mode_of(&m), bytes.len() as u64)))
        }
        Ok(_) => Ok(Some(("<not-a-file>".to_owned(), 0, 0))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Hashes every non-ignored file of the workspace.
pub fn current_tree(root: &Path, opts: &ScanOptions) -> SnapshotResult<BTreeMap<String, FileState>> {
    let (files, _) = list_files(root, opts)?;
    let mut out = BTreeMap::new();
    for f in files {
        match std::fs::read(&f.abs) {
            Ok(bytes) => {
                out.insert(f.rel, (hash_bytes(&bytes), f.mode, bytes.len() as u64));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Atomically writes `bytes` to `rel` under `root` (creating parents).
pub fn write_file(root: &Path, rel: &str, bytes: &[u8], mode: u32) -> SnapshotResult<()> {
    let target = safe_target(root, rel)?;
    if target.is_dir() {
        return Err(SnapshotError::Stale(format!("{rel} is now a directory")));
    }
    let parent = target.parent().ok_or_else(|| SnapshotError::invalid(format!("no parent for {rel}")))?;
    std::fs::create_dir_all(parent)?;
    // Re-check after creating parents (a racing symlink must not redirect us).
    safe_target(root, rel)?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = parent.join(format!(".{name}.orkestr-{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    apply_mode(&tmp, mode)?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Deletes `rel` and then any parent directories left empty (up to `root`).
pub fn delete_file(root: &Path, rel: &str) -> SnapshotResult<()> {
    let target = safe_target(root, rel)?;
    match std::fs::remove_file(&target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    let mut dir = target.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        let empty = std::fs::read_dir(&d).map(|mut it| it.next().is_none()).unwrap_or(false);
        if !empty || std::fs::remove_dir(&d).is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_respecting_ignores_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        std::fs::write(root.join(".orkestrignore"), "secret.txt\n").unwrap();
        std::fs::create_dir_all(root.join("src/deep dir")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("src/deep dir/ü file.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("a.log"), "x").unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();
        std::fs::write(root.join("node_modules/x/i.js"), "x").unwrap();
        std::fs::write(root.join("target/debug/b"), "x").unwrap();
        std::fs::write(root.join("build/out"), "x").unwrap();
        std::fs::write(root.join("big.bin"), vec![1u8; 2048]).unwrap();
        let opts = ScanOptions { max_file_bytes: 1024, ..ScanOptions::default() };
        let (files, excluded) = list_files(root, &opts).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(names, vec![".gitignore", ".orkestrignore", "src/deep dir/ü file.rs"]);
        assert_eq!(excluded, vec![ExcludedEntry { path: "big.bin".into(), reason: ExclusionReason::OverSizeLimit }]);

        let too_many = ScanOptions { max_files: 1, ..ScanOptions::default() };
        assert!(matches!(list_files(root, &too_many), Err(SnapshotError::TooLarge(_))));
    }

    #[test]
    fn safe_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(safe_target(dir.path(), "../x").is_err());
        assert!(safe_target(dir.path(), "/x").is_err());
        assert!(safe_target(dir.path(), "a\\b").is_err());
        assert!(safe_target(dir.path(), "C:/x").is_err());
        assert!(safe_target(dir.path(), "a//b").is_err());
        assert!(safe_target(dir.path(), "a/b c/ü").is_ok());
    }

    #[test]
    fn write_and_delete_prune_empty_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(root, "a/b/c.txt", b"hi", MODE_FILE).unwrap();
        assert_eq!(std::fs::read(root.join("a/b/c.txt")).unwrap(), b"hi");
        write_file(root, "a/b/c.txt", b"again", MODE_FILE).unwrap();
        assert_eq!(std::fs::read(root.join("a/b/c.txt")).unwrap(), b"again");
        std::fs::write(root.join("a/keep.txt"), "k").unwrap();
        delete_file(root, "a/b/c.txt").unwrap();
        assert!(!root.join("a/b").exists(), "empty dir pruned");
        assert!(root.join("a/keep.txt").exists(), "non-empty dir kept");
        delete_file(root, "a/b/c.txt").unwrap();
        assert!(root.exists());
    }
}
