//! Errors of the VS Code host.

/// Result alias.
pub type HostResult<T> = Result<T, HostError>;

/// Errors raised by the VS Code host.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// No server build for this OS / architecture.
    #[error("{0}")]
    Unsupported(String),
    /// The server is not installed.
    #[error("the VS Code server is not installed")]
    NotInstalled,
    /// The user did not accept the risk dialog.
    #[error("installing the VS Code server requires explicit consent (extensions run as your OS user)")]
    ConsentRequired,
    /// Another install / start / stop is in progress, or the state forbids the action.
    #[error("{0}")]
    Busy(String),
    /// Release metadata problem.
    #[error("release: {0}")]
    Release(String),
    /// Checksum problem.
    #[error("checksum: {0}")]
    Checksum(String),
    /// Network problem.
    #[error("network: {0}")]
    Network(String),
    /// Archive problem.
    #[error("archive: {0}")]
    Archive(String),
    /// The server process failed.
    #[error("server: {0}")]
    Server(String),
    /// Bad input.
    #[error("{0}")]
    Invalid(String),
    /// Cancelled by the user.
    #[error("cancelled")]
    Cancelled,
    /// I/O.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A call through the Orkestr bridge extension failed.
    #[error("{0}")]
    Bridge(#[from] crate::bridge::BridgeError),
}

impl From<orkestr_platform::PlatformError> for HostError {
    fn from(e: orkestr_platform::PlatformError) -> Self {
        HostError::Server(e.to_string())
    }
}

impl From<reqwest::Error> for HostError {
    fn from(e: reqwest::Error) -> Self {
        HostError::Network(e.without_url().to_string())
    }
}
