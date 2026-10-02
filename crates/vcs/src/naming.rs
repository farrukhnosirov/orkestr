//! Naming and input validation: agent branches/worktree paths
//! (git-worktree-strategy §2–3), relative paths and revisions.

use std::path::{Component, Path, PathBuf};

use crate::error::{VcsError, VcsResult};

/// Max length of a role slug.
pub const MAX_ROLE_SLUG: usize = 24;

/// Last 8 hex chars of a UUID string (its random part for UUID v7).
#[must_use]
pub fn short_id(uuid: &str) -> String {
    let hex: String = uuid.chars().filter(char::is_ascii_hexdigit).collect();
    let start = hex.len().saturating_sub(8);
    hex[start..].to_ascii_lowercase()
}

/// Lowercase `[a-z0-9-]` slug, ≤ `max` chars, `fallback` when empty.
#[must_use]
pub fn slug(input: &str, max: usize, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(max).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

/// `agent/<role>-<agent short>`.
#[must_use]
pub fn agent_branch_name(role: Option<&str>, agent_id: &str) -> String {
    let role = slug(role.unwrap_or("agent"), MAX_ROLE_SLUG, "agent");
    format!("agent/{role}-{}", short_id(agent_id))
}

/// Appends `-2`, `-3`, … for collision attempt `n` (n = 1 → unchanged).
#[must_use]
pub fn with_suffix(base: &str, n: u32) -> String {
    if n <= 1 {
        base.to_owned()
    } else {
        format!("{base}-{n}")
    }
}

/// `<root>/<project short>/<leaf>`.
#[must_use]
pub fn worktree_path(worktrees_root: &Path, project_id: &str, leaf: &str) -> PathBuf {
    worktrees_root.join(short_id(project_id)).join(leaf)
}

/// Validates a repository-relative path given by a client: non-empty,
/// relative, no `..`, no NUL/control chars. Returns it `/`-normalized.
pub fn validate_rel_path(path: &str) -> VcsResult<String> {
    let p = path.trim();
    if p.is_empty() {
        return Err(VcsError::invalid("path must not be empty"));
    }
    if p.chars().any(|c| c == '\0' || (c.is_control() && c != '\t')) {
        return Err(VcsError::invalid("path contains control characters"));
    }
    let normalized = p.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with("//") {
        return Err(VcsError::invalid(format!("path must be relative: {p}")));
    }
    let pb = Path::new(&normalized);
    for c in pb.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(VcsError::invalid(format!("path must stay inside the repository: {p}"))),
        }
    }
    let cleaned = normalized.trim_start_matches("./").trim_end_matches('/').to_owned();
    if cleaned.is_empty() || cleaned == "." {
        return Err(VcsError::invalid("path must name a file or directory"));
    }
    Ok(cleaned)
}

/// Validates a revision/ref expression: no leading `-` (option injection), no
/// whitespace/control chars, ≤ 256 chars.
pub fn validate_rev(rev: &str) -> VcsResult<String> {
    let r = rev.trim();
    if r.is_empty() {
        return Err(VcsError::invalid("revision must not be empty"));
    }
    if r.len() > 256 {
        return Err(VcsError::invalid("revision is too long"));
    }
    if r.starts_with('-') {
        return Err(VcsError::invalid(format!("invalid revision `{r}`")));
    }
    if r.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(VcsError::invalid(format!("invalid revision `{r}`")));
    }
    Ok(r.to_owned())
}

/// Normalized comparison key of a filesystem path: canonical when it exists,
/// `\\?\` stripped, `/` separators, no trailing slash, case-folded on
/// case-insensitive platforms.
#[must_use]
pub fn path_key(p: &Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/UNC/").map(|r| format!("//{r}")).unwrap_or(s);
    let s = s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s);
    let s = s.trim_end_matches('/').to_owned();
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_uses_random_suffix() {
        assert_eq!(short_id("0190f0b0-1a2b-7c3d-8e4f-0123456789ab"), "456789ab");
        assert_eq!(
            agent_branch_name(Some("Backend"), "0190f0b0-1a2b-7c3d-8e4f-00003f9a1c2e"),
            "agent/backend-3f9a1c2e"
        );
        assert_eq!(agent_branch_name(None, "0190f0b0-1a2b-7c3d-8e4f-00003f9a1c2e"), "agent/agent-3f9a1c2e");
    }

    #[test]
    fn role_slugs() {
        assert_eq!(slug("UI/UX Designer!!", 24, "x"), "ui-ux-designer");
        assert_eq!(slug("   ", 24, "agent"), "agent");
        assert_eq!(slug("a-very-long-role-name-that-goes-on", 24, "x").len(), 24);
        assert_eq!(slug("Школа", 24, "agent"), "agent");
        assert_eq!(with_suffix("agent/qa-1", 1), "agent/qa-1");
        assert_eq!(with_suffix("agent/qa-1", 3), "agent/qa-1-3");
    }

    #[test]
    fn rel_path_validation() {
        assert_eq!(validate_rel_path("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(validate_rel_path("src\\a b\\ü.rs").unwrap(), "src/a b/ü.rs");
        assert_eq!(validate_rel_path("./x/").unwrap(), "x");
        assert!(validate_rel_path("../x").is_err());
        assert!(validate_rel_path("a/../../x").is_err());
        assert!(validate_rel_path("/etc/passwd").is_err());
        assert!(validate_rel_path("").is_err());
        assert!(validate_rel_path(".").is_err());
        assert!(validate_rel_path("a\0b").is_err());
        #[cfg(windows)]
        {
            assert!(validate_rel_path("C:\\x").is_err());
            assert!(validate_rel_path("\\\\server\\share").is_err());
        }
    }

    #[test]
    fn rev_validation() {
        assert!(validate_rev("main~2").is_ok());
        assert!(validate_rev("abc123^").is_ok());
        assert!(validate_rev("--output=/tmp/x").is_err());
        assert!(validate_rev("a b").is_err());
        assert!(validate_rev("").is_err());
    }
}
