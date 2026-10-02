//! Security errors.

use orkestr_events::EventError;
use orkestr_storage::StorageError;

use crate::paths::PathError;

/// Result alias.
pub type SecurityResult<T> = Result<T, SecurityError>;

/// Errors raised by the security subsystem. Messages never contain secret values.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// Path confinement failure.
    #[error(transparent)]
    Path(#[from] PathError),
    /// Secret not found.
    #[error("secret not found: {0}")]
    SecretNotFound(String),
    /// Vault locked (emergency control §52).
    #[error("the secret vault is locked")]
    VaultLocked,
    /// Entity not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// State conflict (e.g. approval already resolved).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Access denied by policy (scope mismatch, emergency stop…).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// Emergency stop is active: new AI work cannot start.
    #[error("emergency stop is active: {0}")]
    EmergencyActive(String),
    /// Storage failure.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Event publishing failure.
    #[error(transparent)]
    Events(#[from] EventError),
    /// Crypto failure (wrong key, tampered vault).
    #[error("crypto: {0}")]
    Crypto(String),
    /// Secret backend failure (keyring, file).
    #[error("secret backend: {0}")]
    Backend(String),
    /// Waiting for a decision timed out.
    #[error("timed out")]
    Timeout,
    /// The operation was cancelled.
    #[error("cancelled")]
    Cancelled,
}

impl From<orkestr_platform::PlatformError> for SecurityError {
    fn from(e: orkestr_platform::PlatformError) -> Self {
        SecurityError::Backend(e.to_string())
    }
}

impl From<serde_json::Error> for SecurityError {
    fn from(e: serde_json::Error) -> Self {
        SecurityError::Storage(StorageError::Json(e))
    }
}
