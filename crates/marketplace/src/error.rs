//! Error type of the crate.

use orkestr_protocol::RpcError;

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
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
    /// Index / package signature invalid or key not trusted.
    #[error("signature: {0}")]
    Signature(String),
    /// Source unreachable / unreadable.
    #[error("source: {0}")]
    Source(String),
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

impl From<MarketplaceError> for RpcError {
    fn from(e: MarketplaceError) -> Self {
        match e {
            MarketplaceError::NotFound(m) => RpcError::not_found(m),
            MarketplaceError::Invalid(m) => RpcError::bad_request(m),
            MarketplaceError::Denied(m) => RpcError::forbidden(m),
            MarketplaceError::Conflict(m) => RpcError::conflict(m),
            MarketplaceError::Unimplemented(f) => RpcError::unimplemented_feature(f),
            other => RpcError::internal(other.to_string()),
        }
    }
}
