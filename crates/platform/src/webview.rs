//! Web view runtime detection (onboarding system checks, §55).
//!
//! * Windows: WebView2 Evergreen runtime version from the EdgeUpdate client
//!   registry key (machine-wide 32/64-bit views, then per-user).
//! * macOS: WKWebView ships with the OS; the Safari version is reported.
//! * Linux: WebKitGTK 4.1 version via `pkg-config`.

use std::time::Duration;

use crate::process::{run_with_timeout, which};

/// Detected web view runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebViewRuntime {
    /// Engine name ("WebView2", "WKWebView", "WebKitGTK").
    pub engine: &'static str,
    /// Version, when known.
    pub version: Option<String>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// Detects the web view runtime. `None` when the runtime is missing.
#[must_use]
pub fn detect_webview() -> Option<WebViewRuntime> {
    detect_native()
}

#[cfg(target_os = "windows")]
fn detect_native() -> Option<WebViewRuntime> {
    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let reg = which("reg")?;
    let keys = [
        format!(r"HKLM\SOFTWARE\WOW6432Node\{CLIENT}"),
        format!(r"HKLM\SOFTWARE\{CLIENT}"),
        format!(r"HKCU\Software\{CLIENT}"),
    ];
    keys.iter().find_map(|key| {
        let out = run_with_timeout(&reg, &["query", key, "/v", "pv"], PROBE_TIMEOUT).ok()?;
        if !out.success {
            return None;
        }
        parse_reg_value(&out.stdout).map(|v| WebViewRuntime { engine: "WebView2", version: Some(v) })
    })
}

#[cfg(target_os = "macos")]
fn detect_native() -> Option<WebViewRuntime> {
    let version = which("defaults").and_then(|d| {
        run_with_timeout(
            &d,
            &["read", "/Applications/Safari.app/Contents/Info", "CFBundleShortVersionString"],
            PROBE_TIMEOUT,
        )
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout.trim().to_owned())
        .filter(|s| !s.is_empty())
    });
    Some(WebViewRuntime { engine: "WKWebView", version })
}

#[cfg(target_os = "linux")]
fn detect_native() -> Option<WebViewRuntime> {
    let pkg = which("pkg-config")?;
    ["webkit2gtk-4.1", "webkit2gtk-4.0"].iter().find_map(|lib| {
        let out = run_with_timeout(&pkg, &["--modversion", lib], PROBE_TIMEOUT).ok()?;
        (out.success && !out.stdout.trim().is_empty())
            .then(|| WebViewRuntime { engine: "WebKitGTK", version: Some(out.stdout.trim().to_owned()) })
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn detect_native() -> Option<WebViewRuntime> {
    let _ = (which, run_with_timeout, PROBE_TIMEOUT);
    None
}

/// Parses `reg query … /v pv` output; `0.0.0.0` (uninstalled) yields `None`.
#[cfg(any(target_os = "windows", test))]
fn parse_reg_value(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("REG_SZ"))?;
    let value = line.split("REG_SZ").nth(1)?.trim();
    (!value.is_empty() && value != "0.0.0.0").then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reg_output() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226}\r\n    pv    REG_SZ    129.0.2792.65\r\n\r\n";
        assert_eq!(parse_reg_value(out).as_deref(), Some("129.0.2792.65"));
        assert_eq!(parse_reg_value("    pv    REG_SZ    0.0.0.0"), None);
        assert_eq!(parse_reg_value("ERROR: The system was unable to find the specified registry key"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_has_webview2() {
        // WebView2 is part of Windows 11; on stripped CI images it may be absent.
        if let Some(rt) = detect_webview() {
            assert_eq!(rt.engine, "WebView2");
            assert!(rt.version.is_some());
        }
    }
}
