//! The Orkestr bridge: a tiny VS Code extension (`crates/vscode-host/bridge/`,
//! MIT) that runs inside the embedded VS Code and connects **back** to this
//! process over loopback so Orkestr can run extension commands.
//!
//! * [`build_vsix`] packs the bundled extension sources into a `.vsix`
//!   (a stored zip) — no build step, the sources are `include_str!`-ed.
//! * [`Bridge`] is the loopback listener (`127.0.0.1:<ephemeral>`): a
//!   connection must open with `{"type":"hello","token":…}` carrying the
//!   current one-time token (compared in constant time) within
//!   [`HELLO_TIMEOUT`], otherwise it is closed. Requests/responses are
//!   newline-delimited JSON (see `bridge/README.md`).
//!
//! The port and token reach the extension through a private file
//! (`<root>/bridge.json`) whose path is passed in `ORKESTR_BRIDGE_FILE`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Version of the bundled bridge extension (matches `bridge/package.json`).
pub const BRIDGE_VERSION: &str = "0.1.0";
/// Extension id of the bridge.
pub const BRIDGE_EXTENSION_ID: &str = "orkestr.orkestr-bridge";
/// Env var naming the bridge connection file.
pub const BRIDGE_FILE_ENV: &str = "ORKESTR_BRIDGE_FILE";
/// How long a new connection has to authenticate.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest accepted line (a full command list is a few hundred KiB).
pub const MAX_LINE: usize = 4 * 1024 * 1024;

const PACKAGE_JSON: &str = include_str!("../bridge/package.json");
const EXTENSION_JS: &str = include_str!("../bridge/extension.js");
const README: &str = include_str!("../bridge/README.md");
const LICENSE: &str = include_str!("../bridge/LICENSE.txt");

/// Errors of a bridge call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// No VS Code window is connected.
    #[error("no VS Code window is connected to Orkestr (open VS Code first)")]
    NotConnected,
    /// The call timed out.
    #[error("VS Code did not answer in time")]
    Timeout,
    /// The window disconnected while the call was running.
    #[error("the VS Code window disconnected")]
    Disconnected,
    /// The extension host reported an error.
    #[error("VS Code: {0}")]
    Remote(String),
}

// ------------------------------------------------------------------ vsix

/// Files of the bridge extension as `(path inside the vsix, contents)`.
#[must_use]
pub fn bridge_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("extension/package.json", PACKAGE_JSON.as_bytes()),
        ("extension/extension.js", EXTENSION_JS.as_bytes()),
        ("extension/README.md", README.as_bytes()),
        ("extension/LICENSE.txt", LICENSE.as_bytes()),
    ]
}

fn vsix_manifest() -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="orkestr-bridge" Version="{BRIDGE_VERSION}" Publisher="orkestr" />
    <DisplayName>Orkestr Bridge</DisplayName>
    <Description xml:space="preserve">Lets Orkestr run commands of your VS Code extensions.</Description>
    <Categories>Other</Categories>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.80.0" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="workspace" />
    </Properties>
    <License>extension/LICENSE.txt</License>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code"/>
  </Installation>
  <Dependencies/>
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.License" Path="extension/LICENSE.txt" Addressable="true" />
  </Assets>
</PackageManifest>
"#
    )
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".md" ContentType="text/markdown"/><Default Extension=".txt" ContentType="text/plain"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
"#;

/// Packs `files` into an uncompressed ("stored") zip archive.
#[must_use]
pub fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    // DOS date 1980-01-01, time 00:00 (deterministic output).
    const DOS_DATE: u16 = (1 << 5) | 1;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
    let cd_size = u32::try_from(central.len()).unwrap_or(u32::MAX);
    let count = u16::try_from(files.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The bridge extension packed as a `.vsix`.
#[must_use]
pub fn build_vsix() -> Vec<u8> {
    let manifest = vsix_manifest();
    let mut files: Vec<(&str, &[u8])> =
        vec![("extension.vsixmanifest", manifest.as_bytes()), ("[Content_Types].xml", CONTENT_TYPES.as_bytes())];
    files.extend(bridge_files());
    build_zip(&files)
}

// ---------------------------------------------------------------- server

/// What the extension reported in its `hello`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeHello {
    /// Bridge extension version.
    pub version: String,
    /// VS Code version of the extension host.
    pub vscode_version: String,
    /// `vscode.env.appName`.
    pub app_name: String,
    /// Open workspace folders.
    pub folders: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Incoming {
    Hello {
        token: String,
        #[serde(flatten)]
        info: BridgeHello,
    },
    Res {
        id: u64,
        ok: bool,
        #[serde(default)]
        result: Value,
        #[serde(default)]
        error: Option<String>,
    },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, BridgeError>>>>>;

struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    hello: BridgeHello,
}

struct Inner {
    token: Mutex<Option<String>>,
    port: Mutex<Option<u16>>,
    conns: Mutex<Vec<Conn>>,
    next_id: AtomicU64,
    count_tx: watch::Sender<usize>,
    shutdown: CancellationToken,
    bind_lock: tokio::sync::Mutex<()>,
}

/// Loopback listener the bridge extension connects to (cheap to clone).
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge").field("port", &*self.inner.port.lock()).finish_non_exhaustive()
    }
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

fn token_matches(expected: &str, given: &str) -> bool {
    use subtle::ConstantTimeEq;
    expected.len() == given.len() && bool::from(expected.as_bytes().ct_eq(given.as_bytes()))
}

/// Reads one `\n`-terminated line of at most `max` bytes (`None` at EOF).
async fn read_line_limited<R: AsyncRead + Unpin>(
    r: &mut BufReader<R>,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<Option<String>> {
    buf.clear();
    loop {
        let available = r.fill_buf().await?;
        if available.is_empty() {
            return Ok(None);
        }
        if let Some(pos) = available.iter().position(|b| *b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            r.consume(pos + 1);
            return Ok(Some(String::from_utf8_lossy(buf).into_owned()));
        }
        let n = available.len();
        buf.extend_from_slice(available);
        r.consume(n);
        if buf.len() > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
        }
    }
}

impl Bridge {
    /// A bridge that is not listening yet.
    #[must_use]
    pub fn new() -> Self {
        let (count_tx, _) = watch::channel(0);
        Self {
            inner: Arc::new(Inner {
                token: Mutex::new(None),
                port: Mutex::new(None),
                conns: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                count_tx,
                shutdown: CancellationToken::new(),
                bind_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Sets (or clears) the token new connections must present.
    pub fn set_token(&self, token: Option<String>) {
        *self.inner.token.lock() = token;
    }

    /// Port the listener is bound to (after [`Bridge::listen`]).
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        *self.inner.port.lock()
    }

    /// Number of authenticated connections (VS Code windows).
    #[must_use]
    pub fn connections(&self) -> usize {
        *self.inner.count_tx.borrow()
    }

    /// Whether at least one VS Code window is connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connections() > 0
    }

    /// `hello` of the most recent connection.
    #[must_use]
    pub fn hello(&self) -> Option<BridgeHello> {
        self.inner.conns.lock().last().map(|c| c.hello.clone())
    }

    /// Watches the connection count.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<usize> {
        self.inner.count_tx.subscribe()
    }

    /// Binds `127.0.0.1:0` once and accepts connections in the background.
    ///
    /// # Errors
    /// The socket could not be bound.
    pub async fn listen(&self) -> std::io::Result<u16> {
        let _guard = self.inner.bind_lock.lock().await;
        if let Some(p) = self.port() {
            return Ok(p);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        *self.inner.port.lock() = Some(port);
        let bridge = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = bridge.inner.shutdown.cancelled() => return,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, addr)) => {
                            if !addr.ip().is_loopback() {
                                continue;
                            }
                            let b = bridge.clone();
                            tokio::spawn(async move { b.serve(stream).await });
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "bridge accept failed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        });
        Ok(port)
    }

    /// Stops accepting and drops every connection.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.conns.lock().clear();
        self.publish_count();
    }

    fn publish_count(&self) {
        let n = self.inner.conns.lock().len();
        self.inner.count_tx.send_replace(n);
    }

    async fn serve(&self, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut buf = Vec::new();
        // 1. Authenticate.
        let first = tokio::time::timeout(HELLO_TIMEOUT, read_line_limited(&mut reader, &mut buf, 64 * 1024)).await;
        let hello = match first {
            Ok(Ok(Some(line))) => match serde_json::from_str::<Incoming>(&line) {
                Ok(Incoming::Hello { token, info }) => {
                    let expected = self.inner.token.lock().clone();
                    if expected.as_deref().is_some_and(|e| token_matches(e, &token)) {
                        Some(info)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let Some(hello) = hello else {
            tracing::warn!("rejected a VS Code bridge connection without a valid token");
            let _ = write.write_all(b"{\"type\":\"error\",\"error\":\"unauthorized\"}\n").await;
            let _ = write.shutdown().await;
            return;
        };
        if write.write_all(b"{\"type\":\"welcome\"}\n").await.is_err() {
            return;
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tracing::info!(version = %hello.version, vscode = %hello.vscode_version, "VS Code bridge connected");
        self.inner.conns.lock().push(Conn { id, tx, pending: Arc::clone(&pending), hello });
        self.publish_count();
        let writer = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if write.write_all(line.as_bytes()).await.is_err() || write.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            let _ = write.shutdown().await;
        });
        // 2. Responses.
        loop {
            let line = tokio::select! {
                () = self.inner.shutdown.cancelled() => break,
                l = read_line_limited(&mut reader, &mut buf, MAX_LINE) => l,
            };
            match line {
                Ok(Some(line)) => {
                    if let Ok(Incoming::Res { id: rid, ok, result, error }) = serde_json::from_str::<Incoming>(&line) {
                        if let Some(tx) = pending.lock().remove(&rid) {
                            let _ = tx.send(if ok {
                                Ok(result)
                            } else {
                                Err(BridgeError::Remote(error.unwrap_or_else(|| "unknown error".into())))
                            });
                        }
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        self.inner.conns.lock().retain(|c| c.id != id);
        for (_, tx) in pending.lock().drain() {
            let _ = tx.send(Err(BridgeError::Disconnected));
        }
        writer.abort();
        self.publish_count();
        tracing::info!("VS Code bridge disconnected");
    }

    /// Waits until a window is connected (at most `timeout`).
    pub async fn wait_connected(&self, timeout: Duration) -> bool {
        let mut rx = self.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|n| *n > 0)).await.is_ok_and(|r| r.is_ok())
    }

    /// Sends `method(params)` to the most recently connected window.
    ///
    /// # Errors
    /// Not connected, timeout, disconnect, or the extension's own error.
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, BridgeError> {
        let rid = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let pending = {
            let conns = self.inner.conns.lock();
            let conn = conns.last().ok_or(BridgeError::NotConnected)?;
            let line = serde_json::json!({ "type": "req", "id": rid, "method": method, "params": params }).to_string();
            conn.pending.lock().insert(rid, tx);
            if conn.tx.send(line).is_err() {
                conn.pending.lock().remove(&rid);
                return Err(BridgeError::Disconnected);
            }
            Arc::clone(&conn.pending)
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(BridgeError::Disconnected),
            Err(_) => {
                pending.lock().remove(&rid);
                Err(BridgeError::Timeout)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    #[test]
    fn vsix_is_a_valid_stored_zip_with_the_manifest() {
        let bytes = build_vsix();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], b"PK\x05\x06");
        let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]);
        assert_eq!(count, 6);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("extension/package.json"));
        assert!(text.contains("\"name\": \"orkestr-bridge\""));
        // The packaged version matches the constant.
        let pkg: Value = serde_json::from_str(PACKAGE_JSON).unwrap();
        assert_eq!(pkg["version"], BRIDGE_VERSION);
        assert_eq!(
            format!("{}.{}", pkg["publisher"].as_str().unwrap(), pkg["name"].as_str().unwrap()),
            BRIDGE_EXTENSION_ID
        );
    }

    #[test]
    fn zip_crc_matches_content() {
        let zip = build_zip(&[("a.txt", b"hello")]);
        let crc = u32::from_le_bytes([zip[14], zip[15], zip[16], zip[17]]);
        assert_eq!(crc, crc32fast::hash(b"hello"));
        assert_eq!(&zip[30..35], b"a.txt");
        assert_eq!(&zip[35..40], b"hello");
    }

    #[test]
    fn token_comparison() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
    }

    async fn client(port: u16) -> (BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf) {
        let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (r, w) = s.into_split();
        (BufReader::new(r), w)
    }

    async fn next_line(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<String> {
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_secs(5), r.read_line(&mut line)).await {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => None,
            Ok(Ok(_)) => Some(line),
        }
    }

    #[tokio::test]
    async fn rejects_connections_without_the_token() {
        let bridge = Bridge::new();
        bridge.set_token(Some("secret-token".into()));
        let port = bridge.listen().await.unwrap();

        // Wrong token.
        let (mut r, mut w) = client(port).await;
        w.write_all(b"{\"type\":\"hello\",\"token\":\"nope\"}\n").await.unwrap();
        let reply = next_line(&mut r).await.unwrap();
        assert!(reply.contains("unauthorized"));
        assert!(next_line(&mut r).await.is_none(), "connection is closed");

        // No hello at all: a request-shaped line is rejected too.
        let (mut r, mut w) = client(port).await;
        w.write_all(b"{\"type\":\"res\",\"id\":1,\"ok\":true}\n").await.unwrap();
        assert!(next_line(&mut r).await.unwrap().contains("unauthorized"));

        // No token configured: everything is rejected.
        bridge.set_token(None);
        let (mut r, mut w) = client(port).await;
        w.write_all(b"{\"type\":\"hello\",\"token\":\"\"}\n").await.unwrap();
        assert!(next_line(&mut r).await.unwrap().contains("unauthorized"));

        assert!(!bridge.is_connected());
        assert_eq!(bridge.call("ping", Value::Null, Duration::from_millis(50)).await, Err(BridgeError::NotConnected));
    }

    #[tokio::test]
    async fn round_trip_with_a_mock_extension() {
        let bridge = Bridge::new();
        bridge.set_token(Some("tkn".into()));
        let port = bridge.listen().await.unwrap();
        let (mut r, mut w) = client(port).await;
        w.write_all(b"{\"type\":\"hello\",\"token\":\"tkn\",\"version\":\"0.1.0\",\"vscodeVersion\":\"1.99.0\"}\n")
            .await
            .unwrap();
        assert!(next_line(&mut r).await.unwrap().contains("welcome"));
        assert!(bridge.wait_connected(Duration::from_secs(5)).await);
        assert_eq!(bridge.hello().unwrap().vscode_version, "1.99.0");

        // Mock extension: answers `execute` with the command id, fails `boom`.
        let mock = tokio::spawn(async move {
            while let Some(line) = next_line(&mut r).await {
                let req: Value = serde_json::from_str(&line).unwrap();
                let id = req["id"].as_u64().unwrap();
                let reply = if req["params"]["command"] == "boom" {
                    serde_json::json!({ "type": "res", "id": id, "ok": false, "error": "exploded" })
                } else {
                    serde_json::json!({ "type": "res", "id": id, "ok": true, "result": { "ran": req["params"]["command"] } })
                };
                w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        });
        let ok = bridge
            .call("execute", serde_json::json!({ "command": "rainbow-csv.Align" }), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(ok["ran"], "rainbow-csv.Align");
        let err = bridge.call("execute", serde_json::json!({ "command": "boom" }), Duration::from_secs(5)).await;
        assert_eq!(err, Err(BridgeError::Remote("exploded".into())));
        mock.abort();
        // Disconnect is observed.
        let mut rx = bridge.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|n| *n == 0)).await;
        assert!(!bridge.is_connected());
    }
}
