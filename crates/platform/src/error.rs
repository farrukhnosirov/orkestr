//! Platform errors.

/// Result alias.
pub type PlatformResult<T> = Result<T, PlatformError>;

/// Errors raised by platform adapters.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The capability is not available on this OS / in this build.
    #[error("{feature} is not supported on {os}: {hint}")]
    NotSupported {
        /// Capability name.
        feature: &'static str,
        /// OS name.
        os: &'static str,
        /// What to do instead / when it arrives.
        hint: &'static str,
    },
    /// I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Credential store error (never contains the secret).
    #[error("credential store: {0}")]
    Credential(String),
    /// An external command failed.
    #[error("command `{command}` failed ({status}): {stderr}")]
    Command {
        /// Program name.
        command: String,
        /// Exit status description.
        status: String,
        /// Trimmed stderr.
        stderr: String,
    },
    /// A command did not finish in time and was killed.
    #[error("command `{command}` timed out after {after_ms} ms")]
    Timeout {
        /// Program name.
        command: String,
        /// Timeout in milliseconds.
        after_ms: u64,
    },
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A native library failed (load, symbol lookup or call).
    #[error("{0}")]
    Native(String),
    /// No usable home/data directory could be determined.
    #[error("cannot determine a data directory; set ORKESTR_HOME")]
    NoDataDir,
}

impl PlatformError {
    /// Convenience constructor for `NotSupported` on the current OS.
    #[must_use]
    pub fn not_supported(feature: &'static str, hint: &'static str) -> Self {
        PlatformError::NotSupported { feature, os: std::env::consts::OS, hint }
    }
}

/// Runs a command and maps a non-zero exit to [`PlatformError::Command`].
pub(crate) fn run_command(program: &str, args: &[&str]) -> PlatformResult<std::process::Output> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null());
    crate::process::HideConsole::hide_console(&mut cmd);
    let output = cmd.output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(PlatformError::Command {
            command: program.to_owned(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().chars().take(500).collect(),
        })
    }
}
