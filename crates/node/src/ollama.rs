//! Local Ollama discovery (`GET /api/tags`) — loopback only, short timeouts.
//!
//! Uses a minimal HTTP/1.0 client (no chunked encoding, connection closes at
//! the end of the body) so the node crate needs no HTTP stack. `OLLAMA_HOST`
//! is honoured when it points at a loopback address.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use orkestr_protocol::methods::nodes::{LocalModel, LocalModelsInfo};

/// Default Ollama endpoint.
pub const DEFAULT_ENDPOINT: &str = "127.0.0.1:11434";
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// Resolves the endpoint from an `OLLAMA_HOST` value; non-loopback hosts are
/// ignored (we only describe models on *this* machine).
#[must_use]
pub fn endpoint_from_env(value: Option<&str>) -> String {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else { return DEFAULT_ENDPOINT.into() };
    let rest = raw.strip_prefix("http://").unwrap_or(raw).trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p),
        _ => (rest, "11434"),
    };
    let host = match host {
        "" | "0.0.0.0" | "localhost" | "127.0.0.1" => "127.0.0.1",
        "[::1]" | "::1" => "[::1]",
        _ => return DEFAULT_ENDPOINT.into(),
    };
    format!("{host}:{port}")
}

fn http_get(endpoint: &str, path: &str, timeout: Duration) -> Result<String, String> {
    let addrs: Vec<SocketAddr> = endpoint.to_socket_addrs().map_err(|e| e.to_string())?.collect();
    let addr = addrs.into_iter().find(|a| a.ip().is_loopback()).ok_or("endpoint is not a loopback address")?;
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(800))).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let req =
        format!("GET {path} HTTP/1.0\r\nHost: {endpoint}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.take(MAX_BODY).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("malformed HTTP response")?;
    let status = head.lines().next().unwrap_or_default();
    if !(status.starts_with("HTTP/1.0 200") || status.starts_with("HTTP/1.1 200")) {
        return Err(format!("unexpected status `{status}`"));
    }
    Ok(body.to_owned())
}

/// Parses the `/api/tags` body.
#[must_use]
pub fn parse_tags(body: &str) -> Vec<LocalModel> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let Some(models) = v.get("models").and_then(|m| m.as_array()) else { return Vec::new() };
    let s = |v: Option<&serde_json::Value>| v.and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_owned);
    models
        .iter()
        .filter_map(|m| {
            let name = s(m.get("name")).or_else(|| s(m.get("model")))?;
            let details = m.get("details");
            Some(LocalModel {
                name,
                size_bytes: m.get("size").and_then(serde_json::Value::as_u64),
                family: s(details.and_then(|d| d.get("family"))),
                parameter_size: s(details.and_then(|d| d.get("parameter_size"))),
                quantization: s(details.and_then(|d| d.get("quantization_level"))),
            })
        })
        .collect()
}

/// Lists local Ollama models; unreachable servers yield `ollama_reachable: false`.
#[must_use]
pub fn discover(endpoint: &str, timeout: Duration) -> LocalModelsInfo {
    match http_get(endpoint, "/api/tags", timeout) {
        Ok(body) => {
            LocalModelsInfo { ollama_endpoint: endpoint.to_owned(), ollama_reachable: true, models: parse_tags(&body) }
        }
        Err(e) => {
            tracing::debug!(endpoint, error = %e, "ollama not reachable");
            LocalModelsInfo { ollama_endpoint: endpoint.to_owned(), ollama_reachable: false, models: Vec::new() }
        }
    }
}

/// Overrides the `ollama` binary used by the model adapter (ADR-0039): a path,
/// or `none` / empty to disable the adapter (deterministic tests).
pub const BIN_ENV: &str = "ORKESTR_OLLAMA_BIN";

/// Finds the `ollama` binary ([`BIN_ENV`] override, else `PATH`).
#[must_use]
pub fn find_binary() -> Option<PathBuf> {
    find_binary_with(std::env::var_os(BIN_ENV), std::env::var_os("PATH"))
}

/// [`find_binary`] with explicit inputs.
#[must_use]
pub fn find_binary_with(override_bin: Option<OsString>, path_var: Option<OsString>) -> Option<PathBuf> {
    if let Some(o) = override_bin {
        let s = o.to_string_lossy();
        if s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none") {
            return None;
        }
        let p = PathBuf::from(&o);
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["ollama.exe", "ollama"] } else { &["ollama"] };
    std::env::split_paths(&path_var?).flat_map(|dir| names.iter().map(move |n| dir.join(n))).find(|p| p.is_file())
}

/// Registers a GGUF file with Ollama: writes `Modelfile` (`FROM <gguf>`) in
/// `scratch` and runs `ollama create <model_ref> -f Modelfile`. Blocking.
///
/// # Errors
/// Spawn failure, non-zero exit, or timeout (the process is killed).
pub fn import_gguf(bin: &Path, model_ref: &str, gguf: &Path, scratch: &Path, timeout: Duration) -> Result<(), String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let modelfile = scratch.join(format!("Modelfile-{}", gguf.file_name().and_then(|n| n.to_str()).unwrap_or("m")));
    let from = gguf.to_string_lossy().replace('\\', "/");
    std::fs::write(&modelfile, format!("FROM {from}\n")).map_err(|e| e.to_string())?;
    let result = run_with_timeout(
        orkestr_platform::process::background_command(bin).arg("create").arg(model_ref).arg("-f").arg(&modelfile),
        timeout,
    );
    let _ = std::fs::remove_file(&modelfile);
    result
}

fn run_with_timeout(cmd: &mut std::process::Command, timeout: Duration) -> Result<(), String> {
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ollama: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.by_ref().take(8 * 1024).read_to_string(&mut err);
                }
                return Err(format!("ollama create failed ({status}): {}", err.trim()));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("ollama create timed out".into());
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Whether Ollama at `endpoint` lists `name`.
#[must_use]
pub fn has_model(endpoint: &str, name: &str, timeout: Duration) -> bool {
    discover(endpoint, timeout).models.iter().any(|m| m.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const TAGS: &str = r#"{"models":[{"name":"llama3.1:8b","model":"llama3.1:8b","size":4920753328,
        "details":{"family":"llama","parameter_size":"8.0B","quantization_level":"Q4_K_M"}},
        {"name":"nomic-embed-text:latest","size":274302450,"details":{}}]}"#;

    #[test]
    fn env_endpoint() {
        assert_eq!(endpoint_from_env(None), DEFAULT_ENDPOINT);
        assert_eq!(endpoint_from_env(Some("0.0.0.0")), "127.0.0.1:11434");
        assert_eq!(endpoint_from_env(Some("http://localhost:9999/")), "127.0.0.1:9999");
        assert_eq!(endpoint_from_env(Some("10.0.0.7:11434")), DEFAULT_ENDPOINT, "remote hosts are ignored");
    }

    #[test]
    fn parses_tags() {
        let m = parse_tags(TAGS);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "llama3.1:8b");
        assert_eq!(m[0].parameter_size.as_deref(), Some("8.0B"));
        assert_eq!(m[0].quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(m[1].family, None);
        assert!(parse_tags("{}").is_empty());
        assert!(parse_tags("garbage").is_empty());
    }

    #[test]
    fn discovers_from_a_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let n = s.read(&mut req).unwrap();
            assert!(String::from_utf8_lossy(&req[..n]).starts_with("GET /api/tags HTTP/1.0"));
            let resp = format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{TAGS}");
            s.write_all(resp.as_bytes()).unwrap();
        });
        let info = discover(&endpoint, Duration::from_secs(3));
        server.join().unwrap();
        assert!(info.ollama_reachable);
        assert_eq!(info.models.len(), 2);
    }

    #[test]
    fn binary_detection() {
        let dir = tempfile::tempdir().unwrap();
        // Absent: empty PATH dir, explicit disable, missing override.
        assert_eq!(find_binary_with(None, Some(dir.path().as_os_str().to_owned())), None);
        assert_eq!(find_binary_with(Some("none".into()), None), None);
        assert_eq!(find_binary_with(Some(dir.path().join("nope").into_os_string()), None), None);
        let name = if cfg!(windows) { "ollama.exe" } else { "ollama" };
        std::fs::write(dir.path().join(name), b"").unwrap();
        assert_eq!(find_binary_with(None, Some(dir.path().as_os_str().to_owned())), Some(dir.path().join(name)));
    }

    #[test]
    fn unreachable_server() {
        // Bind then drop to get a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let info = discover(&format!("127.0.0.1:{port}"), Duration::from_millis(500));
        assert!(!info.ollama_reachable);
        assert!(info.models.is_empty());
    }
}
