//! Terminal errors.

use orkestr_domain::TerminalId;

/// Errors of the terminal service.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// Unknown session.
    #[error("terminal {0} not found")]
    NotFound(TerminalId),
    /// Unknown shell id.
    #[error("unknown shell `{0}`")]
    UnknownShell(String),
    /// No shell could be detected or the custom shell is unusable.
    #[error("no usable shell: {0}")]
    NoShell(String),
    /// Invalid request (size, env name, empty command, …).
    #[error("invalid request: {0}")]
    Invalid(String),
    /// cwd does not exist / is not a directory.
    #[error("invalid cwd: {0}")]
    InvalidCwd(String),
    /// cwd escapes the confinement root.
    #[error("cwd {cwd} is outside of {root}")]
    CwdOutsideRoot {
        /// Requested cwd.
        cwd: String,
        /// Confinement root.
        root: String,
    },
    /// Too many sessions.
    #[error("terminal limit reached ({0} sessions); close a terminal first")]
    LimitReached(usize),
    /// The session has exited; input is not accepted anymore.
    #[error("terminal {0} has exited")]
    Exited(TerminalId),
    /// The caller may not perform this action on the session (e.g. a user
    /// typing into an agent-owned terminal, or raw writes by an agent).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// The command gate denied the command.
    #[error("command denied: {0}")]
    Denied(String),
    /// The command needs a user approval first.
    #[error("command requires approval: {reason}")]
    ApprovalRequired {
        /// Why approval is needed.
        reason: String,
        /// Approval request id when one was created.
        approval_id: Option<String>,
    },
    /// The command was cancelled.
    #[error("command cancelled")]
    Cancelled,
    /// PTY failure.
    #[error("pty: {0}")]
    Pty(String),
    /// Process spawn / I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The service is shutting down.
    #[error("terminal service is shutting down")]
    ShuttingDown,
}

/// Result alias.
pub type TerminalResult<T> = Result<T, TerminalError>;
