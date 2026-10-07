//! `git clone` of a remote repository into a new folder (File → Clone
//! repository…).
//!
//! Same invocation rules as every other git call (ADR-0008): an argument
//! vector, never a shell; the URL is passed after `--` so it can never be
//! read as an option, and only network transports are accepted — local
//! paths, `file://` and remote-helper syntax (`ext::sh -c …`, `fd::…`) are
//! rejected, and `protocol.ext.allow=never` / `protocol.file.allow=never`
//! are forced for the clone (including submodules it might fetch).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::GitCli;
use crate::error::{VcsError, VcsResult};
use crate::runner::RunOptions;

/// Max accepted URL length.
pub const MAX_URL_LEN: usize = 2048;
/// Max folder name length.
pub const MAX_FOLDER_NAME_LEN: usize = 255;
/// Time budget for a clone (large repositories over slow links).
pub const CLONE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Network URL schemes accepted for cloning.
const SCHEMES: [&str; 4] = ["https://", "http://", "ssh://", "git://"];

/// Validates a clone URL and returns it trimmed.
///
/// Accepts `https://`, `http://`, `ssh://`, `git://` URLs and scp-like
/// `[user@]host:path`. Rejects anything that git could read as an option,
/// a local path, `file://`, or a remote-helper transport (`<helper>::<address>`).
pub fn validate_clone_url(url: &str) -> VcsResult<String> {
    let u = url.trim();
    if u.is_empty() {
        return Err(VcsError::invalid("repository URL must not be empty"));
    }
    if u.len() > MAX_URL_LEN {
        return Err(VcsError::invalid("repository URL is too long"));
    }
    if u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(VcsError::invalid("repository URL must not contain spaces or control characters"));
    }
    if u.starts_with('-') {
        return Err(VcsError::invalid("repository URL must not start with '-'"));
    }
    if u.contains("::") {
        return Err(VcsError::invalid("remote-helper URLs (`<transport>::<address>`) are not allowed"));
    }
    let lower = u.to_ascii_lowercase();
    if let Some(scheme) = SCHEMES.iter().find(|s| lower.starts_with(**s)) {
        let rest = &u[scheme.len()..];
        let host = rest.split('/').next().unwrap_or_default();
        if host.is_empty() || rest.len() <= host.len() + 1 {
            return Err(VcsError::invalid("repository URL must name a host and a repository path"));
        }
        if host.rsplit('@').next().unwrap_or_default().starts_with('-') {
            return Err(VcsError::invalid("repository host must not start with '-'"));
        }
        return Ok(u.to_owned());
    }
    if lower.contains("://") {
        return Err(VcsError::invalid("only https, http, ssh and git URLs can be cloned"));
    }
    // scp-like `[user@]host:path` — a host of at least two characters (so a
    // Windows drive `C:\…` is not taken for one) without path separators.
    if let Some((host, path)) = u.split_once(':') {
        let host_name = host.rsplit('@').next().unwrap_or_default();
        let host_ok = host_name.len() >= 2
            && !host.contains(['/', '\\'])
            && host_name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']'));
        if host_ok && !path.is_empty() && !path.starts_with('-') {
            return Ok(u.to_owned());
        }
    }
    Err(VcsError::invalid(
        "enter a repository URL such as https://github.com/owner/repo.git or git@host:owner/repo.git",
    ))
}

/// The folder name git would pick for `url` (`…/repo.git` → `repo`), if it is usable.
#[must_use]
pub fn folder_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    validate_folder_name(name).ok()
}

/// Validates the name of the folder a repository is cloned into.
pub fn validate_folder_name(name: &str) -> VcsResult<String> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." {
        return Err(VcsError::invalid("folder name must not be empty"));
    }
    if n.chars().count() > MAX_FOLDER_NAME_LEN {
        return Err(VcsError::invalid("folder name is too long"));
    }
    if n.starts_with('-') {
        return Err(VcsError::invalid("folder name must not start with '-'"));
    }
    if n.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(VcsError::invalid(format!("folder name contains characters that are not allowed: {n}")));
    }
    if n.ends_with('.') {
        return Err(VcsError::invalid("folder name must not end with a dot"));
    }
    let stem = n.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(VcsError::invalid(format!("{n} is a reserved device name")));
    }
    Ok(n.to_owned())
}

/// The git argument vector of a clone of `url` into `target` (both already
/// validated). Pure, so tests can check that nothing is interpolated.
#[must_use]
pub fn clone_args(url: &str, target: &Path) -> Vec<OsString> {
    vec![
        "-c".into(),
        "protocol.ext.allow=never".into(),
        "-c".into(),
        "protocol.file.allow=never".into(),
        "clone".into(),
        "--quiet".into(),
        "--".into(),
        url.into(),
        target.as_os_str().to_os_string(),
    ]
}

/// Resolves and checks the clone target: `parent` must be an existing
/// absolute directory, `parent/<name>` must not exist (or be an empty folder).
pub fn clone_target(parent: &Path, url: &str, folder_name: Option<&str>) -> VcsResult<(PathBuf, String)> {
    if !parent.is_absolute() {
        return Err(VcsError::invalid("the destination folder must be an absolute path"));
    }
    if !parent.is_dir() {
        return Err(VcsError::NotFound(format!("destination folder {} does not exist", parent.display())));
    }
    let name = match folder_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => validate_folder_name(n)?,
        None => folder_name_from_url(url)
            .ok_or_else(|| VcsError::invalid("could not derive a folder name from the URL; enter one"))?,
    };
    let target = parent.join(&name);
    if target.exists() {
        let empty_dir = target.is_dir() && std::fs::read_dir(&target)?.next().is_none();
        if !empty_dir {
            return Err(VcsError::Dirty(format!("{} already exists and is not an empty folder", target.display())));
        }
    }
    Ok((target, name))
}

impl GitCli {
    /// `git clone -- <url> <parent>/<name>`; returns the new working tree and its folder name.
    pub async fn clone_repo(
        &self,
        url: &str,
        parent: &Path,
        folder_name: Option<&str>,
    ) -> VcsResult<(PathBuf, String)> {
        let url = validate_clone_url(url)?;
        let (target, name) = clone_target(parent, &url, folder_name)?;
        let args = clone_args(&url, &target);
        let opts = RunOptions { timeout: Some(CLONE_TIMEOUT), ..RunOptions::write() };
        self.runner().run_ok(parent, &args, opts).await?;
        Ok((target, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_network_urls() {
        for u in [
            "https://github.com/owner/repo.git",
            "  http://example.com/a/b  ",
            "ssh://git@host.example:2222/owner/repo.git",
            "git://host/repo",
            "git@github.com:owner/repo.git",
            "host.example:owner/repo",
        ] {
            assert!(validate_clone_url(u).is_ok(), "{u}");
        }
        assert_eq!(validate_clone_url(" https://h/r ").unwrap(), "https://h/r");
    }

    #[test]
    fn rejects_options_local_paths_and_helpers() {
        for u in [
            "",
            "--upload-pack=touch /tmp/pwned",
            "-uevil",
            "ext::sh -c touch% /tmp/pwned",
            "ext::sh",
            "fd::17",
            "file:///etc",
            "FILE:///C:/x",
            "C:\\Code\\repo",
            "C:/Code/repo",
            "/home/me/repo",
            "\\\\server\\share\\repo",
            "https://",
            "https://host",
            "https://host/",
            "ftp://host/repo",
            "https://host/repo name",
            "https://host/repo\nx",
            "host:-oProxyCommand=evil",
            "ssh://-oProxyCommand=evil/repo",
            "ssh://git@-oProxyCommand=evil/repo",
        ] {
            assert!(validate_clone_url(u).is_err(), "{u:?} must be rejected");
        }
    }

    #[test]
    fn derives_folder_names() {
        assert_eq!(folder_name_from_url("https://github.com/owner/repo.git").as_deref(), Some("repo"));
        assert_eq!(folder_name_from_url("https://github.com/owner/repo/").as_deref(), Some("repo"));
        assert_eq!(folder_name_from_url("git@github.com:owner/my-app.git").as_deref(), Some("my-app"));
        assert_eq!(folder_name_from_url("host:repo").as_deref(), Some("repo"));
        assert_eq!(folder_name_from_url("https://host/.git"), None);
        assert_eq!(folder_name_from_url("https://host/.."), None);
    }

    #[test]
    fn folder_name_rules() {
        assert_eq!(validate_folder_name(" app ").unwrap(), "app");
        for bad in ["", ".", "..", "a/b", "a\\b", "a:b", "-x", "x.", "CON", "com1", "lpt9.txt", "a*b"] {
            assert!(validate_folder_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_folder_name("console").is_ok());
    }

    #[test]
    fn args_are_a_plain_vector_with_url_after_double_dash() {
        let target = std::env::temp_dir().join("repo; rm -rf ~");
        let url = "https://host/owner/$(whoami)`id`.git";
        let args = clone_args(url, &target);
        let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            &s[..7],
            ["-c", "protocol.ext.allow=never", "-c", "protocol.file.allow=never", "clone", "--quiet", "--"]
        );
        // The URL and the target are single, untouched arguments after `--`.
        assert_eq!(s[7], url);
        assert_eq!(PathBuf::from(&s[8]), target);
        assert_eq!(s.len(), 9);
    }

    #[test]
    fn target_must_be_new_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (t, name) = clone_target(dir.path(), "https://h/o/repo.git", None).unwrap();
        assert_eq!(name, "repo");
        assert_eq!(t, dir.path().join("repo"));
        // An explicit name wins.
        let (t, name) = clone_target(dir.path(), "https://h/o/repo.git", Some("other")).unwrap();
        assert_eq!((t, name.as_str()), (dir.path().join("other"), "other"));
        // Existing empty folder is fine; a non-empty one is refused.
        std::fs::create_dir(dir.path().join("repo")).unwrap();
        assert!(clone_target(dir.path(), "https://h/o/repo.git", None).is_ok());
        std::fs::write(dir.path().join("repo").join("x"), "x").unwrap();
        assert!(matches!(clone_target(dir.path(), "https://h/o/repo.git", None), Err(VcsError::Dirty(_))));
        // Relative / missing parents are refused.
        assert!(clone_target(Path::new("relative"), "https://h/o/r", None).is_err());
        assert!(clone_target(&dir.path().join("missing"), "https://h/o/r", None).is_err());
        assert!(clone_target(dir.path(), "https://h/o/r", Some("../escape")).is_err());
    }

    #[tokio::test]
    async fn clone_of_invalid_url_never_spawns_git() {
        // A runner without git would fail with GitNotFound if it were reached.
        let git = GitCli::new(crate::runner::GitRunner::new(None));
        let dir = tempfile::tempdir().unwrap();
        let err = git.clone_repo("ext::sh -c id", dir.path(), None).await.unwrap_err();
        assert!(matches!(err, VcsError::InvalidArgument(_)), "{err:?}");
        let err = git.clone_repo("https://h/o/r.git", dir.path(), None).await.unwrap_err();
        assert!(matches!(err, VcsError::GitNotFound), "{err:?}");
    }

    #[tokio::test]
    async fn clones_a_local_bare_repo_only_through_validation() {
        // Local sources are refused even when they exist.
        let git = GitCli::discover();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        let err = git.clone_repo(&src.to_string_lossy(), dir.path(), Some("dst")).await.unwrap_err();
        assert!(matches!(err, VcsError::InvalidArgument(_)), "{err:?}");
        assert!(!dir.path().join("dst").exists());
    }
}
