//! VCS errors.

/// Result alias.
pub type VcsResult<T> = Result<T, VcsError>;

/// Errors raised by `orkestr-vcs`.
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    /// `git` is not installed / not on PATH.
    #[error("git executable not found — install Git (2.38 or newer) and restart Orkestr")]
    GitNotFound,
    /// The installed git is too old for an operation.
    #[error("{0}")]
    Unsupported(String),
    /// Not a repository.
    #[error("not a git repository: {0}")]
    NotARepository(String),
    /// A git command failed.
    #[error("git {command} failed: {stderr}")]
    Command {
        /// Subcommand (e.g. `branch`).
        command: String,
        /// Exit code.
        code: Option<i32>,
        /// Trimmed stderr.
        stderr: String,
    },
    /// The command did not finish in time (process killed).
    #[error("git {command} timed out after {secs}s")]
    Timeout {
        /// Subcommand.
        command: String,
        /// Timeout in seconds.
        secs: u64,
    },
    /// Another git process kept a `*.lock` file for longer than the retry budget.
    #[error("repository is locked by another git process ({0}); try again")]
    Locked(String),
    /// Merge conflicts predicted / present.
    #[error("merge conflicts in {0:?}")]
    Conflict(Vec<String>),
    /// The worktree has uncommitted changes.
    #[error("{0}")]
    Dirty(String),
    /// A ref moved concurrently (compare-and-swap lost).
    #[error("{0}")]
    RefChanged(String),
    /// Nothing staged.
    #[error("nothing to commit (stage changes first)")]
    NothingToCommit,
    /// A Dangerous operation was requested without explicit confirmation.
    #[error("confirmation required: {0}")]
    ConfirmationRequired(String),
    /// Invalid input (path, ref name, revision…).
    #[error("{0}")]
    InvalidArgument(String),
    /// Unknown branch/revision/worktree.
    #[error("{0}")]
    NotFound(String),
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Metadata storage failure.
    #[error(transparent)]
    Storage(#[from] orkestr_storage::StorageError),
}

impl VcsError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidArgument(msg.into())
    }
}
