//! Working-directory confinement.
//!
//! Terminals and commands run inside a *root* (the project root or an agent's
//! worktree). The requested cwd is resolved relative to the root, canonicalized
//! (symlinks and `..` resolved by the OS) and must stay inside the root.
//! Canonical Windows paths carry the `\\?\` verbatim prefix, which `cmd.exe`
//! rejects as a cwd, so the prefix is removed again for spawning.

use std::path::{Path, PathBuf};

use crate::error::TerminalError;

/// Resolves `requested` (absolute or relative to `root`) and checks it lies
/// inside `root`. `None` means the root itself.
pub fn resolve_cwd(root: &Path, requested: Option<&Path>) -> Result<PathBuf, TerminalError> {
    let root_canon = canonical_dir(root).map_err(|e| {
        TerminalError::InvalidCwd(format!("root {} is not an accessible directory: {e}", root.display()))
    })?;
    let Some(req) = requested.filter(|p| !p.as_os_str().is_empty()) else {
        return Ok(strip_verbatim(&root_canon));
    };
    let joined = if req.is_absolute() { req.to_path_buf() } else { root_canon.join(req) };
    let canon = canonical_dir(&joined)
        .map_err(|e| TerminalError::InvalidCwd(format!("{} is not an accessible directory: {e}", req.display())))?;
    if !canon.starts_with(&root_canon) {
        return Err(TerminalError::CwdOutsideRoot { cwd: req.display().to_string(), root: root.display().to_string() });
    }
    Ok(strip_verbatim(&canon))
}

/// Resolves an unconfined cwd (user terminals without a project): must be an
/// existing absolute directory.
pub fn resolve_free_cwd(requested: &Path) -> Result<PathBuf, TerminalError> {
    if !requested.is_absolute() {
        return Err(TerminalError::InvalidCwd("cwd must be an absolute path when no project is given".into()));
    }
    let canon = canonical_dir(requested).map_err(|e| {
        TerminalError::InvalidCwd(format!("{} is not an accessible directory: {e}", requested.display()))
    })?;
    Ok(strip_verbatim(&canon))
}

fn canonical_dir(p: &Path) -> std::io::Result<PathBuf> {
    let canon = std::fs::canonicalize(p)?;
    if canon.is_dir() {
        Ok(canon)
    } else {
        Err(std::io::Error::other("not a directory"))
    }
}

/// Removes a `\\?\` prefix from local drive paths (`\\?\C:\x` → `C:\x`);
/// `\\?\UNC\…` and other forms are kept unchanged.
#[must_use]
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_children_are_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        let root = resolve_cwd(dir.path(), None).unwrap();
        assert!(root.is_dir());
        assert!(!root.to_string_lossy().starts_with(r"\\?\"));
        let child = resolve_cwd(dir.path(), Some(Path::new("src/app"))).unwrap();
        assert!(child.ends_with("app"));
        let abs = resolve_cwd(dir.path(), Some(&dir.path().join("src"))).unwrap();
        assert!(abs.ends_with("src"));
    }

    #[test]
    fn traversal_and_outside_paths_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(dir.path().join("other")).unwrap();
        assert!(matches!(resolve_cwd(&root, Some(Path::new("../other"))), Err(TerminalError::CwdOutsideRoot { .. })));
        assert!(matches!(
            resolve_cwd(&root, Some(&dir.path().join("other"))),
            Err(TerminalError::CwdOutsideRoot { .. })
        ));
        assert!(matches!(resolve_cwd(&root, Some(Path::new("missing"))), Err(TerminalError::InvalidCwd(_))));
    }

    #[test]
    fn sibling_with_common_prefix_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::create_dir_all(dir.path().join("app2")).unwrap();
        assert!(resolve_cwd(&dir.path().join("app"), Some(&dir.path().join("app2"))).is_err());
    }

    #[test]
    fn free_cwd_requires_absolute_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_free_cwd(dir.path()).is_ok());
        assert!(resolve_free_cwd(Path::new("relative")).is_err());
    }

    #[test]
    fn verbatim_prefix_is_stripped_for_drive_paths_only() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\work")), PathBuf::from(r"C:\work"));
        assert_eq!(strip_verbatim(Path::new(r"\\?\UNC\srv\share")), PathBuf::from(r"\\?\UNC\srv\share"));
        assert_eq!(strip_verbatim(Path::new("/home/x")), PathBuf::from("/home/x"));
    }
}
