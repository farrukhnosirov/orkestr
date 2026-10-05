//! Errors of the insight crate.

/// Insight errors.
#[derive(Debug, thiserror::Error)]
pub enum InsightError {
    /// Unknown snapshot / run / task.
    #[error("{0} not found")]
    NotFound(String),
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Storage failure.
    #[error(transparent)]
    Storage(#[from] orkestr_storage::StorageError),
    /// Event log failure.
    #[error(transparent)]
    Events(#[from] orkestr_events::EventError),
    /// I/O failure while scanning.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A background scan panicked or was cancelled.
    #[error("scan aborted: {0}")]
    Aborted(String),
}

/// Result alias.
pub type InsightResult<T> = Result<T, InsightError>;

impl From<serde_json::Error> for InsightError {
    fn from(e: serde_json::Error) -> Self {
        Self::Storage(orkestr_storage::StorageError::from(e))
    }
}
