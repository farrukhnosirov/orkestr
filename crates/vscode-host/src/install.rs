//! Download, verification and extraction of the server package, and the
//! on-disk layout under `<data_dir>/vscode-server/`.
//!
//! ```text
//! vscode-server/
//!   <version>/            unpacked REH-web package (node runtime + server)
//!   user-data/            VS Code user settings, state, logs
//!   extensions/           installed extensions (Open VSX)
//!   server-data/          server data dir
//!   downloads/            partial downloads (removed after install)
//!   installed.json        the verified installation
//!   config.json           user configuration
//!   state.json            last port (stable origin keeps VS Code UI state)
//!   connection-token      token of the running server (user-only ACL)
//! ```

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::error::{HostError, HostResult};
use crate::release::MAX_ARCHIVE_BYTES;

/// Paths under the VS Code root.
#[derive(Debug, Clone)]
pub struct Layout {
    /// `<data_dir>/vscode-server`.
    pub root: PathBuf,
}

impl Layout {
    /// Layout rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    /// Directory of an unpacked version.
    #[must_use]
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.root.join(version)
    }
    /// VS Code user data.
    #[must_use]
    pub fn user_data_dir(&self) -> PathBuf {
        self.root.join("user-data")
    }
    /// Extensions.
    #[must_use]
    pub fn extensions_dir(&self) -> PathBuf {
        self.root.join("extensions")
    }
    /// Server data.
    #[must_use]
    pub fn server_data_dir(&self) -> PathBuf {
        self.root.join("server-data")
    }
    /// Partial downloads.
    #[must_use]
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }
    /// Installation manifest.
    #[must_use]
    pub fn manifest_file(&self) -> PathBuf {
        self.root.join("installed.json")
    }
    /// Configuration.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }
    /// Runtime state (last port).
    #[must_use]
    pub fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }
    /// Connection token file.
    #[must_use]
    pub fn token_file(&self) -> PathBuf {
        self.root.join("connection-token")
    }
}

/// Streams `url` into `dest`, computing the sha256 on the fly. `progress`
/// receives `(received, total)`. The size must match `expected_size` when given.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_size: Option<u64>,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> HostResult<String> {
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(HostError::Network(format!("download failed: HTTP {status}")));
    }
    let total = resp.content_length().or(expected_size);
    if let Some(t) = total {
        if t > MAX_ARCHIVE_BYTES {
            return Err(HostError::Archive(format!("refusing a {t}-byte download")));
        }
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut stream = resp.bytes_stream();
    loop {
        let chunk = tokio::select! {
            () = cancel.cancelled() => return Err(HostError::Cancelled),
            next = stream.next() => next,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        received = received.saturating_add(chunk.len() as u64);
        if received > MAX_ARCHIVE_BYTES {
            return Err(HostError::Archive("download exceeds the size limit".into()));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        progress(received, total);
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if let Some(expected) = expected_size {
        if received != expected {
            return Err(HostError::Archive(format!("incomplete download: {received} of {expected} bytes")));
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

/// sha256 of a file (hex).
pub fn sha256_file(path: &Path) -> HostResult<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// A safe relative path for an archive entry: no absolute paths, drive
/// prefixes or `..`; `.` components dropped. `None` = skip (the root entry).
pub fn safe_entry_path(path: &Path) -> HostResult<Option<PathBuf>> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => {
                let s = p.to_string_lossy();
                // A `C:` / `\\?\` hidden in a normal component on Unix-built archives.
                if s.contains(':') || s.contains('\\') {
                    return Err(HostError::Archive(format!("unsafe path in archive: {}", path.display())));
                }
                out.push(p);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(HostError::Archive(format!("unsafe path in archive: {}", path.display())));
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// What an extraction did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractReport {
    /// Regular files written.
    pub files: u64,
    /// Directories created.
    pub dirs: u64,
    /// Symlinks / hard links / special entries skipped.
    pub skipped: u64,
}

/// `<dest>.partial` (appended, not an extension swap: versions contain dots).
fn partial_dir(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".partial");
    dest.with_file_name(name)
}

/// Unpacks a `.tar.gz` into `dest` (which must not exist): first into
/// `dest.partial`, then renamed, so a crash never leaves a half package that
/// looks installed. Only regular files and directories are written; links and
/// special entries are skipped; every path is validated ([`safe_entry_path`]).
pub fn extract_tar_gz(
    archive: &Path,
    dest: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64),
) -> HostResult<ExtractReport> {
    let partial = partial_dir(dest);
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(&partial)?;
    let result = (|| -> HostResult<ExtractReport> {
        let file = std::fs::File::open(archive)?;
        let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
        let mut tar = tar::Archive::new(gz);
        let mut report = ExtractReport::default();
        let entries = tar.entries().map_err(|e| HostError::Archive(e.to_string()))?;
        for entry in entries {
            if cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            let mut entry = entry.map_err(|e| HostError::Archive(e.to_string()))?;
            let raw = entry.path().map_err(|e| HostError::Archive(e.to_string()))?.into_owned();
            let Some(rel) = safe_entry_path(&raw)? else { continue };
            let target = partial.join(&rel);
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                std::fs::create_dir_all(&target)?;
                report.dirs += 1;
            } else if kind.is_file() || kind == tar::EntryType::Continuous {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut entry, &mut out).map_err(|e| HostError::Archive(e.to_string()))?;
                drop(out);
                if let Ok(mode) = entry.header().mode() {
                    if mode & 0o111 != 0 {
                        orkestr_platform::fs::set_unix_mode(&target, 0o755)?;
                    }
                }
                report.files += 1;
                progress(report.files);
            } else {
                tracing::debug!(path = %raw.display(), ?kind, "skipping non-regular archive entry");
                report.skipped += 1;
            }
        }
        Ok(report)
    })();
    match result {
        Ok(report) => {
            if dest.exists() {
                std::fs::remove_dir_all(dest)?;
            }
            std::fs::rename(&partial, dest)?;
            Ok(report)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

/// Checks an unpacked package looks like a VSCodium REH-web server.
pub fn validate_package(dir: &Path, node_executable: &str) -> HostResult<()> {
    for rel in ["out/server-main.js", "product.json", node_executable] {
        if !dir.join(rel).is_file() {
            return Err(HostError::Archive(format!("the package is incomplete: `{rel}` is missing")));
        }
    }
    let product = std::fs::read_to_string(dir.join("product.json"))?;
    let product: serde_json::Value =
        serde_json::from_str(&product).map_err(|e| HostError::Archive(format!("product.json: {e}")))?;
    // VSCodium is configured for Open VSX; never ship a Microsoft Marketplace gallery.
    let gallery = product.pointer("/extensionsGallery/serviceUrl").and_then(|v| v.as_str()).unwrap_or_default();
    if gallery.contains("marketplace.visualstudio.com") {
        return Err(HostError::Archive("the package is configured for the Microsoft Marketplace".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(if path.ends_with("node") { 0o755 } else { 0o644 });
            header.set_entry_type(tar::EntryType::Regular);
            // Write the raw name bytes (bypassing `set_path` validation) so hostile names can be tested.
            let name = header.as_old_mut();
            name.name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn safe_paths() {
        assert_eq!(safe_entry_path(Path::new("./bin/x")).unwrap(), Some(PathBuf::from("bin/x")));
        assert_eq!(safe_entry_path(Path::new("./")).unwrap(), None);
        for bad in ["../evil", "a/../../evil", "/etc/passwd", "C:\\x", "a/C:x"] {
            assert!(safe_entry_path(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn extracts_atomically_and_validates_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.tar.gz");
        let product = br#"{"extensionsGallery":{"serviceUrl":"https://open-vsx.org/vscode/gallery"}}"#;
        std::fs::write(
            &archive,
            tar_gz(&[("./out/server-main.js", b"//"), ("./product.json", product), ("./node", b"bin")]),
        )
        .unwrap();
        let dest = dir.path().join("1.0.0");
        let cancel = CancellationToken::new();
        let mut seen = 0;
        let r = extract_tar_gz(&archive, &dest, &cancel, |n| seen = n).unwrap();
        assert_eq!(r.files, 3);
        assert_eq!(seen, 3);
        assert!(!dir.path().join("1.0.0.partial").exists());
        validate_package(&dest, "node").unwrap();
        assert!(validate_package(&dest, "node.exe").is_err());
    }

    #[test]
    fn path_traversal_aborts_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.tar.gz");
        std::fs::write(&archive, tar_gz(&[("./ok.txt", b"1"), ("../escape.txt", b"2")])).unwrap();
        let dest = dir.path().join("x").join("1.0.0");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let err = extract_tar_gz(&archive, &dest, &CancellationToken::new(), |_| {}).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        assert!(!dest.exists());
        assert!(!dir.path().join("x").join("escape.txt").exists());
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn marketplace_packages_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/server-main.js"), "").unwrap();
        std::fs::write(dir.path().join("node"), "").unwrap();
        std::fs::write(
            dir.path().join("product.json"),
            r#"{"extensionsGallery":{"serviceUrl":"https://marketplace.visualstudio.com/_apis/public/gallery"}}"#,
        )
        .unwrap();
        assert!(validate_package(dir.path(), "node").unwrap_err().to_string().contains("Marketplace"));
    }

    #[test]
    fn file_hash_matches_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn cancelled_extraction_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.tar.gz");
        std::fs::write(&archive, tar_gz(&[("./a", b"1")])).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let dest = dir.path().join("v");
        assert!(matches!(extract_tar_gz(&archive, &dest, &cancel, |_| {}), Err(HostError::Cancelled)));
        assert!(!dest.exists());
        assert!(!dir.path().join("v.partial").exists());
    }
}
