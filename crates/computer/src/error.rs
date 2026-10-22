//! Errors.

use orkestr_protocol::error::RpcErrorCode;
use orkestr_protocol::RpcError;

/// Computer assistant error.
#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    /// Bad input (unknown key, invalid step, bad set…).
    #[error("{0}")]
    Invalid(String),
    /// Unknown id.
    #[error("not found: {0}")]
    NotFound(String),
    /// Not possible on this host / backend.
    #[error("{0}")]
    Unsupported(String),
    /// Refused by the safety rules.
    #[error("{0}")]
    Forbidden(String),
    /// A backend call failed.
    #[error("{0}")]
    Backend(String),
    /// Stopped by the user / emergency stop.
    #[error("stopped")]
    Stopped,
    /// The AI planner failed.
    #[error("planner: {0}")]
    Planner(String),
    /// Conflict (already resolved…).
    #[error("{0}")]
    Conflict(String),
    /// Storage / IO.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias.
pub type ComputerResult<T> = Result<T, ComputerError>;

impl From<ComputerError> for RpcError {
    fn from(e: ComputerError) -> Self {
        match e {
            ComputerError::Invalid(m) => RpcError::bad_request(m),
            ComputerError::NotFound(m) => RpcError::not_found(m),
            ComputerError::Unsupported(m) => RpcError::unimplemented_feature("computer")
                .with_data(serde_json::json!({ "unimplementedFeature": "computer", "detail": m })),
            ComputerError::Forbidden(m) => RpcError::forbidden(m),
            ComputerError::Conflict(m) => RpcError::conflict(m),
            ComputerError::Planner(m) => RpcError::new(RpcErrorCode::ProviderError, m),
            e @ (ComputerError::Backend(_) | ComputerError::Stopped | ComputerError::Io(_)) => {
                RpcError::internal(e.to_string())
            }
        }
    }
}
