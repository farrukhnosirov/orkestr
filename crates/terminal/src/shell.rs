//! Shell catalog and argv construction (§15, §46).
//!
//! Shells come from `orkestr-platform`'s [`ShellDetector`] (no hardcoded
//! paths). Each detected shell gets a stable string id (`pwsh`, `powershell`,
//! `cmd`, `gitbash`, `wsl`, `bash`, `zsh`, `fish`, `sh`) that clients use to
//! pick a shell; custom shells are given as program + args.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use orkestr_platform::{ShellDetector, ShellInfo};

use crate::error::{TerminalError, TerminalResult};

/// Shell family (platform kinds plus `Custom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShellKind {
    /// PowerShell 7+ (`pwsh`).
    PowerShell7,
    /// Windows PowerShell 5.1.
    WindowsPowerShell,
    /// `cmd.exe`.
    Cmd,
    /// Git for Windows bash.
    GitBash,
    /// WSL.
    Wsl,
    /// bash.
    Bash,
    /// zsh.
    Zsh,
    /// fish.
    Fish,
    /// POSIX sh.
    Sh,
    /// User-specified program of unknown family.
    Custom,
}

impl From<orkestr_platform::ShellKind> for ShellKind {
    fn from(k: orkestr_platform::ShellKind) -> Self {
        use orkestr_platform::ShellKind as P;
        match k {
            P::PowerShell7 => Self::PowerShell7,
            P::WindowsPowerShell => Self::WindowsPowerShell,
            P::Cmd => Self::Cmd,
            P::GitBash => Self::GitBash,
            P::Wsl => Self::Wsl,
            P::Bash => Self::Bash,
            P::Zsh => Self::Zsh,
            P::Fish => Self::Fish,
            P::Sh => Self::Sh,
        }
    }
}

impl ShellKind {
    /// Stable id slug.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::PowerShell7 => "pwsh",
            Self::WindowsPowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::GitBash => "gitbash",
            Self::Wsl => "wsl",
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Sh => "sh",
            Self::Custom => "custom",
        }
    }

    /// Classifies a program by file name (`Custom` when unknown).
    #[must_use]
    pub fn classify(program: &Path) -> Self {
        orkestr_platform::shell::classify(program).map_or(Self::Custom, Self::from)
    }
}

/// A shell offered to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOption {
    /// Stable id (`pwsh`, `cmd`, …).
    pub id: String,
    /// Family.
    pub kind: ShellKind,
    /// Display name.
    pub name: String,
    /// Executable.
    pub program: PathBuf,
    /// Interactive arguments.
    pub args: Vec<String>,
    /// Platform default.
    pub is_default: bool,
}

/// Which shell to use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShellSpec {
    /// The platform default shell.
    #[default]
    Default,
    /// A detected shell by id.
    Id(String),
    /// A custom program with interactive arguments.
    Custom {
        /// Program (absolute path or name on PATH).
        program: PathBuf,
        /// Arguments.
        args: Vec<String>,
    },
}

/// A shell resolved for spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShell {
    /// Shell id (`custom` for custom programs).
    pub id: String,
    /// Family.
    pub kind: ShellKind,
    /// Display name.
    pub name: String,
    /// Executable.
    pub program: PathBuf,
    /// Interactive arguments.
    pub args: Vec<String>,
}

/// A non-interactive command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// Executable.
    pub program: PathBuf,
    /// Arguments (quoted by the OS layer).
    pub args: Vec<OsString>,
    /// Verbatim tail appended without quoting (Windows `cmd /s /c "…"` only).
    pub raw_tail: Option<String>,
}

/// The detected shells.
#[derive(Debug, Clone, Default)]
pub struct ShellCatalog {
    shells: Vec<ShellOption>,
}

impl ShellCatalog {
    /// Builds the catalog from a detector.
    #[must_use]
    pub fn detect(detector: &dyn ShellDetector) -> Self {
        Self::from_infos(detector.detect())
    }

    /// Builds the catalog from detected shells (default first, unique ids).
    #[must_use]
    pub fn from_infos(infos: Vec<ShellInfo>) -> Self {
        let mut shells: Vec<ShellOption> = Vec::with_capacity(infos.len());
        for info in infos {
            let kind = ShellKind::from(info.kind);
            let base = kind.slug();
            let mut id = base.to_owned();
            let mut n = 2;
            while shells.iter().any(|s| s.id == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            shells.push(ShellOption {
                id,
                kind,
                name: info.name,
                program: info.path,
                args: info.args,
                is_default: info.is_default,
            });
        }
        if !shells.iter().any(|s| s.is_default) {
            if let Some(first) = shells.first_mut() {
                first.is_default = true;
            }
        }
        shells.sort_by_key(|s| !s.is_default);
        Self { shells }
    }

    /// All shells, default first.
    #[must_use]
    pub fn shells(&self) -> &[ShellOption] {
        &self.shells
    }

    /// Looks up a shell by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ShellOption> {
        self.shells.iter().find(|s| s.id == id)
    }

    /// Resolves a [`ShellSpec`].
    pub fn resolve(&self, spec: &ShellSpec) -> TerminalResult<ResolvedShell> {
        let from_option = |s: &ShellOption| ResolvedShell {
            id: s.id.clone(),
            kind: s.kind,
            name: s.name.clone(),
            program: s.program.clone(),
            args: s.args.clone(),
        };
        match spec {
            ShellSpec::Default => self
                .shells
                .iter()
                .find(|s| s.is_default)
                .or_else(|| self.shells.first())
                .map(from_option)
                .ok_or_else(|| TerminalError::NoShell("no shell detected on this system".into())),
            ShellSpec::Id(id) => self.get(id).map(from_option).ok_or_else(|| TerminalError::UnknownShell(id.clone())),
            ShellSpec::Custom { program, args } => {
                if program.as_os_str().is_empty() {
                    return Err(TerminalError::NoShell("custom shell program is empty".into()));
                }
                let resolved = if program.components().count() > 1 || program.is_absolute() {
                    program.is_file().then(|| program.clone())
                } else {
                    orkestr_platform::process::which(&program.to_string_lossy())
                };
                let path = resolved
                    .ok_or_else(|| TerminalError::NoShell(format!("custom shell {} not found", program.display())))?;
                let name = path.file_stem().map_or_else(|| "Custom".to_owned(), |s| s.to_string_lossy().into_owned());
                Ok(ResolvedShell {
                    id: "custom".into(),
                    kind: ShellKind::classify(&path),
                    name,
                    program: path,
                    args: args.clone(),
                })
            }
        }
    }
}

/// Prefix making PowerShell write UTF-8 to redirected pipes.
const PS_UTF8_PREFIX: &str =
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;$OutputEncoding=[System.Text.Encoding]::UTF8;";

/// Builds the argv to run `command` non-interactively in `shell`'s dialect.
pub fn command_line(shell: &ResolvedShell, command: &str) -> TerminalResult<CommandLine> {
    if command.trim().is_empty() {
        return Err(TerminalError::Invalid("command must not be empty".into()));
    }
    if command.contains('\0') {
        return Err(TerminalError::Invalid("command must not contain NUL".into()));
    }
    let program = shell.program.clone();
    let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    Ok(match shell.kind {
        ShellKind::Cmd => {
            // `/s /c "<cmd>"`: cmd strips exactly the outer quotes. `chcp 65001`
            // switches cmd's own output to UTF-8 (only for this process).
            if command.contains(['\r', '\n']) {
                return Err(TerminalError::Invalid("cmd commands must be a single line".into()));
            }
            CommandLine {
                program,
                args: os(&["/d", "/s", "/c"]),
                raw_tail: Some(format!("\"chcp 65001>nul & {command}\"")),
            }
        }
        ShellKind::PowerShell7 | ShellKind::WindowsPowerShell => {
            let mut args = os(&["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
            args.push(OsString::from(format!("{PS_UTF8_PREFIX} {command}")));
            CommandLine { program, args, raw_tail: None }
        }
        ShellKind::GitBash | ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish | ShellKind::Sh => {
            let mut args = os(&["-c"]);
            args.push(OsString::from(command));
            CommandLine { program, args, raw_tail: None }
        }
        ShellKind::Wsl => {
            let mut args = os(&["-e", "sh", "-c"]);
            args.push(OsString::from(command));
            CommandLine { program, args, raw_tail: None }
        }
        ShellKind::Custom => {
            return Err(TerminalError::NoShell(format!(
                "custom shell {} has an unknown dialect; pick a detected shell for commands",
                shell.program.display()
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::ShellKind as P;

    fn info(kind: P, name: &str, path: &str, default: bool) -> ShellInfo {
        ShellInfo { kind, name: name.into(), path: PathBuf::from(path), args: vec![], is_default: default }
    }

    #[test]
    fn catalog_assigns_unique_ids_and_default_first() {
        let cat = ShellCatalog::from_infos(vec![
            info(P::Bash, "bash", "/bin/bash", false),
            info(P::Bash, "bash", "/usr/local/bin/bash", false),
            info(P::Zsh, "zsh", "/bin/zsh", true),
        ]);
        let ids: Vec<_> = cat.shells().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["zsh", "bash", "bash-2"]);
        assert_eq!(cat.resolve(&ShellSpec::Default).unwrap().id, "zsh");
        assert_eq!(cat.resolve(&ShellSpec::Id("bash-2".into())).unwrap().program, PathBuf::from("/usr/local/bin/bash"));
        assert!(matches!(cat.resolve(&ShellSpec::Id("fish".into())), Err(TerminalError::UnknownShell(_))));
    }

    #[test]
    fn catalog_without_default_promotes_first() {
        let cat = ShellCatalog::from_infos(vec![info(P::Sh, "sh", "/bin/sh", false)]);
        assert!(cat.shells()[0].is_default);
        assert!(ShellCatalog::default().resolve(&ShellSpec::Default).is_err());
    }

    #[test]
    fn real_detection_finds_a_shell() {
        let cat = ShellCatalog::detect(&orkestr_platform::NativeShellDetector::default());
        assert!(cat.resolve(&ShellSpec::Default).is_ok());
    }

    #[test]
    fn custom_shell_must_exist() {
        let err = ShellCatalog::default()
            .resolve(&ShellSpec::Custom { program: PathBuf::from("definitely-missing-shell-xyz"), args: vec![] });
        assert!(matches!(err, Err(TerminalError::NoShell(_))));
    }

    fn resolved(kind: ShellKind, program: &str) -> ResolvedShell {
        ResolvedShell { id: kind.slug().into(), kind, name: "x".into(), program: program.into(), args: vec![] }
    }

    #[test]
    fn command_lines_per_dialect() {
        let c = command_line(&resolved(ShellKind::Cmd, "cmd.exe"), "echo hi").unwrap();
        assert_eq!(c.args, vec![OsString::from("/d"), OsString::from("/s"), OsString::from("/c")]);
        assert_eq!(c.raw_tail.as_deref(), Some("\"chcp 65001>nul & echo hi\""));

        let p = command_line(&resolved(ShellKind::PowerShell7, "pwsh"), "Get-Date").unwrap();
        assert_eq!(p.args[3], OsString::from("-Command"));
        assert!(p.args[4].to_string_lossy().ends_with("Get-Date"));

        let b = command_line(&resolved(ShellKind::Bash, "/bin/bash"), "ls -la").unwrap();
        assert_eq!(b.args, vec![OsString::from("-c"), OsString::from("ls -la")]);

        let w = command_line(&resolved(ShellKind::Wsl, "wsl.exe"), "uname").unwrap();
        assert_eq!(w.args.len(), 4);

        assert!(command_line(&resolved(ShellKind::Bash, "/bin/bash"), "  ").is_err());
        assert!(command_line(&resolved(ShellKind::Cmd, "cmd.exe"), "a\nb").is_err());
        assert!(command_line(&resolved(ShellKind::Custom, "/opt/x"), "a").is_err());
    }
}
