//! Filesystem confinement (contract §6, permission model step 2, hard policy P4).
//!
//! [`confine`] resolves a candidate path inside a root and rejects every way
//! of escaping it:
//!
//! * `..` traversal above the root (lexically, before touching the disk);
//! * absolute paths outside the root;
//! * UNC (`\\server\share`, `//server/share`), device and verbatim paths
//!   (`\\?\`, `\\.\`), drive-relative paths (`C:foo`);
//! * reserved Windows device names (`CON`, `NUL`, `COM1`, `LPT1`, …, also
//!   with extensions such as `nul.txt`), names with trailing dots/spaces
//!   (aliasing), NTFS alternate data streams (`file:stream`);
//! * symlink / junction escapes: the longest existing ancestor is
//!   canonicalized and must still lie inside the canonical root.
//!
//! The returned path is absolute, uses the canonical root and has no `..`.
//! Note the inherent TOCTOU window: a link created after `confine` returns
//! can still redirect the operation. Tools should open files right after
//! confining and must never follow links they create themselves.

use std::path::{Component, Path, PathBuf};

/// Why a path was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Empty or contains NUL.
    #[error("invalid path: {0}")]
    Invalid(String),
    /// UNC / device / verbatim path.
    #[error("UNC and device paths are not allowed: {0}")]
    Unc(String),
    /// Drive-relative path (`C:foo`).
    #[error("drive-relative paths are not allowed: {0}")]
    DriveRelative(String),
    /// Reserved device name or ambiguous trailing dot/space.
    #[error("reserved or ambiguous file name: {0}")]
    ReservedName(String),
    /// NTFS alternate data stream.
    #[error("alternate data streams are not allowed: {0}")]
    AlternateDataStream(String),
    /// Escapes the root (traversal, absolute, symlink/junction).
    #[error("path escapes the allowed root: {0}")]
    Escape(String),
    /// The root itself is unusable.
    #[error("invalid root {0}: {1}")]
    Root(String, String),
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹",
    "com²", "com³", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²",
    "lpt³", "conin$", "conout$", "clock$",
];

/// Windows reserved device names (`CON`, `NUL`, `COM1`, `LPT¹`, `CONIN$`, …),
/// also with an extension (`nul.txt`) or trailing spaces before it
/// (`CON .txt`). Shared by every confinement helper (files, terminal, guard).
#[must_use]
pub fn is_reserved_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim_end().to_lowercase();
    RESERVED.contains(&stem.as_str())
}

fn check_component(name: &str, original: &str) -> Result<(), PathError> {
    if name == "." || name == ".." {
        return Ok(());
    }
    if name.contains(':') {
        return Err(PathError::AlternateDataStream(original.to_owned()));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(PathError::ReservedName(original.to_owned()));
    }
    if is_reserved_device_name(name) {
        return Err(PathError::ReservedName(original.to_owned()));
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
        return Err(PathError::Invalid(original.to_owned()));
    }
    Ok(())
}

/// Strips the `\\?\` verbatim prefix produced by `canonicalize` on Windows
/// (`\\?\C:\x` → `C:\x`; `\\?\UNC\s\x` → `\\s\x`).
#[must_use]
pub fn simplify(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// Canonicalizes the root (must exist and be a directory).
pub fn canonical_root(root: &Path) -> Result<PathBuf, PathError> {
    let canon = std::fs::canonicalize(root).map_err(|e| PathError::Root(root.display().to_string(), e.to_string()))?;
    if !canon.is_dir() {
        return Err(PathError::Root(root.display().to_string(), "not a directory".into()));
    }
    Ok(canon)
}

/// Lexical part of [`confine`]: validates `candidate` and returns the
/// normalized relative components (no `..`, no `.`). Does not touch the disk.
pub fn normalize_relative(root: &Path, candidate: &str) -> Result<Vec<String>, PathError> {
    if candidate.is_empty() || candidate.contains('\0') {
        return Err(PathError::Invalid(candidate.escape_debug().to_string()));
    }
    let unified = candidate.replace('\\', "/");
    if unified.starts_with("//") {
        return Err(PathError::Unc(candidate.to_owned()));
    }
    let b = unified.as_bytes();
    let has_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if has_drive && b.get(2).is_some_and(|c| *c != b'/') {
        return Err(PathError::DriveRelative(candidate.to_owned()));
    }
    if has_drive && b.len() == 2 {
        return Err(PathError::DriveRelative(candidate.to_owned()));
    }
    let is_abs = has_drive || unified.starts_with('/');

    // Validate every component of the candidate (drive prefix excluded).
    let body = if has_drive { &unified[2..] } else { unified.as_str() };
    for comp in body.split('/').filter(|c| !c.is_empty()) {
        check_component(comp, candidate)?;
    }

    let rel_text: String = if is_abs {
        // Absolute: must be lexically under the root.
        let root_unified = root.to_string_lossy().replace('\\', "/");
        let root_unified = simplify(Path::new(&root_unified)).to_string_lossy().replace('\\', "/");
        let cand_norm = lexical_normalize(&unified).ok_or_else(|| PathError::Escape(candidate.to_owned()))?;
        let root_norm = lexical_normalize(&root_unified)
            .ok_or_else(|| PathError::Root(root.display().to_string(), "unnormalizable".into()))?;
        let fold = cfg!(windows);
        let (c_cmp, r_cmp) = if fold {
            (cand_norm.to_lowercase(), root_norm.to_lowercase())
        } else {
            (cand_norm.clone(), root_norm.clone())
        };
        let r_cmp = r_cmp.trim_end_matches('/').to_owned();
        if c_cmp == r_cmp {
            String::new()
        } else if let Some(rest) = c_cmp.strip_prefix(&format!("{r_cmp}/")) {
            // keep original casing of the remainder
            cand_norm[cand_norm.len() - rest.len()..].to_owned()
        } else {
            return Err(PathError::Escape(candidate.to_owned()));
        }
    } else {
        unified
    };

    let mut out: Vec<String> = Vec::new();
    for comp in rel_text.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.pop().is_none() {
                    return Err(PathError::Escape(candidate.to_owned()));
                }
            }
            c => out.push(c.to_owned()),
        }
    }
    Ok(out)
}

/// Lexically resolves `.`/`..` in a `/`-separated absolute path. `None` if
/// `..` would go above the filesystem root.
fn lexical_normalize(p: &str) -> Option<String> {
    let (prefix, body) = {
        let b = p.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            (p[..2].to_owned(), &p[2..])
        } else {
            (String::new(), p)
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in body.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(format!("{prefix}/{}", parts.join("/")))
}

/// Confines `candidate` (relative to `root`, or absolute inside it) and
/// returns the absolute path under the canonical root.
pub fn confine(root: &Path, candidate: &str) -> Result<PathBuf, PathError> {
    let canon_root = canonical_root(root)?;
    // Lexical check against both the given and the canonical root form.
    let rel = match normalize_relative(root, candidate) {
        Ok(r) => r,
        Err(PathError::Escape(_)) if Path::new(candidate).is_absolute() => {
            normalize_relative(&simplify(&canon_root), candidate)?
        }
        Err(e) => return Err(e),
    };
    let mut full = canon_root.clone();
    for c in &rel {
        full.push(c);
    }
    // Symlink / junction resolution: canonicalize the longest existing ancestor.
    let mut existing = full.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if std::fs::symlink_metadata(&existing).is_ok() {
            break;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return Err(PathError::Escape(candidate.to_owned())),
        }
    }
    let resolved = std::fs::canonicalize(&existing).map_err(|_| {
        // Dangling symlink (target missing) — refuse: we cannot prove where it points.
        PathError::Escape(format!("{candidate} (unresolvable link)"))
    })?;
    if !resolved.starts_with(&canon_root) {
        return Err(PathError::Escape(format!("{candidate} (via symlink/junction)")));
    }
    let mut out = resolved;
    for name in tail.into_iter().rev() {
        out.push(name);
    }
    // Defense in depth: no `..` may remain.
    if out.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(PathError::Escape(candidate.to_owned()));
    }
    Ok(simplify(&out))
}

/// Like [`confine`] but returns the path relative to the root, `/`-separated
/// (the canonical resource string used for permission rule matching).
pub fn confine_relative(root: &Path, candidate: &str) -> Result<String, PathError> {
    let full = confine(root, candidate)?;
    let canon_root = simplify(&canonical_root(root)?);
    let rel = full.strip_prefix(&canon_root).map_err(|_| PathError::Escape(candidate.to_owned()))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/a.txt"), "a").unwrap();
        std::fs::create_dir_all(dir.path().join("outside")).unwrap();
        std::fs::write(dir.path().join("outside/secret.txt"), "s").unwrap();
        (dir, root)
    }

    #[test]
    fn accepts_paths_inside_root() {
        let (_d, root) = setup();
        let canon = simplify(&std::fs::canonicalize(&root).unwrap());
        assert_eq!(confine(&root, "src/a.txt").unwrap(), canon.join("src").join("a.txt"));
        assert_eq!(confine(&root, "./src/../src/a.txt").unwrap(), canon.join("src").join("a.txt"));
        assert_eq!(confine(&root, "src\\sub\\new.txt").unwrap(), canon.join("src").join("sub").join("new.txt"));
        assert_eq!(confine(&root, "new/dir/file.rs").unwrap(), canon.join("new").join("dir").join("file.rs"));
        assert_eq!(confine(&root, ".").unwrap(), canon);
        let abs = canon.join("src").join("a.txt");
        assert_eq!(confine(&root, &abs.to_string_lossy()).unwrap(), abs);
        assert_eq!(confine_relative(&root, "src/sub/../a.txt").unwrap(), "src/a.txt");
    }

    #[test]
    fn rejects_traversal() {
        let (_d, root) = setup();
        for c in ["..", "../outside/secret.txt", "src/../../outside", "src/../..", "a/b/../../../x", "..\\outside"] {
            assert!(matches!(confine(&root, c), Err(PathError::Escape(_))), "{c}");
        }
    }

    #[test]
    fn rejects_absolute_outside() {
        let (d, root) = setup();
        let outside = d.path().join("outside").join("secret.txt");
        assert!(matches!(confine(&root, &outside.to_string_lossy()), Err(PathError::Escape(_))));
        let sibling = format!("{}-evil/x", root.to_string_lossy());
        assert!(matches!(confine(&root, &sibling), Err(PathError::Escape(_))), "prefix sibling must not match");
        #[cfg(unix)]
        assert!(matches!(confine(&root, "/etc/passwd"), Err(PathError::Escape(_))));
        #[cfg(windows)]
        assert!(matches!(confine(&root, "C:\\Windows\\win.ini"), Err(PathError::Escape(_))));
    }

    #[test]
    fn rejects_unc_device_and_drive_relative() {
        let (_d, root) = setup();
        for c in ["\\\\server\\share\\x", "//server/share/x", "\\\\?\\C:\\x", "\\\\.\\PhysicalDrive0", "//?/C:/x"] {
            assert!(matches!(confine(&root, c), Err(PathError::Unc(_))), "{c}");
        }
        for c in ["C:foo", "c:..\\x", "D:"] {
            assert!(matches!(confine(&root, c), Err(PathError::DriveRelative(_))), "{c}");
        }
    }

    #[test]
    fn rejects_reserved_names_ads_and_bad_input() {
        let (_d, root) = setup();
        for c in ["CON", "nul", "src/NUL.txt", "aux.c", "COM1", "lpt9.log", "src/x.", "src/x ", "conout$"] {
            assert!(matches!(confine(&root, c), Err(PathError::ReservedName(_))), "{c}");
        }
        for c in ["file.txt:stream", "src/a.txt::$DATA"] {
            assert!(matches!(confine(&root, c), Err(PathError::AlternateDataStream(_))), "{c}");
        }
        assert!(matches!(confine(&root, ""), Err(PathError::Invalid(_))));
        assert!(matches!(confine(&root, "a\0b"), Err(PathError::Invalid(_))));
        assert!(matches!(confine(&root, "a*b"), Err(PathError::Invalid(_))));
        assert!(confine(&root, "console.log").is_ok(), "CON prefix inside a longer stem is fine");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let (d, root) = setup();
        std::os::unix::fs::symlink(d.path().join("outside"), root.join("link")).unwrap();
        assert!(matches!(confine(&root, "link/secret.txt"), Err(PathError::Escape(_))));
        assert!(matches!(confine(&root, "link/new.txt"), Err(PathError::Escape(_))));
        std::os::unix::fs::symlink(root.join("src"), root.join("inner")).unwrap();
        assert!(confine(&root, "inner/a.txt").is_ok(), "links inside the root are fine");
        std::os::unix::fs::symlink(d.path().join("missing"), root.join("dangling")).unwrap();
        assert!(confine(&root, "dangling/x").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_junction_escape() {
        let (d, root) = setup();
        let link = root.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(d.path().join("outside"))
            .output()
            .unwrap();
        if !status.status.success() {
            eprintln!("skipping: could not create junction: {}", String::from_utf8_lossy(&status.stderr));
            return;
        }
        assert!(matches!(confine(&root, "link\\secret.txt"), Err(PathError::Escape(_))));
        assert!(matches!(confine(&root, "link/new.txt"), Err(PathError::Escape(_))));
        let inner = root.join("inner");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&inner)
            .arg(root.join("src"))
            .output()
            .unwrap();
        assert!(ok.status.success());
        assert!(confine(&root, "inner\\a.txt").is_ok());
    }

    #[test]
    fn root_must_exist() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(confine(&d.path().join("missing"), "x"), Err(PathError::Root(..))));
    }
}
