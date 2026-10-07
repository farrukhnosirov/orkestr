//! Errors of the extension subsystem.

use thiserror::Error;

/// Extension / language-server error.
#[derive(Debug, Error)]
pub enum ExtError {
    /// Unknown extension, theme, file or session.
    #[error("not found: {0}")]
    NotFound(String),
    /// Malformed input (manifest, theme, request parameter).
    #[error("{0}")]
    Invalid(String),
    /// The archive or a path in it was rejected for safety reasons.
    #[error("rejected unsafe extension content: {0}")]
    Unsafe(String),
    /// Size limit exceeded.
    #[error("too large: {0}")]
    TooLarge(String),
    /// Downloaded file does not match the published checksum.
    #[error("SHA-256 mismatch: Open VSX publishes {expected}, the download is {actual}")]
    Checksum {
        /// Published checksum.
        expected: String,
        /// Computed checksum.
        actual: String,
    },
    /// Open VSX / HTTP failure.
    #[error("Open VSX request failed: {0}")]
    Network(String),
    /// A feature Orkestr's editor does not provide.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Conflicting state (install already running, server restarting, …).
    #[error("{0}")]
    Conflict(String),
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Database failure.
    #[error("storage: {0}")]
    Storage(#[from] orkestr_storage::StorageError),
}

/// Result alias.
pub type ExtResult<T> = Result<T, ExtError>;

impl From<serde_json::Error> for ExtError {
    fn from(e: serde_json::Error) -> Self {
        ExtError::Invalid(format!("invalid JSON: {e}"))
    }
}

impl From<zip::result::ZipError> for ExtError {
    fn from(e: zip::result::ZipError) -> Self {
        ExtError::Invalid(format!("not a valid .vsix (zip) archive: {e}"))
    }
}
