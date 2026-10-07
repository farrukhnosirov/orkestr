//! Extension-relative paths.
//!
//! Every path an extension mentions (theme files, grammar files, icons,
//! `include`s, archive entries) is untrusted. It is normalized to a
//! `/`-separated path relative to the extension root; absolute paths, drive
//! letters, UNC prefixes, alternate data streams, reserved device names,
//! control characters and `..` that would leave the root are rejected.

use std::path::{Path, PathBuf};

use crate::error::{ExtError, ExtResult};

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn check_component(c: &str, original: &str) -> ExtResult<()> {
    if c.chars().any(|ch| ch.is_control() || matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(ExtError::Unsafe(format!("invalid character in path `{original}`")));
    }
    let stem = c.split('.').next().unwrap_or("").trim_end().to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(ExtError::Unsafe(format!("reserved device name in path `{original}`")));
    }
    if c.ends_with(' ') || (c.ends_with('.') && c != "." && c != "..") {
        return Err(ExtError::Unsafe(format!("trailing dot or space in path `{original}`")));
    }
    Ok(())
}

/// Joins `rel` onto the extension-relative directory `base_dir` (`""` = the
/// root), resolving `.` and `..` lexically. Fails when the result would leave
/// the root or when `rel` is absolute.
pub fn join_rel(base_dir: &str, rel: &str) -> ExtResult<String> {
    let rel_norm = rel.replace('\\', "/");
    if rel_norm.is_empty() {
        return Err(ExtError::Unsafe("empty path".into()));
    }
    if rel_norm.starts_with('/') || rel_norm.starts_with("//") {
        return Err(ExtError::Unsafe(format!("absolute path `{rel}`")));
    }
    let bytes = rel_norm.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(ExtError::Unsafe(format!("drive path `{rel}`")));
    }
    let mut parts: Vec<String> = Vec::new();
    for c in base_dir.split('/').filter(|c| !c.is_empty() && *c != ".") {
        parts.push(c.to_owned());
    }
    for c in rel_norm.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(ExtError::Unsafe(format!("path `{rel}` leaves the extension folder")));
                }
            }
            other => {
                check_component(other, rel)?;
                parts.push(other.to_owned());
            }
        }
    }
    if parts.is_empty() {
        return Err(ExtError::Unsafe(format!("path `{rel}` points at the extension root")));
    }
    Ok(parts.join("/"))
}

/// Normalizes a root-relative path (`./themes/x.json` → `themes/x.json`).
pub fn normalize(rel: &str) -> ExtResult<String> {
    join_rel("", rel)
}

/// Directory part of a normalized relative path (`a/b/c.json` → `a/b`).
#[must_use]
pub fn parent(rel: &str) -> &str {
    rel.rfind('/').map_or("", |i| &rel[..i])
}

/// Absolute path of a normalized relative path under `root`, verified on disk
/// (after resolving links) to stay inside `root`. The file must exist.
pub fn resolve_existing(root: &Path, rel: &str) -> ExtResult<PathBuf> {
    let rel = normalize(rel)?;
    let mut p = root.to_path_buf();
    for c in rel.split('/') {
        p.push(c);
    }
    let meta = std::fs::symlink_metadata(&p).map_err(|_| ExtError::NotFound(format!("extension file `{rel}`")))?;
    if meta.file_type().is_symlink() {
        return Err(ExtError::Unsafe(format!("`{rel}` is a link")));
    }
    let canon_root = std::fs::canonicalize(root)?;
    let canon = std::fs::canonicalize(&p)?;
    if !canon.starts_with(&canon_root) {
        return Err(ExtError::Unsafe(format!("`{rel}` resolves outside the extension folder")));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_joins() {
        assert_eq!(normalize("./themes/x.json").unwrap(), "themes/x.json");
        assert_eq!(normalize("themes\\x.json").unwrap(), "themes/x.json");
        assert_eq!(join_rel("themes", "./base.json").unwrap(), "themes/base.json");
        assert_eq!(join_rel("dist", "./../icons/a.svg").unwrap(), "icons/a.svg");
        assert_eq!(parent("a/b/c.json"), "a/b");
        assert_eq!(parent("c.json"), "");
    }

    #[test]
    fn rejects_escapes() {
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/Windows/x",
            "c:x",
            "\\\\server\\share\\x",
            "a/file.txt:stream",
            "a/CON.txt",
            "a/nul",
            "a/b./c",
            "a/\u{1}b",
            "",
            ".",
        ] {
            assert!(normalize(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(join_rel("", "..").is_err());
    }

    #[test]
    fn resolve_existing_stays_inside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/f.json"), "{}").unwrap();
        assert!(resolve_existing(dir.path(), "a/f.json").is_ok());
        assert!(matches!(resolve_existing(dir.path(), "a/missing.json"), Err(ExtError::NotFound(_))));
        assert!(resolve_existing(dir.path(), "../f.json").is_err());
    }
}
