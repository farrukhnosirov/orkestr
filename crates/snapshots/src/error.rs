//! Snapshot errors.

/// Result alias.
pub type SnapshotResult<T> = Result<T, SnapshotError>;

/// Errors raised by `orkestr-snapshots`.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Unknown snapshot / path.
    #[error("{0}")]
    NotFound(String),
    /// Invalid input.
    #[error("{0}")]
    InvalidInput(String),
    /// A Dangerous operation was requested without `confirm: true`.
    #[error("confirmation required: {0}")]
    ConfirmationRequired(String),
    /// The workspace changed after the plan was made.
    #[error("{0}")]
    Stale(String),
    /// Workspace exceeds the configured limits.
    #[error("{0}")]
    TooLarge(String),
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Metadata/object storage failure.
    #[error(transparent)]
    Storage(#[from] orkestr_storage::StorageError),
    /// Manifest (de)serialization failure.
    #[error("manifest: {0}")]
    Json(#[from] serde_json::Error),
}

impl SnapshotError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}
