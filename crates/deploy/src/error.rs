//! Error type of the crate.

use orkestr_protocol::RpcError;

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    /// Not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Refused by policy / permission.
    #[error("denied: {0}")]
    Denied(String),
    /// Conflicting state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Adapter cannot run here / misconfigured.
    #[error("adapter unavailable: {0}")]
    Unavailable(String),
    /// Remote API / command failure (redacted).
    #[error("deploy failed: {0}")]
    Failed(String),
    /// Feature not implemented yet (explicit `unimplementedFeature`, RR-G14).
    #[error("`{0}` is not implemented yet (unimplemented-feature)")]
    Unimplemented(&'static str),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(#[from] orkestr_storage::StorageError),
    /// I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Internal failure.
    #[error("internal: {0}")]
    Internal(String),
}

impl From<DeployError> for RpcError {
    fn from(e: DeployError) -> Self {
        match e {
            DeployError::NotFound(m) => RpcError::not_found(m),
            DeployError::Invalid(m) => RpcError::bad_request(m),
            DeployError::Denied(m) => RpcError::forbidden(m),
            DeployError::Conflict(m) => RpcError::conflict(m),
            DeployError::Unimplemented(f) => RpcError::unimplemented_feature(f),
            other => RpcError::internal(other.to_string()),
        }
    }
}
