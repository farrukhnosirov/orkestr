//! Errors of the file service.

use std::io;

/// Result alias.
pub type FilesResult<T> = Result<T, FilesError>;

/// File service errors. Messages never contain absolute host paths of the
/// workspace root, only workspace-relative paths.
#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// The path is syntactically not allowed (traversal, absolute, UNC, device name, ADS, …).
    #[error("invalid path `{path}`: {reason}")]
    InvalidPath {
        /// Offending input (truncated).
        path: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The path resolves outside the workspace root (symlink/junction escape).
    #[error("path `{0}` resolves outside the workspace root")]
    OutsideRoot(String),
    /// The path is protected from modification (e.g. `.git`).
    #[error("path `{0}` is protected and cannot be modified through the file service")]
    Protected(String),
    /// The workspace root is unusable.
    #[error("workspace root is not usable: {0}")]
    InvalidRoot(String),
    /// Nothing at the path.
    #[error("`{0}` not found")]
    NotFound(String),
    /// Something already exists at the path.
    #[error("`{0}` already exists")]
    AlreadyExists(String),
    /// The file changed since it was read (optimistic concurrency).
    #[error("`{path}` was changed on disk since it was read")]
    Conflict {
        /// Path.
        path: String,
        /// Current hash on disk (`None` when the file is gone).
        actual_hash: Option<String>,
    },
    /// File larger than the allowed limit.
    #[error("`{path}` is too large ({size} bytes, limit {limit})")]
    TooLarge {
        /// Path.
        path: String,
        /// Actual size.
        size: u64,
        /// Limit.
        limit: u64,
    },
    /// Expected a file.
    #[error("`{0}` is not a file")]
    NotAFile(String),
    /// Expected a directory.
    #[error("`{0}` is not a directory")]
    NotADirectory(String),
    /// Non-empty directory deleted without `recursive`.
    #[error("`{0}` is a non-empty directory; pass recursive=true to delete it")]
    RecursiveRequired(String),
    /// Content cannot be represented in the requested encoding.
    #[error("content cannot be encoded as {0}")]
    Encoding(&'static str),
    /// Invalid search query / glob.
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    /// Permission denied by the OS or a read-only file.
    #[error("permission denied for `{0}`")]
    PermissionDenied(String),
    /// Unsupported request.
    #[error("{0}")]
    Unsupported(String),
    /// Watcher failure.
    #[error("watcher: {0}")]
    Watch(String),
    /// Trash / storage failure.
    #[error("trash storage: {0}")]
    Storage(String),
    /// Internal failure (task join, poisoned state).
    #[error("internal: {0}")]
    Internal(String),
    /// Other I/O failure.
    #[error("i/o error on `{path}`: {source}")]
    Io {
        /// Path.
        path: String,
        /// Source.
        #[source]
        source: io::Error,
    },
}

impl FilesError {
    /// Maps an I/O error for `path`, turning well-known kinds into specific variants.
    pub fn io(path: impl Into<String>, source: io::Error) -> Self {
        let path = path.into();
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path),
            io::ErrorKind::AlreadyExists => Self::AlreadyExists(path),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(path),
            _ => Self::Io { path, source },
        }
    }

    pub(crate) fn invalid(path: &str, reason: &'static str) -> Self {
        Self::InvalidPath { path: path.chars().take(200).collect(), reason }
    }
}

impl From<orkestr_storage::StorageError> for FilesError {
    fn from(e: orkestr_storage::StorageError) -> Self {
        Self::Storage(e.to_string())
    }
}
