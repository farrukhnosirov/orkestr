//! Filesystem helpers with OS-specific permission handling.

use std::io::Write;
use std::path::Path;

use crate::error::PlatformResult;

/// Atomically writes a file readable only by the current user.
///
/// * Unix: created with mode `0600` before any content is written.
/// * Windows: the file's DACL is replaced by an explicit, non-inherited ACE
///   granting full control to the current user only
///   ([`restrict_to_current_user`], via `icacls /inheritance:r /grant:r
///   *<user SID>:F`), applied to the temp file **before** any content is
///   written. If `icacls` is unavailable the inherited user-profile ACL
///   (user, SYSTEM, Administrators) remains and a warning is logged.
///
/// The content is written to a sibling temp file and renamed over `path`.
pub fn write_private_file(path: &Path, contents: &[u8]) -> PlatformResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        #[cfg(windows)]
        if let Err(e) = restrict_to_current_user(&tmp) {
            tracing::warn!(error = %e, path = %path.display(), "could not restrict file ACL to the current user");
        }
        f.write_all(contents)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// SID of the current user (`S-1-5-21-…`), cached.
#[cfg(windows)]
fn current_user_sid() -> PlatformResult<String> {
    use std::sync::OnceLock;
    static SID: OnceLock<Option<String>> = OnceLock::new();
    let sid = SID.get_or_init(|| {
        let out = std::process::Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        // "domain\user","S-1-5-21-…"
        let text = String::from_utf8_lossy(&out.stdout);
        let sid = text.trim().rsplit(',').next()?.trim().trim_matches('"').to_owned();
        sid.starts_with("S-1-").then_some(sid)
    });
    sid.clone().ok_or_else(|| crate::error::PlatformError::Invalid("cannot determine the current user's SID".into()))
}

/// Restricts `path` to the current user: removes inherited ACEs and grants
/// only `<current user SID>:F` (Windows, via `icacls`). No-op elsewhere
/// (Unix uses mode `0600`).
pub fn restrict_to_current_user(path: &Path) -> PlatformResult<()> {
    #[cfg(windows)]
    {
        let sid = current_user_sid()?;
        let out = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{sid}:F"))
            .arg("/q")
            .output()?;
        if !out.status.success() {
            return Err(crate::error::PlatformError::Command {
                command: "icacls".into(),
                status: out.status.to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

/// Applies Unix permission bits (e.g. the executable bit of a binary unpacked
/// from an archive). No-op on Windows, where executability is by extension.
pub fn set_unix_mode(path: &Path, mode: u32) -> PlatformResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("secret.txt");
        write_private_file(&p, b"one").unwrap();
        write_private_file(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            let out = std::process::Command::new("icacls").arg(&p).output().unwrap();
            let acl = String::from_utf8_lossy(&out.stdout).to_string();
            let sid = current_user_sid().unwrap();
            assert!(!acl.contains("(I)"), "no inherited ACEs: {acl}");
            assert!(!acl.to_lowercase().contains("everyone"), "{acl}");
            let lines = acl.lines().filter(|l| l.contains(":(")).count();
            assert_eq!(lines, 1, "exactly one ACE (the current user {sid}): {acl}");
        }
    }
}
