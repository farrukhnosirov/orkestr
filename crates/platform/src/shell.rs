//! Shell detection (§15: PowerShell, CMD, Git Bash, WSL, bash, zsh, fish, custom).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::process::which;

/// Known shell families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ShellKind {
    /// PowerShell 7+ (`pwsh`).
    PowerShell7,
    /// Windows PowerShell 5.1.
    WindowsPowerShell,
    /// `cmd.exe`.
    Cmd,
    /// Git for Windows bash.
    GitBash,
    /// Windows Subsystem for Linux.
    Wsl,
    /// bash.
    Bash,
    /// zsh.
    Zsh,
    /// fish.
    Fish,
    /// POSIX sh / dash / other.
    Sh,
}

/// A detected shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellInfo {
    /// Family.
    pub kind: ShellKind,
    /// Display name ("PowerShell 7", "Git Bash").
    pub name: String,
    /// Executable.
    pub path: PathBuf,
    /// Arguments for an interactive session.
    pub args: Vec<String>,
    /// Whether this is the user's default shell.
    pub is_default: bool,
}

/// Detects installed shells.
pub trait ShellDetector: Send + Sync {
    /// All detected shells, default first.
    fn detect(&self) -> Vec<ShellInfo>;
    /// The default shell, if any was found.
    fn default_shell(&self) -> Option<ShellInfo> {
        self.detect().into_iter().find(|s| s.is_default)
    }
}

/// Native detector.
#[derive(Debug, Default, Clone)]
pub struct NativeShellDetector {
    _private: (),
}

fn shell(kind: ShellKind, name: &str, path: PathBuf, args: &[&str]) -> ShellInfo {
    ShellInfo { kind, name: name.into(), path, args: args.iter().map(|s| (*s).to_owned()).collect(), is_default: false }
}

fn existing(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

#[cfg(windows)]
fn detect_native() -> Vec<ShellInfo> {
    let mut out = Vec::new();
    let system_root = env_path("SystemRoot").or_else(|| env_path("windir"));
    let program_files: Vec<PathBuf> =
        ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"].iter().filter_map(|v| env_path(v)).collect();

    // PowerShell 7
    let pwsh = which("pwsh.exe")
        .or_else(|| program_files.iter().find_map(|pf| existing(pf.join("PowerShell").join("7").join("pwsh.exe"))));
    if let Some(p) = pwsh {
        out.push(shell(ShellKind::PowerShell7, "PowerShell 7", p, &["-NoLogo"]));
    }
    // Windows PowerShell 5.1
    let ps5 = system_root
        .as_ref()
        .and_then(|r| existing(r.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe")))
        .or_else(|| which("powershell.exe"));
    if let Some(p) = ps5 {
        out.push(shell(ShellKind::WindowsPowerShell, "Windows PowerShell", p, &["-NoLogo"]));
    }
    // cmd
    let cmd = env_path("ComSpec")
        .and_then(existing)
        .or_else(|| system_root.as_ref().and_then(|r| existing(r.join("System32").join("cmd.exe"))));
    if let Some(p) = cmd {
        out.push(shell(ShellKind::Cmd, "Command Prompt", p, &[]));
    }
    // Git Bash: derive from git.exe location, never pick System32\bash.exe (WSL launcher).
    if let Some(p) = git_bash(&program_files) {
        out.push(shell(ShellKind::GitBash, "Git Bash", p, &["--login", "-i"]));
    }
    // WSL
    if let Some(p) = system_root.as_ref().and_then(|r| existing(r.join("System32").join("wsl.exe"))) {
        out.push(shell(ShellKind::Wsl, "WSL", p, &[]));
    }
    // Default: PowerShell 7 > Windows PowerShell > cmd.
    if let Some(first) = out
        .iter_mut()
        .find(|s| matches!(s.kind, ShellKind::PowerShell7 | ShellKind::WindowsPowerShell | ShellKind::Cmd))
    {
        first.is_default = true;
    }
    out
}

#[cfg(windows)]
fn git_bash(program_files: &[PathBuf]) -> Option<PathBuf> {
    if let Some(git) = which("git.exe") {
        // <root>\cmd\git.exe, <root>\bin\git.exe or <root>\mingw64\bin\git.exe
        for ancestor in git.ancestors().skip(1).take(3) {
            let candidate = ancestor.join("bin").join("bash.exe");
            if candidate.is_file() && !is_system32(&candidate) {
                return Some(candidate);
            }
        }
    }
    program_files.iter().find_map(|pf| existing(pf.join("Git").join("bin").join("bash.exe")))
}

#[cfg(windows)]
fn is_system32(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str().eq_ignore_ascii_case("System32"))
}

#[cfg(unix)]
fn detect_native() -> Vec<ShellInfo> {
    let mut paths: Vec<PathBuf> = std::fs::read_to_string("/etc/shells")
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(PathBuf::from).collect())
        .unwrap_or_default();
    for name in ["bash", "zsh", "fish", "sh", "pwsh"] {
        if let Some(p) = which(name) {
            paths.push(p);
        }
    }
    let default = env_path("SHELL");
    let mut out: Vec<ShellInfo> = Vec::new();
    let mut seen_kinds = std::collections::HashSet::new();
    for p in paths {
        if !p.is_file() {
            continue;
        }
        let Some(kind) = classify(&p) else { continue };
        let is_default = default.as_deref() == Some(p.as_path());
        if !is_default && !seen_kinds.insert(kind) {
            continue;
        }
        let (name, args): (&str, &[&str]) = match kind {
            ShellKind::Bash => ("bash", &["-l"]),
            ShellKind::Zsh => ("zsh", &["-l"]),
            ShellKind::Fish => ("fish", &["-l"]),
            ShellKind::PowerShell7 => ("PowerShell 7", &["-NoLogo"]),
            _ => ("sh", &[]),
        };
        let mut info = shell(kind, name, p, args);
        info.is_default = is_default;
        out.retain(|s: &ShellInfo| !(s.kind == kind && is_default));
        out.push(info);
    }
    out.sort_by_key(|s| !s.is_default);
    out
}

#[cfg(not(any(windows, unix)))]
fn detect_native() -> Vec<ShellInfo> {
    Vec::new()
}

/// Classifies a shell binary by file name.
#[must_use]
pub fn classify(path: &Path) -> Option<ShellKind> {
    let stem = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
    Some(match stem.as_str() {
        "pwsh" => ShellKind::PowerShell7,
        "powershell" => ShellKind::WindowsPowerShell,
        "cmd" => ShellKind::Cmd,
        "wsl" => ShellKind::Wsl,
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        "fish" => ShellKind::Fish,
        "sh" | "dash" | "ksh" | "mksh" => ShellKind::Sh,
        _ => return None,
    })
}

impl ShellDetector for NativeShellDetector {
    fn detect(&self) -> Vec<ShellInfo> {
        let mut shells = detect_native();
        shells.sort_by_key(|s| !s.is_default);
        shells
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_at_least_one_default_shell() {
        let shells = NativeShellDetector::default().detect();
        assert!(!shells.is_empty(), "no shells detected");
        assert!(shells[0].is_default, "default shell must come first: {shells:?}");
        assert_eq!(shells.iter().filter(|s| s.is_default).count(), 1);
        for s in &shells {
            assert!(s.path.is_file(), "{s:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_has_cmd_and_powershell() {
        let shells = NativeShellDetector::default().detect();
        assert!(shells.iter().any(|s| s.kind == ShellKind::Cmd));
        assert!(shells.iter().any(|s| matches!(s.kind, ShellKind::WindowsPowerShell | ShellKind::PowerShell7)));
        if let Some(gb) = shells.iter().find(|s| s.kind == ShellKind::GitBash) {
            assert!(!gb.path.to_string_lossy().to_ascii_lowercase().contains("system32"));
        }
    }

    #[test]
    fn classify_names() {
        assert_eq!(classify(Path::new("/usr/bin/zsh")), Some(ShellKind::Zsh));
        assert_eq!(classify(Path::new("C:/x/pwsh.exe")), Some(ShellKind::PowerShell7));
        assert_eq!(classify(Path::new("/bin/false")), None);
    }
}
