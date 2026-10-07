//! Safe `.vsix` extraction.
//!
//! A `.vsix` is a zip archive whose payload lives under `extension/`. The
//! archive is untrusted:
//!
//! * entry names are normalized and confined ([`crate::paths`]): absolute
//!   paths, drive letters, UNC, `..` escapes (zip-slip), alternate data
//!   streams and reserved device names are rejected;
//! * symbolic links are rejected;
//! * entry count, per-file size, total size and compression ratio are capped
//!   (zip bombs), and the actual decompressed byte count is enforced, not
//!   just the size the archive claims;
//! * files are written with default permissions — the executable bit of
//!   archive entries is never applied (a bundled language server binary is
//!   made runnable only after the user approved running it);
//! * extraction goes to a fresh staging directory that is removed on failure.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::error::{ExtError, ExtResult};
use crate::paths;

/// Extraction limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Max number of entries.
    pub max_entries: usize,
    /// Max decompressed size of one file.
    pub max_file_bytes: u64,
    /// Max decompressed size of all files.
    pub max_total_bytes: u64,
    /// Max ratio decompressed / compressed for entries larger than 1 MiB.
    pub max_ratio: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_entries: 50_000, max_file_bytes: 256 << 20, max_total_bytes: 768 << 20, max_ratio: 200 }
    }
}

/// Result of an extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// Files written.
    pub files: usize,
    /// Bytes written.
    pub bytes: u64,
}

const PREFIX: &str = "extension/";
const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

/// Extracts the `extension/` payload of a `.vsix` into `dest` (which must
/// not exist yet). On error `dest` is removed.
pub fn extract<R: Read + Seek>(reader: R, dest: &Path, limits: Limits) -> ExtResult<Extracted> {
    if dest.exists() {
        return Err(ExtError::Conflict(format!("staging folder {} already exists", dest.display())));
    }
    fs::create_dir_all(dest)?;
    let result = extract_inner(reader, dest, limits);
    if result.is_err() {
        let _ = fs::remove_dir_all(dest);
    }
    result
}

fn extract_inner<R: Read + Seek>(reader: R, dest: &Path, limits: Limits) -> ExtResult<Extracted> {
    let mut zip = zip::ZipArchive::new(reader)?;
    if zip.len() > limits.max_entries {
        return Err(ExtError::TooLarge(format!("{} entries (limit {})", zip.len(), limits.max_entries)));
    }
    let mut out = Extracted { files: 0, bytes: 0 };
    let mut seen = std::collections::HashSet::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let raw = entry.name().to_owned();
        let name = raw.replace('\\', "/");
        // Check every entry name (also outside `extension/`) so a malicious
        // archive is rejected as a whole rather than partially installed.
        let Some(rest) = name.strip_prefix(PREFIX) else {
            if name.contains("..") || name.starts_with('/') {
                return Err(ExtError::Unsafe(format!("archive entry `{raw}` escapes the archive root")));
            }
            continue;
        };
        if entry.is_symlink() || entry.unix_mode().is_some_and(|m| m & S_IFMT == S_IFLNK) {
            return Err(ExtError::Unsafe(format!("archive entry `{raw}` is a symbolic link")));
        }
        if rest.is_empty() {
            continue;
        }
        let is_dir = entry.is_dir() || rest.ends_with('/');
        let rel = paths::normalize(rest.trim_end_matches('/'))?;
        // Case-insensitive duplicates would overwrite each other on Windows/macOS.
        if !seen.insert(rel.to_ascii_lowercase()) && !is_dir {
            return Err(ExtError::Unsafe(format!("duplicate archive entry `{raw}`")));
        }
        let target = rel.split('/').fold(dest.to_path_buf(), |p, c| p.join(c));
        if is_dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        let declared = entry.size();
        if declared > limits.max_file_bytes {
            return Err(ExtError::TooLarge(format!("`{rel}` is {declared} bytes (limit {})", limits.max_file_bytes)));
        }
        let compressed = entry.compressed_size().max(1);
        if declared > (1 << 20) && declared / compressed > limits.max_ratio {
            return Err(ExtError::Unsafe(format!("`{rel}` has a suspicious compression ratio (zip bomb?)")));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        ensure_inside(dest, &target)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(&target)?;
        let budget = limits.max_file_bytes.min(limits.max_total_bytes.saturating_sub(out.bytes));
        let written = copy_limited(&mut entry, &mut file, budget)?;
        if written > budget {
            drop(file);
            return Err(ExtError::TooLarge(format!(
                "`{rel}` decompresses beyond the size limit (total limit {} bytes)",
                limits.max_total_bytes
            )));
        }
        file.flush()?;
        out.files += 1;
        out.bytes += written;
    }
    if out.files == 0 {
        return Err(ExtError::Invalid("the archive has no `extension/` folder — is this a .vsix?".into()));
    }
    Ok(out)
}

/// Copies at most `budget + 1` bytes (so an overrun is detectable).
fn copy_limited(r: &mut impl Read, w: &mut impl Write, budget: u64) -> ExtResult<u64> {
    let mut limited = r.take(budget.saturating_add(1));
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = limited.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > budget {
            return Ok(total);
        }
        w.write_all(&buf[..n])?;
    }
    Ok(total)
}

/// Defence in depth: the parent of `target` must resolve inside `dest`.
fn ensure_inside(dest: &Path, target: &Path) -> ExtResult<()> {
    let root = fs::canonicalize(dest)?;
    let parent = target.parent().map_or_else(|| Ok(root.clone()), fs::canonicalize)?;
    if !parent.starts_with(&root) {
        return Err(ExtError::Unsafe(format!("`{}` resolves outside the extension folder", target.display())));
    }
    if fs::symlink_metadata(target).is_ok() {
        return Err(ExtError::Unsafe(format!("`{}` already exists", target.display())));
    }
    Ok(())
}

/// Reads `extension/package.json` (and `package.nls.json`) from an extracted folder.
pub fn read_manifest(dir: &Path) -> ExtResult<(String, Option<serde_json::Value>)> {
    let manifest = read_text_capped(&dir.join("package.json"), 4 << 20)
        .map_err(|_| ExtError::Invalid("the extension has no package.json".into()))?;
    let nls = read_text_capped(&dir.join("package.nls.json"), 4 << 20).ok().and_then(|t| crate::jsonc::parse(&t).ok());
    Ok((manifest, nls))
}

/// Reads a UTF-8 file up to `max` bytes.
pub fn read_text_capped(path: &PathBuf, max: u64) -> ExtResult<String> {
    let meta = fs::metadata(path)?;
    if meta.len() > max {
        return Err(ExtError::TooLarge(format!("{} is larger than {max} bytes", path.display())));
    }
    let bytes = fs::read(path)?;
    String::from_utf8(bytes).map_err(|_| ExtError::Invalid(format!("{} is not UTF-8", path.display())))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    /// Builds an in-memory zip from `(name, contents, unix_mode)` entries.
    pub(crate) fn make_zip(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data, mode) in entries {
            let mut opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            if let Some(m) = mode {
                opts = opts.unix_permissions(*m);
            }
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_payload_only() {
        let dir = tempfile::tempdir().unwrap();
        let z = make_zip(&[
            ("[Content_Types].xml", b"<x/>", None),
            ("extension.vsixmanifest", b"<x/>", None),
            ("extension/package.json", br#"{"name":"a"}"#, None),
            ("extension/themes/t.json", b"{}", None),
            ("extension/bin/server", b"\x7fELF", Some(0o755)),
        ]);
        let dest = dir.path().join("out");
        let r = extract(Cursor::new(z), &dest, Limits::default()).unwrap();
        assert_eq!(r.files, 3);
        assert!(dest.join("package.json").is_file());
        assert!(dest.join("themes/t.json").is_file());
        assert!(!dest.join("extension.vsixmanifest").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.join("bin/server")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0, "executable bits from the archive must not be applied");
        }
    }

    #[test]
    fn rejects_zip_slip() {
        for evil in ["extension/../../evil.txt", "extension/a/../../../evil.txt", "../evil.txt", "extension//abs"] {
            let dir = tempfile::tempdir().unwrap();
            let z = make_zip(&[("extension/package.json", b"{}", None), (evil, b"x", None)]);
            let dest = dir.path().join("out");
            let err = extract(Cursor::new(z), &dest, Limits::default());
            assert!(matches!(err, Err(ExtError::Unsafe(_))), "{evil}: {err:?}");
            assert!(!dest.exists(), "staging folder must be removed");
            assert!(!dir.path().join("evil.txt").exists());
        }
    }

    #[test]
    fn rejects_windows_tricks() {
        for evil in ["extension/C:/x.txt", "extension/a/file.txt:ads", "extension/con.json", "extension/a\\..\\..\\x"] {
            let dir = tempfile::tempdir().unwrap();
            let z = make_zip(&[("extension/package.json", b"{}", None), (evil, b"x", None)]);
            let err = extract(Cursor::new(z), &dir.path().join("out"), Limits::default());
            assert!(matches!(err, Err(ExtError::Unsafe(_))), "{evil}: {err:?}");
        }
    }

    #[test]
    fn rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("extension/package.json", SimpleFileOptions::default()).unwrap();
        w.write_all(b"{}").unwrap();
        w.add_symlink("extension/link", "/etc/passwd", SimpleFileOptions::default()).unwrap();
        let z = w.finish().unwrap().into_inner();
        let err = extract(Cursor::new(z), &dir.path().join("out"), Limits::default());
        assert!(matches!(err, Err(ExtError::Unsafe(_))), "{err:?}");
    }

    #[test]
    fn enforces_size_limits() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b'a'; 4096];
        let z = make_zip(&[("extension/package.json", b"{}", None), ("extension/big.txt", &big, None)]);
        let limits = Limits { max_file_bytes: 1024, ..Limits::default() };
        let err = extract(Cursor::new(z.clone()), &dir.path().join("a"), limits);
        assert!(matches!(err, Err(ExtError::TooLarge(_))), "{err:?}");
        let limits = Limits { max_total_bytes: 2000, ..Limits::default() };
        let err = extract(Cursor::new(z), &dir.path().join("b"), limits);
        assert!(matches!(err, Err(ExtError::TooLarge(_))), "{err:?}");
    }

    #[test]
    fn rejects_non_vsix() {
        let dir = tempfile::tempdir().unwrap();
        let z = make_zip(&[("readme.txt", b"hi", None)]);
        assert!(matches!(extract(Cursor::new(z), &dir.path().join("o"), Limits::default()), Err(ExtError::Invalid(_))));
        let err = extract(Cursor::new(b"not a zip".to_vec()), &dir.path().join("p"), Limits::default());
        assert!(matches!(err, Err(ExtError::Invalid(_))));
    }
}
