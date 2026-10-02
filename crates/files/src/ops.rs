//! Synchronous, confined file operations on a [`WorkspaceRoot`].
//!
//! These functions block; [`crate::FilesManager`] runs them on the blocking
//! pool. Every path is a validated [`RelPath`] and is resolved through
//! [`WorkspaceRoot::resolve`] before any I/O.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use orkestr_protocol::methods::files::{FileEntry, FileKind, ListFilesResult, ReadFileResult, TextEncoding};

use crate::error::{FilesError, FilesResult};
use crate::path::{RelPath, Resolved, WorkspaceRoot};
use crate::text::{self, Decoded};

/// Marker contained in the names of temporary files used for atomic writes
/// (the watcher ignores them).
pub const TEMP_MARKER: &str = ".orkestr-tmp";

fn rel_str(rel: &RelPath) -> String {
    rel.as_string()
}

fn ensure_mutable(rel: &RelPath) -> FilesResult<()> {
    if rel.is_protected() {
        return Err(FilesError::Protected(rel.as_string()));
    }
    Ok(())
}

/// Builds the wire entry of an existing path.
pub fn entry_for(root: &WorkspaceRoot, rel: &RelPath, abs: &Path, ignored: bool) -> FilesResult<FileEntry> {
    let lmeta = fs::symlink_metadata(abs).map_err(|e| FilesError::io(rel_str(rel), e))?;
    let symlink = lmeta.file_type().is_symlink();
    // Follow a link only when its target stays inside the root (never leak outside metadata).
    let meta = if symlink {
        match fs::canonicalize(abs) {
            Ok(target) if target.starts_with(root.path()) => fs::metadata(abs).unwrap_or(lmeta),
            _ => lmeta,
        }
    } else {
        lmeta
    };
    let kind = if meta.is_dir() { FileKind::Dir } else { FileKind::File };
    let name = rel.name().map_or_else(|| root.name(), str::to_owned);
    Ok(FileEntry {
        hidden: name.starts_with('.'),
        name,
        path: rel.as_string(),
        kind,
        symlink,
        size: if kind == FileKind::File && !meta.file_type().is_symlink() { meta.len() } else { 0 },
        modified: meta.modified().ok().map(DateTime::<Utc>::from),
        ignored,
        readonly: meta.permissions().readonly(),
    })
}

/// Names in `dir` that survive `.gitignore`/`.ignore` filtering.
fn visible_names(dir: &Path) -> HashSet<OsString> {
    WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| e.file_name().to_owned())
        .collect()
}

fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let dir_a = a.kind == FileKind::Dir;
        let dir_b = b.kind == FileKind::Dir;
        dir_b
            .cmp(&dir_a)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Lists one directory level.
pub fn list(
    root: &WorkspaceRoot,
    rel: &RelPath,
    show_ignored: bool,
    max_entries: usize,
) -> FilesResult<ListFilesResult> {
    let res = root.resolve(rel, true)?;
    if !res.exists {
        return Err(FilesError::NotFound(rel.as_string()));
    }
    if !res.abs.is_dir() {
        return Err(FilesError::NotADirectory(res.rel.as_string()));
    }
    let visible = visible_names(&res.abs);
    let mut entries = Vec::new();
    let mut truncated = false;
    let read = fs::read_dir(&res.abs).map_err(|e| FilesError::io(res.rel.as_string(), e))?;
    for item in read {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                tracing::debug!(error = %e, "skipping unreadable directory entry");
                continue;
            }
        };
        let os_name = item.file_name();
        let Some(name) = os_name.to_str() else {
            tracing::debug!(name = ?os_name, "skipping non-UTF-8 file name");
            continue;
        };
        if name.contains(TEMP_MARKER) {
            continue;
        }
        let is_git_dir = name.eq_ignore_ascii_case(".git");
        let ignored = is_git_dir || !visible.contains(&os_name);
        if ignored && !show_ignored {
            continue;
        }
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        let Ok(child) = res.rel.join(name) else {
            continue; // names the wire format cannot address (e.g. reserved device names)
        };
        match entry_for(root, &child, &item.path(), ignored) {
            Ok(e) => entries.push(e),
            Err(e) => tracing::debug!(error = %e, "skipping entry"),
        }
    }
    sort_entries(&mut entries);
    Ok(ListFilesResult { path: res.rel.as_string(), entries, truncated })
}

/// Stats a path.
pub fn stat(root: &WorkspaceRoot, rel: &RelPath) -> FilesResult<FileEntry> {
    let res = root.resolve(rel, true)?;
    if !res.exists {
        return Err(FilesError::NotFound(rel.as_string()));
    }
    entry_for(root, &res.rel, &res.abs, res.rel.is_protected())
}

/// Reads a file (text decoded, binary flagged).
pub fn read(root: &WorkspaceRoot, rel: &RelPath, max_bytes: u64) -> FilesResult<ReadFileResult> {
    let res = root.resolve(rel, true)?;
    if !res.exists {
        return Err(FilesError::NotFound(rel.as_string()));
    }
    let path = res.rel.as_string();
    let meta = fs::metadata(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
    if !meta.is_file() {
        return Err(FilesError::NotAFile(path));
    }
    if meta.len() > max_bytes {
        return Err(FilesError::TooLarge { path, size: meta.len(), limit: max_bytes });
    }
    let bytes = fs::read(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
    let size = bytes.len() as u64;
    if size > max_bytes {
        return Err(FilesError::TooLarge { path, size, limit: max_bytes });
    }
    let hash = text::hash_bytes(&bytes);
    let language = text::language_for(res.rel.name().unwrap_or_default()).to_owned();
    let modified = meta.modified().ok().map(DateTime::<Utc>::from);
    let readonly = meta.permissions().readonly();
    Ok(match text::decode(&bytes) {
        Decoded::Text { content, encoding } => ReadFileResult {
            line_ending: text::line_ending(&content),
            content: Some(content),
            binary: false,
            encoding,
            path,
            hash,
            size,
            language,
            modified,
            readonly,
        },
        Decoded::Binary => ReadFileResult {
            path,
            content: None,
            binary: true,
            encoding: TextEncoding::Utf8,
            line_ending: Default::default(),
            hash,
            size,
            language: "plaintext".into(),
            modified,
            readonly,
        },
    })
}

/// Outcome of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Real relative path.
    pub rel: RelPath,
    /// New hash.
    pub hash: String,
    /// Bytes written.
    pub size: u64,
    /// The file was created.
    pub created: bool,
}

fn retry_io<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    // Windows: indexers/antivirus briefly hold handles without FILE_SHARE_DELETE.
    let mut attempt = 0;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < 4 && e.kind() == io::ErrorKind::PermissionDenied => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(25 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Writes `bytes` to `target` atomically: temp file in the same directory,
/// fsync, rename over the target.
pub fn atomic_write(target: &Path, bytes: &[u8], label: &str) -> FilesResult<()> {
    let dir = target.parent().ok_or_else(|| FilesError::NotAFile(label.to_owned()))?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let short = uuid::Uuid::now_v7().simple().to_string();
    let tmp = dir.join(format!(".{name}.{}{TEMP_MARKER}", &short[short.len() - 8..]));
    let result = (|| -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        if let Ok(meta) = fs::metadata(target) {
            let perms = meta.permissions();
            if perms.readonly() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "target is read-only"));
            }
            // Keeps unix mode bits of the original; harmless elsewhere.
            let _ = fs::set_permissions(&tmp, perms);
        }
        retry_io(|| fs::rename(&tmp, target))
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(FilesError::io(label.to_owned(), e));
    }
    Ok(())
}

fn create_parents(root: &WorkspaceRoot, res: &Resolved) -> FilesResult<()> {
    if let Some(parent) = res.abs.parent() {
        let parent_rel = res.rel.parent().unwrap_or_default();
        fs::create_dir_all(parent).map_err(|e| FilesError::io(parent_rel.as_string(), e))?;
        root.ensure_inside(parent, &parent_rel)?;
        if !parent.is_dir() {
            return Err(FilesError::NotADirectory(parent_rel.as_string()));
        }
    }
    Ok(())
}

/// Writes a file with optimistic concurrency.
///
/// `expected_hash`: when `Some` (non-empty) the current on-disk hash must
/// match, otherwise [`FilesError::Conflict`] is returned and nothing is written.
pub fn write(
    root: &WorkspaceRoot,
    rel: &RelPath,
    bytes: &[u8],
    expected_hash: Option<&str>,
    create: bool,
) -> FilesResult<WriteOutcome> {
    let res = root.resolve(rel, true)?;
    if res.rel.is_root() {
        return Err(FilesError::NotAFile(String::new()));
    }
    ensure_mutable(&res.rel)?;
    let path = res.rel.as_string();
    let expected = expected_hash.filter(|h| !h.is_empty());
    let created = !res.exists;
    if res.exists {
        let meta = fs::metadata(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
        if !meta.is_file() {
            return Err(FilesError::NotAFile(path));
        }
        if meta.permissions().readonly() {
            return Err(FilesError::PermissionDenied(path));
        }
        if let Some(expected) = expected {
            let current = fs::read(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
            let actual = text::hash_bytes(&current);
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(FilesError::Conflict { path, actual_hash: Some(actual) });
            }
        }
    } else {
        if expected.is_some() {
            return Err(FilesError::Conflict { path, actual_hash: None });
        }
        if !create {
            return Err(FilesError::NotFound(path));
        }
        create_parents(root, &res)?;
    }
    atomic_write(&res.abs, bytes, &path)?;
    Ok(WriteOutcome { hash: text::hash_bytes(bytes), size: bytes.len() as u64, created, rel: res.rel })
}

/// Creates a new file (fails when anything exists at the path).
pub fn create_file(root: &WorkspaceRoot, rel: &RelPath, bytes: &[u8]) -> FilesResult<FileEntry> {
    let res = root.resolve(rel, false)?;
    if res.rel.is_root() {
        return Err(FilesError::AlreadyExists(String::new()));
    }
    ensure_mutable(&res.rel)?;
    let path = res.rel.as_string();
    if res.exists {
        return Err(FilesError::AlreadyExists(path));
    }
    create_parents(root, &res)?;
    let mut f =
        OpenOptions::new().write(true).create_new(true).open(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(|e| FilesError::io(path.clone(), e))?;
    drop(f);
    entry_for(root, &res.rel, &res.abs, false)
}

/// Creates a directory (and parents).
pub fn mkdir(root: &WorkspaceRoot, rel: &RelPath) -> FilesResult<FileEntry> {
    let res = root.resolve(rel, false)?;
    ensure_mutable(&res.rel)?;
    let path = res.rel.as_string();
    if res.exists {
        return Err(FilesError::AlreadyExists(path));
    }
    fs::create_dir_all(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
    root.ensure_inside(&res.abs, &res.rel)?;
    entry_for(root, &res.rel, &res.abs, false)
}

/// Renames / moves. Returns the real source path and the new entry.
pub fn rename(
    root: &WorkspaceRoot,
    from: &RelPath,
    to: &RelPath,
    overwrite: bool,
) -> FilesResult<(RelPath, FileEntry)> {
    let src = root.resolve(from, false)?;
    if src.rel.is_root() || to.is_root() {
        return Err(FilesError::invalid("", "the workspace root cannot be renamed"));
    }
    if !src.exists {
        return Err(FilesError::NotFound(from.as_string()));
    }
    ensure_mutable(&src.rel)?;
    let dst = root.resolve(to, false)?;
    ensure_mutable(&dst.rel)?;
    if src.abs == dst.abs {
        // Same physical entry: a case-only rename on a case-insensitive file system, or a no-op.
        let (Some(old_name), Some(new_name)) = (src.rel.name(), to.name()) else {
            return Err(FilesError::invalid(&to.as_string(), "invalid destination"));
        };
        if old_name == new_name {
            let entry = entry_for(root, &src.rel, &src.abs, false)?;
            return Ok((src.rel, entry));
        }
        let parent = src.abs.parent().map(Path::to_path_buf).unwrap_or_else(|| root.path().to_path_buf());
        let target = parent.join(new_name);
        retry_io(|| fs::rename(&src.abs, &target)).map_err(|e| FilesError::io(src.rel.as_string(), e))?;
        let new_rel = src.rel.parent().unwrap_or_default().join(new_name)?;
        let entry = entry_for(root, &new_rel, &target, false)?;
        return Ok((src.rel, entry));
    }
    if dst.rel.starts_with(&src.rel) {
        return Err(FilesError::invalid(&to.as_string(), "cannot move a folder into itself"));
    }
    let src_meta = fs::symlink_metadata(&src.abs).map_err(|e| FilesError::io(src.rel.as_string(), e))?;
    if dst.exists {
        let dst_meta = fs::symlink_metadata(&dst.abs).map_err(|e| FilesError::io(dst.rel.as_string(), e))?;
        if !overwrite || dst_meta.is_dir() || src_meta.is_dir() {
            return Err(FilesError::AlreadyExists(dst.rel.as_string()));
        }
    }
    create_parents(root, &dst)?;
    retry_io(|| fs::rename(&src.abs, &dst.abs)).map_err(|e| FilesError::io(src.rel.as_string(), e))?;
    let entry = entry_for(root, &dst.rel, &dst.abs, false)?;
    Ok((src.rel, entry))
}

/// What a delete will remove.
#[derive(Debug, Clone)]
pub struct DeletePlan {
    /// Real relative path of the target.
    pub rel: RelPath,
    /// Absolute path.
    pub abs: PathBuf,
    /// Target kind.
    pub target: DeleteTarget,
    /// Regular files (relative path, absolute path, size) to copy into the trash first.
    pub files: Vec<(RelPath, PathBuf, u64)>,
    /// Sum of `files` sizes.
    pub total_bytes: u64,
}

/// Kind of a delete target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteTarget {
    /// A regular file.
    File,
    /// A directory (with contents).
    Dir,
    /// A symbolic link / junction (the link itself is removed, never its target).
    Link,
}

/// Plans a delete: validates and collects files for the trash.
pub fn plan_delete(
    root: &WorkspaceRoot,
    rel: &RelPath,
    recursive: bool,
    max_trash_bytes: u64,
) -> FilesResult<DeletePlan> {
    let res = root.resolve(rel, false)?;
    if res.rel.is_root() {
        return Err(FilesError::invalid("", "the workspace root cannot be deleted"));
    }
    if !res.exists {
        return Err(FilesError::NotFound(rel.as_string()));
    }
    ensure_mutable(&res.rel)?;
    let path = res.rel.as_string();
    let meta = fs::symlink_metadata(&res.abs).map_err(|e| FilesError::io(path.clone(), e))?;
    let mut files = Vec::new();
    let target = if meta.file_type().is_symlink() {
        DeleteTarget::Link
    } else if meta.is_dir() {
        let mut stack = vec![(res.rel.clone(), res.abs.clone())];
        while let Some((dir_rel, dir_abs)) = stack.pop() {
            let read = fs::read_dir(&dir_abs).map_err(|e| FilesError::io(dir_rel.as_string(), e))?;
            for item in read {
                let item = item.map_err(|e| FilesError::io(dir_rel.as_string(), e))?;
                if !recursive {
                    return Err(FilesError::RecursiveRequired(path));
                }
                let name = item.file_name().to_string_lossy().into_owned();
                let child_rel = match dir_rel.join(&name) {
                    Ok(r) => r,
                    Err(_) => continue, // unaddressable name: removed with the folder, not trashed
                };
                let ft = item.file_type().map_err(|e| FilesError::io(child_rel.as_string(), e))?;
                if ft.is_symlink() {
                    continue;
                } else if ft.is_dir() {
                    stack.push((child_rel, item.path()));
                } else {
                    let size = item.metadata().map(|m| m.len()).unwrap_or(0);
                    files.push((child_rel, item.path(), size));
                }
            }
        }
        DeleteTarget::Dir
    } else {
        files.push((res.rel.clone(), res.abs.clone(), meta.len()));
        DeleteTarget::File
    };
    let total_bytes: u64 = files.iter().map(|(_, _, s)| *s).sum();
    if total_bytes > max_trash_bytes {
        return Err(FilesError::TooLarge { path, size: total_bytes, limit: max_trash_bytes });
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(DeletePlan { rel: res.rel, abs: res.abs, target, files, total_bytes })
}

/// Executes a planned delete.
pub fn execute_delete(plan: &DeletePlan) -> FilesResult<()> {
    let path = plan.rel.as_string();
    let result = match plan.target {
        DeleteTarget::File => retry_io(|| fs::remove_file(&plan.abs)),
        DeleteTarget::Dir => retry_io(|| fs::remove_dir_all(&plan.abs)),
        // Directory links on Windows must be removed with remove_dir.
        DeleteTarget::Link => fs::remove_file(&plan.abs).or_else(|_| fs::remove_dir(&plan.abs)),
    };
    result.map_err(|e| FilesError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, WorkspaceRoot) {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    fn rp(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    #[test]
    fn list_sorts_dirs_first_and_respects_gitignore() {
        let (dir, root) = setup();
        let p = dir.path();
        fs::write(p.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir_all(p.join("target/debug")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::create_dir_all(p.join("Docs")).unwrap();
        fs::write(p.join("b.txt"), "b").unwrap();
        fs::write(p.join("A.txt"), "a").unwrap();
        fs::write(p.join("debug.log"), "x").unwrap();
        let res = list(&root, &RelPath::root(), false, 100).unwrap();
        let names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Docs", "src", ".gitignore", "A.txt", "b.txt"]);
        assert!(!res.truncated);

        let all = list(&root, &RelPath::root(), true, 100).unwrap();
        let ignored: Vec<_> = all.entries.iter().filter(|e| e.ignored).map(|e| e.name.as_str()).collect();
        assert_eq!(ignored, vec![".git", "target", "debug.log"]);
        let limited = list(&root, &RelPath::root(), true, 2).unwrap();
        assert!(limited.truncated);
        assert_eq!(limited.entries.len(), 2);
    }

    #[test]
    fn list_nested_honors_parent_gitignore() {
        let (dir, root) = setup();
        let p = dir.path();
        fs::write(p.join(".gitignore"), "*.tmp\n").unwrap();
        fs::create_dir_all(p.join("a")).unwrap();
        fs::write(p.join("a/keep.rs"), "").unwrap();
        fs::write(p.join("a/drop.tmp"), "").unwrap();
        let res = list(&root, &rp("a"), false, 100).unwrap();
        assert_eq!(res.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["a/keep.rs"]);
        assert!(matches!(list(&root, &rp("a/keep.rs"), false, 10), Err(FilesError::NotADirectory(_))));
        assert!(matches!(list(&root, &rp("missing"), false, 10), Err(FilesError::NotFound(_))));
    }

    #[test]
    fn read_text_binary_and_limits() {
        let (dir, root) = setup();
        fs::write(dir.path().join("main.rs"), "fn main() {}\r\n").unwrap();
        fs::write(dir.path().join("img.png"), [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
        let r = read(&root, &rp("main.rs"), 1024).unwrap();
        assert_eq!(r.content.as_deref(), Some("fn main() {}\r\n"));
        assert_eq!(r.language, "rust");
        assert_eq!(r.line_ending, orkestr_protocol::methods::files::LineEnding::Crlf);
        assert_eq!(r.hash, text::hash_bytes(b"fn main() {}\r\n"));
        let b = read(&root, &rp("img.png"), 1024).unwrap();
        assert!(b.binary);
        assert!(b.content.is_none());
        assert!(matches!(read(&root, &rp("main.rs"), 4), Err(FilesError::TooLarge { size: 14, limit: 4, .. })));
        assert!(matches!(read(&root, &RelPath::root(), 1024), Err(FilesError::NotAFile(_))));
        assert!(matches!(read(&root, &rp("nope.txt"), 1024), Err(FilesError::NotFound(_))));
    }

    #[test]
    fn write_is_atomic_and_detects_conflicts() {
        let (dir, root) = setup();
        let f = dir.path().join("a.txt");
        fs::write(&f, "one").unwrap();
        let h1 = text::hash_bytes(b"one");
        let out = write(&root, &rp("a.txt"), b"two", Some(&h1), false).unwrap();
        assert!(!out.created);
        assert_eq!(fs::read_to_string(&f).unwrap(), "two");
        // Stale hash → conflict, content untouched.
        let err = write(&root, &rp("a.txt"), b"three", Some(&h1), false).unwrap_err();
        match err {
            FilesError::Conflict { actual_hash, .. } => assert_eq!(actual_hash, Some(text::hash_bytes(b"two"))),
            other => panic!("expected conflict, got {other}"),
        }
        assert_eq!(fs::read_to_string(&f).unwrap(), "two");
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(TEMP_MARKER))
            .collect();
        assert!(leftovers.is_empty());
        // Missing file: requires create; expected hash on a missing file conflicts.
        assert!(matches!(write(&root, &rp("new/x.txt"), b"x", None, false), Err(FilesError::NotFound(_))));
        assert!(matches!(
            write(&root, &rp("new/x.txt"), b"x", Some(&h1), true),
            Err(FilesError::Conflict { actual_hash: None, .. })
        ));
        let out = write(&root, &rp("new/x.txt"), b"x", None, true).unwrap();
        assert!(out.created);
        assert_eq!(out.rel.as_string(), "new/x.txt");
        assert_eq!(fs::read_to_string(dir.path().join("new/x.txt")).unwrap(), "x");
    }

    #[test]
    #[allow(clippy::permissions_set_readonly_false)]
    fn write_rejects_dirs_protected_and_readonly() {
        let (dir, root) = setup();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::create_dir_all(dir.path().join("d")).unwrap();
        assert!(matches!(write(&root, &rp("d"), b"x", None, true), Err(FilesError::NotAFile(_))));
        assert!(matches!(write(&root, &rp(".git/hooks/pre-commit"), b"x", None, true), Err(FilesError::Protected(_))));
        assert!(matches!(write(&root, &rp(".GIT/config"), b"x", None, true), Err(FilesError::Protected(_))));
        let ro = dir.path().join("ro.txt");
        fs::write(&ro, "x").unwrap();
        let mut perms = fs::metadata(&ro).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&ro, perms.clone()).unwrap();
        assert!(matches!(write(&root, &rp("ro.txt"), b"y", None, false), Err(FilesError::PermissionDenied(_))));
        perms.set_readonly(false);
        fs::set_permissions(&ro, perms).unwrap();
    }

    #[test]
    fn create_mkdir_and_conflicts() {
        let (dir, root) = setup();
        let e = create_file(&root, &rp("src/new.ts"), b"export {}").unwrap();
        assert_eq!(e.path, "src/new.ts");
        assert_eq!(e.kind, FileKind::File);
        assert_eq!(e.size, 9);
        assert!(matches!(create_file(&root, &rp("src/new.ts"), b""), Err(FilesError::AlreadyExists(_))));
        let d = mkdir(&root, &rp("a/b/c")).unwrap();
        assert_eq!(d.kind, FileKind::Dir);
        assert!(dir.path().join("a/b/c").is_dir());
        assert!(matches!(mkdir(&root, &rp("a/b")), Err(FilesError::AlreadyExists(_))));
        assert!(matches!(mkdir(&root, &rp(".git")), Err(FilesError::Protected(_))));
        // A file in the way of a parent folder.
        assert!(create_file(&root, &rp("src/new.ts/child.txt"), b"").is_err());
    }

    #[test]
    fn rename_and_move_rules() {
        let (dir, root) = setup();
        let p = dir.path();
        fs::create_dir_all(p.join("dir/sub")).unwrap();
        fs::write(p.join("a.txt"), "a").unwrap();
        fs::write(p.join("b.txt"), "b").unwrap();
        let (from, entry) = rename(&root, &rp("a.txt"), &rp("moved/a2.txt"), false).unwrap();
        assert_eq!(from.as_string(), "a.txt");
        assert_eq!(entry.path, "moved/a2.txt");
        assert!(!p.join("a.txt").exists());
        // Existing destination needs overwrite (files only).
        assert!(matches!(rename(&root, &rp("b.txt"), &rp("moved/a2.txt"), false), Err(FilesError::AlreadyExists(_))));
        rename(&root, &rp("b.txt"), &rp("moved/a2.txt"), true).unwrap();
        assert_eq!(fs::read_to_string(p.join("moved/a2.txt")).unwrap(), "b");
        assert!(matches!(rename(&root, &rp("moved/a2.txt"), &rp("dir"), true), Err(FilesError::AlreadyExists(_))));
        // Folder into itself.
        assert!(matches!(rename(&root, &rp("dir"), &rp("dir/sub/x"), false), Err(FilesError::InvalidPath { .. })));
        // Case-only rename.
        let (_, entry) = rename(&root, &rp("dir"), &rp("Dir"), false).unwrap();
        assert_eq!(entry.name, "Dir");
        assert!(matches!(rename(&root, &rp("missing"), &rp("x"), false), Err(FilesError::NotFound(_))));
        assert!(matches!(rename(&root, &rp("Dir"), &rp(".git/x"), false), Err(FilesError::Protected(_))));
    }

    #[test]
    fn delete_plans_require_recursive_and_respect_limits() {
        let (dir, root) = setup();
        let p = dir.path();
        fs::create_dir_all(p.join("d/e")).unwrap();
        fs::write(p.join("d/one.txt"), "1").unwrap();
        fs::write(p.join("d/e/two.txt"), "22").unwrap();
        fs::create_dir_all(p.join("empty")).unwrap();
        assert!(matches!(plan_delete(&root, &rp("d"), false, 1 << 20), Err(FilesError::RecursiveRequired(_))));
        let plan = plan_delete(&root, &rp("empty"), false, 1 << 20).unwrap();
        assert_eq!(plan.target, DeleteTarget::Dir);
        execute_delete(&plan).unwrap();
        assert!(!p.join("empty").exists());
        let plan = plan_delete(&root, &rp("d"), true, 1 << 20).unwrap();
        let files: Vec<_> = plan.files.iter().map(|(r, _, s)| (r.as_string(), *s)).collect();
        assert_eq!(files, vec![("d/e/two.txt".to_owned(), 2), ("d/one.txt".to_owned(), 1)]);
        assert_eq!(plan.total_bytes, 3);
        assert!(matches!(plan_delete(&root, &rp("d"), true, 2), Err(FilesError::TooLarge { .. })));
        execute_delete(&plan).unwrap();
        assert!(!p.join("d").exists());
        assert!(plan_delete(&root, &RelPath::root(), true, 1 << 20).is_err());
        assert!(matches!(plan_delete(&root, &rp("gone"), true, 1 << 20), Err(FilesError::NotFound(_))));
    }

    #[test]
    fn stat_reports_entries() {
        let (dir, root) = setup();
        fs::write(dir.path().join(".env"), "A=1").unwrap();
        let e = stat(&root, &rp(".env")).unwrap();
        assert!(e.hidden);
        assert_eq!(e.size, 3);
        assert!(e.modified.is_some());
        let r = stat(&root, &RelPath::root()).unwrap();
        assert_eq!(r.kind, FileKind::Dir);
        assert_eq!(r.path, "");
    }
}
