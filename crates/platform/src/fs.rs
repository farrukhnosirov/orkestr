//! Filesystem helpers with OS-specific permission handling.

use std::io::Write;
use std::path::Path;

use crate::error::PlatformResult;

/// Atomically writes a file readable only by the current user.
///
/// * Unix: created with mode `0600` before any content is written.
/// * Windows: files under the user profile inherit an ACL granting access to
///   the user, SYSTEM and Administrators only; no extra handling is needed
///   for the data dir. (Hardening with an explicit DACL is tracked for the
///   security phase.)
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
        f.write_all(contents)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
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
    }
}
