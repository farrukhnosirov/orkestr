//! Running-core discovery (contract §3).
//!
//! `<data_dir>/core.json` = `{ "url", "token_file_hint", "pid", "version", "started_at" }`;
//! the token itself lives in `<data_dir>/core.token` (user-private file).
//! Clients (desktop, CLI) read both, verify the pid is alive and `/health`
//! answers, then connect.

use std::path::{Path, PathBuf};
use std::time::Duration;

use orkestr_platform::fs::write_private_file;
use orkestr_platform::{NativeProcessControl, Paths, ProcessControl};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Contents of `core.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryFile {
    /// RPC WebSocket URL.
    pub url: String,
    /// Path of the token file.
    pub token_file_hint: PathBuf,
    /// Core process id.
    pub pid: u32,
    /// Core version.
    #[serde(default)]
    pub version: String,
    /// RFC 3339 start time.
    #[serde(default)]
    pub started_at: String,
}

/// A verified running core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningCore {
    /// RPC URL.
    pub url: String,
    /// Bearer token.
    pub token: String,
    /// Pid.
    pub pid: u32,
}

/// Writes `core.token` then `core.json`.
pub fn write(paths: &Paths, url: &str, token: &str) -> std::io::Result<()> {
    let token_file = paths.token_file();
    write_private_file(&token_file, token.as_bytes()).map_err(std::io::Error::other)?;
    let file = DiscoveryFile {
        url: url.to_owned(),
        token_file_hint: token_file,
        pid: std::process::id(),
        version: crate::VERSION.to_owned(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    write_private_file(&paths.discovery_file(), &json).map_err(std::io::Error::other)
}

/// Reads `core.json` without verifying anything.
#[must_use]
pub fn read(paths: &Paths) -> Option<DiscoveryFile> {
    let bytes = std::fs::read(paths.discovery_file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Removes the discovery + token files if they belong to this process.
pub fn remove_if_ours(paths: &Paths) {
    if read(paths).is_some_and(|d| d.pid == std::process::id()) {
        let _ = std::fs::remove_file(paths.discovery_file());
        let _ = std::fs::remove_file(paths.token_file());
    }
}

/// Finds a live core for this data dir: pid alive, token readable, `/health` OK.
/// Stale files are ignored (and left for the next start to overwrite).
pub async fn find_running(paths: &Paths) -> Option<RunningCore> {
    let file = read(paths)?;
    if file.pid == std::process::id() || !NativeProcessControl.is_alive(file.pid) {
        return None;
    }
    // Only ever read the canonical token file of this data dir: `core.json`
    // is not trusted to point us at arbitrary files.
    if file.token_file_hint != paths.token_file() {
        tracing::warn!("ignoring core.json whose token_file_hint is not this data dir's token file");
        return None;
    }
    let token = std::fs::read_to_string(paths.token_file()).ok()?.trim().to_owned();
    if token.is_empty() {
        return None;
    }
    let authority = authority_of(&file.url)?;
    if !probe_health(&authority, Duration::from_millis(1500)).await {
        return None;
    }
    Some(RunningCore { url: file.url, token, pid: file.pid })
}

/// `ws://127.0.0.1:1234/rpc` → `127.0.0.1:1234` (loopback only).
#[must_use]
pub fn authority_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("ws://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| authority.to_owned())
}

/// Minimal HTTP/1.1 `GET /health` probe (no HTTP client dependency needed).
pub async fn probe_health(authority: &str, timeout: Duration) -> bool {
    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect(authority).await.ok()?;
        let req = format!("GET /health HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.ok()?;
        let mut buf = Vec::with_capacity(512);
        stream.take(8192).read_to_end(&mut buf).await.ok()?;
        let text = String::from_utf8_lossy(&buf);
        Some(text.starts_with("HTTP/1.1 200") && text.contains("\"ok\""))
    };
    matches!(tokio::time::timeout(timeout, attempt).await, Ok(Some(true)))
}

/// Path helper for hosts that want to show where discovery lives.
#[must_use]
pub fn discovery_path(paths: &Paths) -> PathBuf {
    paths.discovery_file()
}

/// Whether a path looks like our token file (sanity check for hints).
#[must_use]
pub fn is_token_file(paths: &Paths, p: &Path) -> bool {
    p == paths.token_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_parsing_is_loopback_only() {
        assert_eq!(authority_of("ws://127.0.0.1:5000/rpc").as_deref(), Some("127.0.0.1:5000"));
        assert_eq!(authority_of("ws://localhost:5000/rpc").as_deref(), Some("localhost:5000"));
        assert_eq!(authority_of("ws://10.0.0.5:5000/rpc"), None);
        assert_eq!(authority_of("wss://127.0.0.1:5000/rpc"), None);
    }

    #[tokio::test]
    async fn stale_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(dir.path());
        write(&paths, "ws://127.0.0.1:1/rpc", "t").unwrap();
        // pid is ours → never considered "another running core".
        assert!(find_running(&paths).await.is_none());
        let mut f = read(&paths).unwrap();
        f.pid = u32::MAX - 3; // dead pid
        std::fs::write(paths.discovery_file(), serde_json::to_vec(&f).unwrap()).unwrap();
        assert!(find_running(&paths).await.is_none());
        remove_if_ours(&paths); // not ours anymore → untouched
        assert!(paths.discovery_file().exists());
    }
}
