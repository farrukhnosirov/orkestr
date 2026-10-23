//! Server command line, UI URL and output parsing.
//!
//! Security invariants (tested):
//! * the server binds `127.0.0.1` only (never `0.0.0.0` / `localhost`, which
//!   may resolve to `::` on some hosts);
//! * a random 256-bit connection token is always required — passed through
//!   `--connection-token-file` (a user-only file), so it never appears in the
//!   process list; `--without-connection-token` is never used;
//! * telemetry is off.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rand::RngCore;

/// Loopback address the server binds to.
pub const BIND_HOST: &str = "127.0.0.1";

/// Everything needed to start the server.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Unpacked package directory.
    pub install_dir: PathBuf,
    /// Bundled Node.js executable.
    pub node: PathBuf,
    /// Port (0 = the server picks a free one).
    pub port: u16,
    /// File holding the connection token.
    pub token_file: PathBuf,
    /// `--server-data-dir`.
    pub server_data_dir: PathBuf,
    /// `--user-data-dir`.
    pub user_data_dir: PathBuf,
    /// `--extensions-dir`.
    pub extensions_dir: PathBuf,
}

/// Arguments for `node` (the first one is `out/server-main.js`).
#[must_use]
pub fn build_args(spec: &LaunchSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![spec.install_dir.join("out").join("server-main.js").into_os_string()];
    let mut push = |flag: &str, value: OsString| {
        args.push(flag.into());
        args.push(value);
    };
    push("--host", BIND_HOST.into());
    push("--port", spec.port.to_string().into());
    push("--connection-token-file", spec.token_file.clone().into_os_string());
    push("--server-data-dir", spec.server_data_dir.clone().into_os_string());
    push("--user-data-dir", spec.user_data_dir.clone().into_os_string());
    push("--extensions-dir", spec.extensions_dir.clone().into_os_string());
    push("--telemetry-level", "off".into());
    args.push("--accept-server-license-terms".into());
    args
}

/// Server flags for VS Code's Workspace Trust (ADR-0043): without trust the
/// workbench starts every folder trusted (no Restricted Mode).
#[must_use]
pub fn trust_args(workspace_trust: bool) -> Vec<OsString> {
    if workspace_trust {
        Vec::new()
    } else {
        vec!["--disable-workspace-trust".into()]
    }
}

/// A fresh connection token: 256 random bits, hex (the server accepts `[0-9A-Za-z-]`).
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Port announced by a server output line
/// (`Extension host agent listening on 9900` / `Server bound to 127.0.0.1:9900 (IPv4)`).
#[must_use]
pub fn parse_listening_port(line: &str) -> Option<u16> {
    if let Some(rest) = line.split("Extension host agent listening on ").nth(1) {
        return rest.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok().filter(|p| *p != 0);
    }
    if let Some(rest) = line.split("Server bound to 127.0.0.1:").nth(1) {
        return rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok().filter(|p| *p != 0);
    }
    None
}

/// `http://127.0.0.1:<port>`.
#[must_use]
pub fn origin(port: u16) -> String {
    format!("http://{BIND_HOST}:{port}")
}

/// The UI URL: `http://127.0.0.1:<port>/?tkn=<token>&folder=<path>`. The
/// server swaps the `tkn` query for a cookie on the first request.
#[must_use]
pub fn ui_url(port: u16, token: &str, folder: Option<&str>) -> String {
    let mut url = format!("{}/?tkn={}", origin(port), encode_query(token));
    if let Some(folder) = folder {
        url.push_str("&folder=");
        url.push_str(&encode_query(folder));
    }
    url
}

/// A local path as a VS Code URI path: `C:\x\y` → `/c:/x/y`, `/home/x` stays.
#[must_use]
pub fn folder_uri_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_owned();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        format!("/{drive}{}", &s[1..])
    } else {
        s
    }
}

/// Percent-encodes a query value (keeps unreserved characters, `/` and `:`).
#[must_use]
pub fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Replaces the token everywhere in a server output line.
#[must_use]
pub fn redact_line(line: &str, token: Option<&str>) -> String {
    let line = match token {
        Some(t) if !t.is_empty() => line.replace(t, orkestr_security::redact::REDACTED),
        _ => line.to_owned(),
    };
    orkestr_security::redact::redact(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            install_dir: PathBuf::from("/srv/1.0"),
            node: PathBuf::from("/srv/1.0/node"),
            port: 0,
            token_file: PathBuf::from("/srv/connection-token"),
            server_data_dir: PathBuf::from("/srv/server-data"),
            user_data_dir: PathBuf::from("/srv/user-data"),
            extensions_dir: PathBuf::from("/srv/extensions"),
        }
    }

    fn value_of(args: &[OsString], flag: &str) -> Option<String> {
        let i = args.iter().position(|a| a == flag)?;
        args.get(i + 1).map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn binds_loopback_with_a_token_file() {
        let args = build_args(&spec());
        assert!(args[0].to_string_lossy().ends_with("server-main.js"));
        assert_eq!(value_of(&args, "--host").as_deref(), Some("127.0.0.1"));
        assert_eq!(value_of(&args, "--port").as_deref(), Some("0"));
        assert!(value_of(&args, "--connection-token-file").unwrap().ends_with("connection-token"));
        assert_eq!(value_of(&args, "--telemetry-level").as_deref(), Some("off"));
        assert!(value_of(&args, "--extensions-dir").unwrap().ends_with("extensions"));
        assert!(value_of(&args, "--user-data-dir").unwrap().ends_with("user-data"));
        let all: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!all.iter().any(|a| a == "--without-connection-token"), "the token is never disabled");
        assert!(!all.iter().any(|a| a == "--connection-token"), "the token never appears on the command line");
        assert!(!all.iter().any(|a| a == "0.0.0.0" || a == "::" || a == "localhost"));
    }

    #[test]
    fn workspace_trust_flag() {
        assert_eq!(trust_args(false), vec![OsString::from("--disable-workspace-trust")]);
        assert!(trust_args(true).is_empty());
    }

    #[test]
    fn tokens_are_random_256_bit_hex() {
        let (a, b) = (generate_token(), generate_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parses_the_listening_port() {
        assert_eq!(parse_listening_port("Extension host agent listening on 9900"), Some(9900));
        assert_eq!(parse_listening_port("Server bound to 127.0.0.1:3623 (IPv4)"), Some(3623));
        assert_eq!(parse_listening_port("[19:06:14] Extension host agent started."), None);
        assert_eq!(parse_listening_port("Extension host agent listening on 0"), None);
        assert_eq!(parse_listening_port("Server bound to 0.0.0.0:80"), None);
    }

    #[test]
    fn builds_the_ui_url() {
        assert_eq!(ui_url(9900, "abc", None), "http://127.0.0.1:9900/?tkn=abc");
        assert_eq!(
            ui_url(9900, "abc", Some("/c:/My Projects/a&b")),
            "http://127.0.0.1:9900/?tkn=abc&folder=/c:/My%20Projects/a%26b"
        );
    }

    #[test]
    fn converts_local_paths_to_uri_paths() {
        assert_eq!(folder_uri_path(Path::new("C:\\Users\\me\\proj")), "/c:/Users/me/proj");
        assert_eq!(folder_uri_path(Path::new("\\\\?\\D:\\x")), "/d:/x");
        assert_eq!(folder_uri_path(Path::new("/home/me/proj")), "/home/me/proj");
    }

    #[test]
    fn redacts_the_token_from_output() {
        let line = "Web UI available at http://localhost:9900?tkn=0123abcd";
        let out = redact_line(line, Some("0123abcd"));
        assert!(!out.contains("0123abcd"), "{out}");
        assert!(out.contains("localhost:9900"));
    }
}
