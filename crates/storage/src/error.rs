//! Storage errors.

/// Result alias.
pub type StorageResult<T> = Result<T, StorageError>;

/// Errors raised by storage components.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// (De)serialization failure of a stored document.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Migration problem (checksum drift, unknown applied migration, bad SQL…).
    #[error("migration `{id}`: {message}")]
    Migration {
        /// Migration id.
        id: String,
        /// Description.
        message: String,
    },
    /// Entity not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Uniqueness or state conflict.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Stored data is corrupt (hash mismatch, bad header…).
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// Invalid input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The blocking task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Join(String),
}

impl StorageError {
    /// True when the underlying SQLite error is a UNIQUE/PRIMARY KEY violation.
    #[must_use]
    pub fn is_unique_violation(&self) -> bool {
        matches!(
            self,
            StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    || e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    }
}
