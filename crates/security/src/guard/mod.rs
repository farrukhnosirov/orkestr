//! Dangerous Action Guard (contract §6, permission model §6).
//!
//! [`DangerousActionGuard::classify`] analyses a command line for a given
//! shell and returns a [`DangerClass`]:
//!
//! * `Safe` — nothing dangerous detected (normal permission rules apply);
//! * `Dangerous{category, reason}` — **always** requires an approval, even
//!   in FullAuto (§23), and is never "rememberable";
//! * `Forbidden{category, reason}` — hard-denied; no rule or approval can
//!   allow it.
//!
//! The pipeline: raw-text checks (fork bombs, credential theft, unsafe agent
//! CLI flags) → lexing ([`lexer`]) into simple commands, pipelines,
//! redirections and nested substitutions → wrapper unwrapping (`sudo`,
//! `env`, `bash -c`, `cmd /c`, `powershell -Command`/`-EncodedCommand`,
//! `wsl`, `Invoke-Expression`, `npx`, …, max depth [`MAX_DEPTH`]) →
//! per-program rules ([`rules`]). The overall class is the maximum over all
//! parts. Anything unparseable fails closed (`Dangerous(Unparseable)`).

mod facets;
pub mod lexer;
mod rules;
pub mod sql;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use orkestr_domain::DangerCategory;
use serde::{Deserialize, Serialize};

pub use facets::CommandFacets;
pub use lexer::Family;
pub use sql::classify_sql;

/// Maximum unwrap/nesting depth before a command counts as obfuscated.
pub const MAX_DEPTH: usize = 4;
/// Longest command line analysed; longer input is `Dangerous(Unparseable)`.
pub const MAX_COMMAND_LEN: usize = 64 * 1024;

/// Classification result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all_fields = "camelCase")]
pub enum DangerClass {
    /// Nothing dangerous detected.
    Safe,
    /// Requires an approval, always (non-rememberable).
    Dangerous {
        /// Category.
        category: DangerCategory,
        /// Human-readable reason.
        reason: String,
    },
    /// Hard-denied.
    Forbidden {
        /// Category.
        category: DangerCategory,
        /// Human-readable reason.
        reason: String,
    },
}

impl DangerClass {
    /// Severity rank: Safe 0, Dangerous 1, Forbidden 2.
    #[must_use]
    pub fn rank(&self) -> u8 {
        match self {
            DangerClass::Safe => 0,
            DangerClass::Dangerous { .. } => 1,
            DangerClass::Forbidden { .. } => 2,
        }
    }

    /// Whether this is `Safe`.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self, DangerClass::Safe)
    }

    /// Whether this is `Dangerous`.
    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        matches!(self, DangerClass::Dangerous { .. })
    }

    /// Whether this is `Forbidden`.
    #[must_use]
    pub fn is_forbidden(&self) -> bool {
        matches!(self, DangerClass::Forbidden { .. })
    }

    /// Category, if not safe.
    #[must_use]
    pub fn category(&self) -> Option<DangerCategory> {
        match self {
            DangerClass::Safe => None,
            DangerClass::Dangerous { category, .. } | DangerClass::Forbidden { category, .. } => Some(*category),
        }
    }

    /// Reason, if not safe.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            DangerClass::Safe => None,
            DangerClass::Dangerous { reason, .. } | DangerClass::Forbidden { reason, .. } => Some(reason),
        }
    }

    /// The more severe of `self` and `other` (first wins on ties).
    #[must_use]
    pub fn max(self, other: DangerClass) -> DangerClass {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }

    pub(crate) fn dangerous(category: DangerCategory, reason: impl Into<String>) -> Self {
        DangerClass::Dangerous { category, reason: reason.into() }
    }

    pub(crate) fn forbidden(category: DangerCategory, reason: impl Into<String>) -> Self {
        DangerClass::Forbidden { category, reason: reason.into() }
    }
}

/// Shell a command is written for. Detected from the terminal session,
/// never guessed from the command text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShellKind {
    /// PowerShell 7 / Windows PowerShell 5.1.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
    /// bash / zsh / Git Bash / WSL bash.
    Bash,
    /// POSIX `sh` / dash / fish (analysed with POSIX rules).
    Sh,
}

impl ShellKind {
    /// Grammar family.
    #[must_use]
    pub fn family(self) -> Family {
        match self {
            ShellKind::PowerShell => Family::PowerShell,
            ShellKind::Cmd => Family::Cmd,
            ShellKind::Bash | ShellKind::Sh => Family::Posix,
        }
    }

    /// Parses a user-facing shell name (`powershell`, `pwsh`, `cmd`, `bash`, `sh`, `zsh`, …).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "powershell" | "pwsh" | "powershell7" | "windowspowershell" | "ps" => Some(ShellKind::PowerShell),
            "cmd" | "cmd.exe" => Some(ShellKind::Cmd),
            "bash" | "zsh" | "gitbash" | "git-bash" | "wsl" => Some(ShellKind::Bash),
            "sh" | "dash" | "fish" | "ksh" | "posix" => Some(ShellKind::Sh),
            _ => None,
        }
    }
}

impl From<orkestr_platform::ShellKind> for ShellKind {
    fn from(k: orkestr_platform::ShellKind) -> Self {
        use orkestr_platform::ShellKind as P;
        match k {
            P::PowerShell7 | P::WindowsPowerShell => ShellKind::PowerShell,
            P::Cmd => ShellKind::Cmd,
            P::GitBash | P::Wsl | P::Bash | P::Zsh => ShellKind::Bash,
            P::Fish | P::Sh => ShellKind::Sh,
        }
    }
}

/// A structured (non-shell) action to classify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum Action {
    /// Read a file.
    ReadFile {
        /// Path (as requested).
        path: String,
    },
    /// Create or modify a file.
    WriteFile {
        /// Path.
        path: String,
    },
    /// Delete a file or directory.
    DeletePath {
        /// Path.
        path: String,
        /// Recursive delete of a directory.
        recursive: bool,
    },
    /// A git operation (`args` without the leading `git`).
    Git {
        /// Arguments.
        args: Vec<String>,
    },
    /// A SQL statement against an environment database.
    Sql {
        /// Statement text.
        statement: String,
    },
    /// Deployment to an environment.
    Deploy {
        /// Environment name.
        environment: String,
        /// Production / staging (protected) environment.
        protected: bool,
    },
    /// Creating/updating a secret.
    SecretWrite {
        /// Secret name.
        name: String,
    },
    /// Deleting a secret.
    SecretDelete {
        /// Secret name.
        name: String,
    },
}

/// Guard configuration.
#[derive(Debug, Clone, Default)]
pub struct GuardConfig {
    /// Paths that must never be written/deleted/killed by agents (the
    /// Orkestr data dir, H8). Compared case-insensitively as text.
    pub protected_paths: Vec<PathBuf>,
    /// The local core RPC port (H10: exposing it is forbidden).
    pub core_port: Option<u16>,
}

/// The dangerous-action guard.
#[derive(Debug, Clone, Default)]
pub struct DangerousActionGuard {
    config: GuardConfig,
    pub(crate) protected_lower: Vec<String>,
}

impl DangerousActionGuard {
    /// Guard with default (empty) config.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Guard with config.
    #[must_use]
    pub fn with_config(config: GuardConfig) -> Self {
        let protected_lower = config
            .protected_paths
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase())
            .filter(|p| p.len() > 3)
            .collect();
        Self { config, protected_lower }
    }

    /// Config.
    #[must_use]
    pub fn config(&self) -> &GuardConfig {
        &self.config
    }

    /// Classifies a command line written for `shell`.
    #[must_use]
    pub fn classify(&self, command: &str, shell: ShellKind) -> DangerClass {
        self.classify_in(command, shell, None)
    }

    /// Like [`classify`](Self::classify), additionally flagging absolute write
    /// targets outside `root` as `Dangerous(OutsideWorktree)`.
    #[must_use]
    pub fn classify_in(&self, command: &str, shell: ShellKind, root: Option<&std::path::Path>) -> DangerClass {
        self.analyze_in(command, shell, root).0
    }

    /// Like [`classify_in`](Self::classify_in), also returning the command's
    /// [`CommandFacets`] (network use, secret reads, reads outside `root`) so
    /// the permission engine can require the matching capabilities.
    #[must_use]
    pub fn analyze_in(
        &self,
        command: &str,
        shell: ShellKind,
        root: Option<&std::path::Path>,
    ) -> (DangerClass, CommandFacets) {
        let ctx = rules::Ctx::new(self, root.map(|r| r.to_string_lossy().replace('\\', "/").to_lowercase()));
        let class = rules::classify_text(&ctx, command, shell.family(), 0);
        (class, ctx.facets.into_inner())
    }

    /// Classifies a structured action.
    #[must_use]
    pub fn classify_action(&self, action: &Action) -> DangerClass {
        let ctx = rules::Ctx::new(self, None);
        match action {
            Action::ReadFile { path } => {
                rules::check_read_path(&ctx, path).max(facets::check_protected_ref(&ctx, path))
            }
            Action::WriteFile { path } => rules::check_write_path(&ctx, path, false),
            Action::DeletePath { path, recursive } => {
                let w = lexer::Word::literal(path);
                let mut c = rules::check_delete_target(&ctx, &w, *recursive, Family::Posix);
                c = c.max(rules::check_write_path(&ctx, path, true));
                if *recursive {
                    c = c.max(DangerClass::dangerous(DangerCategory::RecursiveDelete, "recursive delete"));
                }
                c
            }
            Action::Git { args } => {
                let mut words = vec![lexer::Word::literal("git")];
                words.extend(args.iter().map(|a| lexer::Word::literal(a)));
                let cmd = lexer::SimpleCommand { words, ..Default::default() };
                rules::classify_simple(&ctx, &cmd, &[], Family::Posix, 0)
            }
            Action::Sql { statement } => classify_sql(statement),
            Action::Deploy { environment, protected } => {
                if *protected {
                    DangerClass::dangerous(
                        DangerCategory::ProductionChange,
                        format!("deployment to protected environment `{environment}`"),
                    )
                } else {
                    DangerClass::Safe
                }
            }
            Action::SecretWrite { name } => {
                DangerClass::dangerous(DangerCategory::SecretChange, format!("writes secret `{name}`"))
            }
            Action::SecretDelete { name } => {
                DangerClass::dangerous(DangerCategory::SecretChange, format!("deletes secret `{name}`"))
            }
        }
    }

    /// Whether `path` names a secret-bearing file (`.env*`, `*.pem`, keys, …).
    /// The permission engine treats reads of such files as `Secrets`.
    #[must_use]
    pub fn is_secret_file(path: &str) -> bool {
        rules::is_secret_file(path)
    }

    /// Whether git `args` (without `git`) talk to a remote (`push`, `fetch`,
    /// `pull`, `clone`, `ls-remote`, …) and so need `Network`.
    #[must_use]
    pub fn git_uses_network(args: &[String]) -> bool {
        let lower: Vec<String> = args.iter().map(|a| a.to_lowercase()).collect();
        facets::git_is_network(&lower)
    }
}
