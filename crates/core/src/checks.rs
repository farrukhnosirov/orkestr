//! Environment checks for onboarding (§55 steps 7–8) and Settings → System:
//! git, terminal shells, data directory, web view runtime, OS keyring.
//!
//! Every check is blocking but bounded (process probes have timeouts) and
//! read-only — the keyring check only *reads* a non-existent entry and the
//! data-dir check writes and removes a temporary file.

use std::time::{Duration, Instant};

use orkestr_domain::OsKind;
use orkestr_platform::{PlatformAdapter, PlatformError};
use orkestr_protocol::methods::system::{CheckStatus, SystemCheck, SystemChecksResult};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// Warn when the data volume has less free space than this.
pub const LOW_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn check(id: &str, title: &str, status: CheckStatus, detail: impl Into<String>) -> SystemCheck {
    SystemCheck { id: id.into(), title: title.into(), status, detail: detail.into(), hint: None, version: None }
}

fn with_hint(mut c: SystemCheck, hint: impl Into<String>) -> SystemCheck {
    c.hint = Some(hint.into());
    c
}

fn git_install_hint(os: OsKind) -> &'static str {
    match os {
        OsKind::Windows => "Install Git for Windows (https://git-scm.com/download/win) or `winget install Git.Git`, then restart Orkestr.",
        OsKind::MacOs => "Run `xcode-select --install` or `brew install git`, then restart Orkestr.",
        _ => "Install git with your package manager (e.g. `sudo apt install git`), then restart Orkestr.",
    }
}

/// `git` on PATH and answering `--version`.
#[must_use]
pub fn git_check(platform: &dyn PlatformAdapter) -> SystemCheck {
    let os = platform.info().os;
    let Some(path) = platform.process().which("git") else {
        return with_hint(check("git", "Git", CheckStatus::Error, "git was not found on PATH"), git_install_hint(os));
    };
    match platform.process().run(&path, &["--version"], PROBE_TIMEOUT) {
        Ok(out) if out.success => {
            let version = orkestr_node::runtimes::parse_version(out.text());
            let mut c = check(
                "git",
                "Git",
                CheckStatus::Ok,
                format!("git {} at {}", version.as_deref().unwrap_or("?"), path.display()),
            );
            c.version = version;
            c
        }
        Ok(out) => with_hint(
            check("git", "Git", CheckStatus::Error, format!("`git --version` failed: {}", out.text().trim())),
            git_install_hint(os),
        ),
        Err(e) => {
            with_hint(check("git", "Git", CheckStatus::Error, format!("git did not answer: {e}")), git_install_hint(os))
        }
    }
}

/// At least one interactive shell for the terminal panel.
#[must_use]
pub fn shell_check(platform: &dyn PlatformAdapter) -> SystemCheck {
    let shells = platform.shells().detect();
    if shells.is_empty() {
        return with_hint(
            check("shell", "Terminal shell", CheckStatus::Error, "no supported shell was found"),
            "Install PowerShell 7 (`winget install Microsoft.PowerShell`) or make sure cmd/bash is available.",
        );
    }
    let names: Vec<String> =
        shells.iter().map(|s| if s.is_default { format!("{} (default)", s.name) } else { s.name.clone() }).collect();
    check("shell", "Terminal shell", CheckStatus::Ok, names.join(", "))
}

/// The data directory is writable and has free space.
#[must_use]
pub fn data_dir_check(platform: &dyn PlatformAdapter) -> SystemCheck {
    let dir = &platform.paths().data_dir;
    let probe = dir.join(format!(".write-check-{}", std::process::id()));
    let writable = std::fs::write(&probe, b"ok").and_then(|()| std::fs::remove_file(&probe));
    if let Err(e) = writable {
        return with_hint(
            check("dataDir", "Data directory", CheckStatus::Error, format!("{} is not writable: {e}", dir.display())),
            "Check folder permissions or set ORKESTR_HOME to a writable folder.",
        );
    }
    let snap = platform.resources().snapshot();
    let free_gb = snap.disk_free_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let detail = format!("{} — {free_gb:.1} GB free", dir.display());
    if snap.disk_total_bytes > 0 && snap.disk_free_bytes < LOW_DISK_BYTES {
        return with_hint(
            check("dataDir", "Data directory", CheckStatus::Warning, detail),
            "Free up disk space: snapshots and worktrees need room.",
        );
    }
    check("dataDir", "Data directory", CheckStatus::Ok, detail)
}

/// Web view runtime (the desktop UI needs it; headless cores do not).
#[must_use]
pub fn webview_check(platform: &dyn PlatformAdapter) -> SystemCheck {
    let os = platform.info().os;
    match orkestr_platform::detect_webview() {
        Some(rt) => {
            let mut c = check(
                "webview",
                "Web view runtime",
                CheckStatus::Ok,
                format!("{} {}", rt.engine, rt.version.as_deref().unwrap_or("(system)")),
            );
            c.version = rt.version;
            c
        }
        None => {
            let (status, hint) = match os {
                OsKind::Windows => (
                    CheckStatus::Error,
                    "Install the Microsoft Edge WebView2 Runtime (https://go.microsoft.com/fwlink/p/?LinkId=2124703).",
                ),
                OsKind::Linux => {
                    (CheckStatus::Warning, "Install WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`) for the desktop app.")
                }
                _ => (CheckStatus::Skipped, "Not required for a headless core."),
            };
            with_hint(check("webview", "Web view runtime", status, "no web view runtime detected"), hint)
        }
    }
}

/// The OS credential store answers (read-only probe of a non-existent entry).
#[must_use]
pub fn keyring_check(platform: &dyn PlatformAdapter) -> SystemCheck {
    let store = match platform.info().os {
        OsKind::Windows => "Windows Credential Manager",
        OsKind::MacOs => "macOS Keychain",
        OsKind::Linux => "Linux kernel keyring",
        OsKind::Other => "OS keyring",
    };
    match platform.credentials().get("orkestr-availability-probe", "probe") {
        Ok(_) => check("keyring", "Secure credential store", CheckStatus::Ok, format!("{store} is available")),
        Err(PlatformError::NotSupported { hint, .. }) => {
            with_hint(check("keyring", "Secure credential store", CheckStatus::Warning, format!("{store} is not supported")), hint)
        }
        Err(e) => with_hint(
            check("keyring", "Secure credential store", CheckStatus::Error, format!("{store} failed: {e}")),
            "API keys cannot be stored securely until the credential store works; an encrypted file vault is used as fallback.",
        ),
    }
}

/// Runs all checks in parallel. Blocking.
#[must_use]
pub fn run_all(platform: &dyn PlatformAdapter) -> SystemChecksResult {
    let started = Instant::now();
    let checks = std::thread::scope(|s| {
        let git = s.spawn(|| git_check(platform));
        let shell = s.spawn(|| shell_check(platform));
        let webview = s.spawn(|| webview_check(platform));
        let keyring = s.spawn(|| keyring_check(platform));
        let data = data_dir_check(platform);
        let join = |h: std::thread::ScopedJoinHandle<'_, SystemCheck>, id: &str, title: &str| {
            h.join().unwrap_or_else(|_| check(id, title, CheckStatus::Error, "check panicked"))
        };
        vec![
            join(git, "git", "Git"),
            join(shell, "shell", "Terminal shell"),
            data,
            join(webview, "webview", "Web view runtime"),
            join(keyring, "keyring", "Secure credential store"),
        ]
    });
    SystemChecksResult { checks, duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use orkestr_platform::{MemoryCredentialStore, NativePlatform, Paths};

    #[test]
    fn checks_run_on_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(dir.path());
        paths.ensure().unwrap();
        let p = NativePlatform::new(paths).with_credentials(Arc::new(MemoryCredentialStore::default()));
        let r = run_all(&p);
        let ids: Vec<&str> = r.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["git", "shell", "dataDir", "webview", "keyring"]);
        let by = |id: &str| r.checks.iter().find(|c| c.id == id).unwrap();
        assert_ne!(by("dataDir").status, CheckStatus::Error, "{:?}", by("dataDir"));
        assert_eq!(by("shell").status, CheckStatus::Ok);
        assert_eq!(by("keyring").status, CheckStatus::Ok);
        // The test machine builds with git-based tooling.
        if p.process().which("git").is_some() {
            assert_eq!(by("git").status, CheckStatus::Ok);
            assert!(by("git").version.is_some());
        }
    }

    #[test]
    fn unwritable_data_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // data_dir points at a file → writing a child fails.
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let p = NativePlatform::new(Paths::from_root(&file));
        let c = data_dir_check(&p);
        assert_eq!(c.status, CheckStatus::Error);
        assert!(c.hint.is_some());
    }
}
