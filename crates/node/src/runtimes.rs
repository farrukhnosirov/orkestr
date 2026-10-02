//! Developer runtime detection (git, node, npm, python, rustc, cargo, go,
//! dotnet, java, docker, wsl) with versions.
//!
//! Every probe resolves the executable on `PATH` and runs its version flag
//! through the platform process API with a timeout; probes run in parallel.
//! Windows Store "app execution alias" stubs for Python (which print an
//! install hint and exit non-zero) are reported as unavailable.

use std::time::Duration;

use orkestr_platform::ProcessControl;
use orkestr_protocol::methods::nodes::RuntimeInfo;

/// A runtime probe.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeProbe {
    /// Stable id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Executable names tried in order.
    pub candidates: &'static [&'static str],
    /// Version arguments.
    pub args: &'static [&'static str],
    /// Only probed on Windows (WSL).
    pub windows_only: bool,
}

/// Probes in display order.
pub const RUNTIME_PROBES: &[RuntimeProbe] = &[
    RuntimeProbe { id: "git", name: "Git", candidates: &["git"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "node", name: "Node.js", candidates: &["node"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "npm", name: "npm", candidates: &["npm"], args: &["--version"], windows_only: false },
    RuntimeProbe {
        id: "python",
        name: "Python",
        candidates: &["python", "python3", "py"],
        args: &["--version"],
        windows_only: false,
    },
    RuntimeProbe { id: "rustc", name: "Rust", candidates: &["rustc"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "cargo", name: "Cargo", candidates: &["cargo"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "go", name: "Go", candidates: &["go"], args: &["version"], windows_only: false },
    RuntimeProbe { id: "dotnet", name: ".NET SDK", candidates: &["dotnet"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "java", name: "Java", candidates: &["java"], args: &["-version"], windows_only: false },
    RuntimeProbe { id: "docker", name: "Docker", candidates: &["docker"], args: &["--version"], windows_only: false },
    RuntimeProbe { id: "wsl", name: "WSL", candidates: &["wsl"], args: &["--version"], windows_only: true },
];

/// Extracts the first dotted version number (`v22.1.0` → `22.1.0`,
/// `go version go1.22.3 windows/amd64` → `1.22.3`).
#[must_use]
pub fn parse_version(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|t| t.trim_matches('.'))
        .find(|t| {
            let mut parts = t.split('.');
            t.contains('.') && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|t| t.split('.').take(4).collect::<Vec<_>>().join("."))
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().chars().take(160).collect()
}

/// Runs one probe.
#[must_use]
pub fn probe(process: &dyn ProcessControl, p: &RuntimeProbe, timeout: Duration) -> RuntimeInfo {
    let mut info = RuntimeInfo {
        id: p.id.to_owned(),
        name: p.name.to_owned(),
        available: false,
        version: None,
        path: None,
        detail: Some("not found on PATH".into()),
    };
    for candidate in p.candidates {
        let Some(path) = process.which(candidate) else { continue };
        info.path = Some(path.display().to_string());
        match process.run(&path, p.args, timeout) {
            Ok(out) if out.success => {
                info.available = true;
                info.version = parse_version(out.text());
                info.detail = None;
                return info;
            }
            Ok(out) => {
                let msg = first_line(out.text());
                info.detail = Some(if msg.is_empty() {
                    format!("`{candidate}` exited with code {}", out.code.map_or_else(|| "?".into(), |c| c.to_string()))
                } else {
                    msg
                });
            }
            Err(e) => info.detail = Some(e.to_string()),
        }
    }
    info
}

/// Runs every applicable probe in parallel.
#[must_use]
pub fn detect_runtimes(process: &dyn ProcessControl, timeout: Duration) -> Vec<RuntimeInfo> {
    let probes: Vec<&RuntimeProbe> = RUNTIME_PROBES.iter().filter(|p| !p.windows_only || cfg!(windows)).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = probes.iter().map(|p| s.spawn(move || probe(process, p, timeout))).collect();
        handles
            .into_iter()
            .zip(&probes)
            .map(|(h, p)| {
                h.join().unwrap_or_else(|_| RuntimeInfo {
                    id: p.id.into(),
                    name: p.name.into(),
                    available: false,
                    version: None,
                    path: None,
                    detail: Some("probe panicked".into()),
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::NativeProcessControl;

    #[test]
    fn version_parsing() {
        let cases = [
            ("git version 2.45.1.windows.1", Some("2.45.1")),
            ("v22.1.0", Some("22.1.0")),
            ("10.7.0", Some("10.7.0")),
            ("Python 3.12.1", Some("3.12.1")),
            ("rustc 1.99.0 (b940084d7 2026-09-28)", Some("1.99.0")),
            ("go version go1.22.3 windows/amd64", Some("1.22.3")),
            ("openjdk version \"21.0.2\" 2024-01-16", Some("21.0.2")),
            ("Docker version 26.1.1, build 4cf5afa", Some("26.1.1")),
            ("WSL version: 2.1.5.0\nKernel version: 5.15.146.1-2", Some("2.1.5.0")),
            ("no digits here", None),
            ("version 7", None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_version(input).as_deref(), want, "{input}");
        }
        assert_eq!(parse_version("1.2.3.4.5").as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn missing_runtime_is_unavailable() {
        let p = RuntimeProbe {
            id: "nope",
            name: "Nope",
            candidates: &["definitely-not-a-real-binary-orkestr"],
            args: &["--version"],
            windows_only: false,
        };
        let r = probe(&NativeProcessControl, &p, Duration::from_secs(2));
        assert!(!r.available);
        assert_eq!(r.detail.as_deref(), Some("not found on PATH"));
    }

    #[test]
    fn detects_in_parallel() {
        let all = detect_runtimes(&NativeProcessControl, Duration::from_secs(15));
        assert!(all.len() >= 10);
        assert_eq!(all[0].id, "git");
        // cargo runs these tests, so it must be found with a version.
        let cargo = all.iter().find(|r| r.id == "cargo").unwrap();
        assert!(cargo.available, "{cargo:?}");
        assert!(cargo.version.is_some());
    }
}
