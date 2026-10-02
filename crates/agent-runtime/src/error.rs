//! Errors of the agent runtime.

use orkestr_storage::StorageError;

/// Agent runtime error.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Unknown entity.
    #[error("{0} not found")]
    NotFound(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// Conflicting state (busy task, finished agent, …).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Not allowed (read-only built-in template, emergency stop, …).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// No usable model for the request.
    #[error("no usable model: {0}")]
    NoModel(String),
    /// Explicit stub marker (never fake success).
    #[error("unimplemented-feature: {0}")]
    Unimplemented(String),
    /// Task engine failure.
    #[error(transparent)]
    Task(#[from] orkestr_task_engine::TaskError),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Anything else (message is safe to show).
    #[error("{0}")]
    Internal(String),
}

impl From<serde_json::Error> for AgentError {
    fn from(e: serde_json::Error) -> Self {
        AgentError::Storage(StorageError::Json(e))
    }
}

impl From<orkestr_storage::rusqlite::Error> for AgentError {
    fn from(e: orkestr_storage::rusqlite::Error) -> Self {
        AgentError::Storage(StorageError::Sqlite(e))
    }
}

/// Result alias.
pub type AgentResult<T> = Result<T, AgentError>;
