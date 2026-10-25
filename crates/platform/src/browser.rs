//! Opening URLs in the user's default web browser.
//!
//! Only `http://` and `https://` URLs are accepted (never `file:`, `javascript:`
//! or custom schemes, which could launch arbitrary handlers). The URL is
//! passed as a single argument — never through a shell — so `&`, quotes and
//! spaces cannot inject commands.

use crate::error::{PlatformError, PlatformResult};

/// Validates that `url` is an absolute `http(s)` URL without control or
/// whitespace characters (the form [`open_url`] accepts).
pub fn validate_web_url(url: &str) -> PlatformResult<()> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
        .ok_or_else(|| PlatformError::Invalid("only http(s) URLs can be opened in the browser".into()))?;
    if rest.is_empty() || rest.starts_with('/') {
        return Err(PlatformError::Invalid("the URL has no host".into()));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"') {
        return Err(PlatformError::Invalid("the URL contains invalid characters".into()));
    }
    if url.len() > 8192 {
        return Err(PlatformError::Invalid("the URL is too long".into()));
    }
    Ok(())
}

/// Opens an `http(s)` URL in the default browser (detached; does not wait).
pub fn open_url(url: &str) -> PlatformResult<()> {
    validate_web_url(url)?;
    spawn_opener(url)
}

#[cfg(windows)]
fn spawn_opener(url: &str) -> PlatformResult<()> {
    // `rundll32 url.dll,FileProtocolHandler <url>` hands the URL to the
    // registered http handler without `cmd /c start` quoting pitfalls.
    let mut cmd = crate::process::background_command("rundll32");
    cmd.arg("url.dll,FileProtocolHandler").arg(url);
    detach(cmd)
}

#[cfg(target_os = "macos")]
fn spawn_opener(url: &str) -> PlatformResult<()> {
    let mut cmd = crate::process::background_command("open");
    cmd.arg(url);
    detach(cmd)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_opener(url: &str) -> PlatformResult<()> {
    let mut cmd = crate::process::background_command("xdg-open");
    cmd.arg(url);
    detach(cmd)
}

#[cfg(not(any(windows, unix)))]
fn spawn_opener(_url: &str) -> PlatformResult<()> {
    Err(PlatformError::NotSupported {
        feature: "opening URLs",
        os: std::env::consts::OS,
        hint: "copy the URL into a browser",
    })
}

#[cfg(any(windows, unix))]
fn detach(mut cmd: std::process::Command) -> PlatformResult<()> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Reap in the background so no zombie is left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_web_urls() {
        assert!(validate_web_url("http://127.0.0.1:8080/?tkn=abc&folder=/c:/x").is_ok());
        assert!(validate_web_url("https://open-vsx.org/extension/a/b").is_ok());
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "ms-settings:",
            "http://",
            "https:///path",
            "http://host/a b",
            "http://host/\"&calc",
            "http://host/\n",
            "",
        ] {
            assert!(validate_web_url(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
