//! Quality errors.

use orkestr_storage::StorageError;

/// Result alias.
pub type QualityResult<T> = Result<T, QualityError>;

/// Errors of the quality service.
#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    /// Unknown run / project.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid request.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// A run is already active for this workspace.
    #[error("a quality run is already active in {0}")]
    Busy(String),
    /// Nothing to run.
    #[error("no checks to run: {0}")]
    NothingToRun(String),
    /// The service is shutting down.
    #[error("quality service is shutting down")]
    ShuttingDown,
    /// Storage failure.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// JSON failure.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
