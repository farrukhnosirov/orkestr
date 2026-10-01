//! Core errors and mapping to RPC errors.

use orkestr_events::EventError;
use orkestr_platform::PlatformError;
use orkestr_protocol::RpcError;
use orkestr_storage::StorageError;

/// Errors raised while starting/running the core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Invalid configuration.
    #[error("config: {0}")]
    Config(String),
    /// Another core already serves this data dir.
    #[error("another Orkestr core (pid {pid}) is already running at {url}")]
    AlreadyRunning {
        /// Its URL.
        url: String,
        /// Its pid.
        pid: u32,
    },
    /// Storage failure.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Event subsystem failure.
    #[error(transparent)]
    Events(#[from] EventError),
    /// Platform failure.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Maps storage errors to RPC errors without leaking internals.
pub fn storage_to_rpc(err: StorageError) -> RpcError {
    match err {
        StorageError::NotFound(m) => RpcError::not_found(m),
        StorageError::Conflict(m) => RpcError::conflict(m),
        StorageError::Invalid(m) => RpcError::bad_request(m),
        other => {
            tracing::error!(error = %other, "storage failure while handling request");
            RpcError::internal("internal storage error")
        }
    }
}

/// Maps event errors to RPC errors.
pub fn events_to_rpc(err: EventError) -> RpcError {
    match err {
        EventError::Invalid(m) => RpcError::bad_request(m),
        EventError::Log(e) => storage_to_rpc(e),
    }
}
