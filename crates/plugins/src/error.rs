//! Error type of the crate.

use orkestr_protocol::RpcError;

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
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
    /// Package / manifest invalid.
    #[error("invalid package: {0}")]
    Package(String),
    /// Signature missing / invalid / untrusted.
    #[error("signature: {0}")]
    Signature(String),
    /// Unsupported ABI.
    #[error("unsupported plugin ABI {0}")]
    Abi(u32),
    /// The wasm module trapped (fuel, memory, epoch deadline, guest panic).
    #[error("plugin trapped: {0}")]
    Trap(String),
    /// Killed by the kill switch / emergency stop.
    #[error("plugin killed")]
    Killed,
    /// Safe mode: no plugin code runs.
    #[error("plugin safe mode is on")]
    SafeMode,
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

impl From<PluginError> for RpcError {
    fn from(e: PluginError) -> Self {
        match e {
            PluginError::NotFound(m) => RpcError::not_found(m),
            PluginError::Invalid(m) => RpcError::bad_request(m),
            PluginError::Denied(m) => RpcError::forbidden(m),
            PluginError::Conflict(m) => RpcError::conflict(m),
            PluginError::Unimplemented(f) => RpcError::unimplemented_feature(f),
            other => RpcError::internal(other.to_string()),
        }
    }
}
