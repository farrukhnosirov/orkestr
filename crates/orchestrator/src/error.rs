//! Errors of the orchestrator.

use orkestr_agent_runtime::AgentError;
use orkestr_storage::StorageError;
use orkestr_task_engine::TaskError;

/// Orchestrator error.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// Unknown entity.
    #[error("{0} not found")]
    NotFound(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// Conflicting state (busy conversation, plan already decided, …).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Not allowed (emergency stop, privacy policy, permission denied).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// No usable model.
    #[error("no usable model: {0}")]
    NoModel(String),
    /// The model call or the plan it produced failed.
    #[error("planning failed: {0}")]
    Planning(String),
    /// Explicit stub marker (never fake success).
    #[error("unimplemented-feature: {0}")]
    Unimplemented(String),
    /// Agent runtime failure.
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// Task engine failure.
    #[error(transparent)]
    Task(#[from] TaskError),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Anything else (message is safe to show).
    #[error("{0}")]
    Internal(String),
}

impl From<serde_json::Error> for OrchestratorError {
    fn from(e: serde_json::Error) -> Self {
        OrchestratorError::Storage(StorageError::Json(e))
    }
}

impl From<orkestr_storage::rusqlite::Error> for OrchestratorError {
    fn from(e: orkestr_storage::rusqlite::Error) -> Self {
        OrchestratorError::Storage(StorageError::Sqlite(e))
    }
}

/// Result alias.
pub type OrchResult<T> = Result<T, OrchestratorError>;
