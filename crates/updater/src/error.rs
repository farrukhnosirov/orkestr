//! Update errors with stable, UI-mappable codes.

use serde::Serialize;

/// Result alias of this crate.
pub type UpdateResult<T> = Result<T, UpdateError>;

/// Everything that can go wrong while checking, downloading or preparing an
/// update. [`UpdateError::code`] is stable: the UI localizes by code
/// (`updates:errors.<code>`) and shows the message only as a detail.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// No update source is configured (remote endpoint disabled, no local file).
    #[error("no update source is configured")]
    NotConfigured,
    /// The endpoint is not a valid `https://` URL (plain `http://` is accepted only for loopback hosts).
    #[error("invalid update endpoint: {0}")]
    InvalidEndpoint(String),
    /// The machine looks offline or the server is unreachable (DNS, connect, timeout).
    #[error("update server unreachable: {0}")]
    Offline(String),
    /// The server answered with an unexpected HTTP status.
    #[error("update server answered HTTP {0}")]
    Server(u16),
    /// The release manifest is malformed.
    #[error("invalid release manifest: {0}")]
    BadManifest(String),
    /// The manifest has no build for this platform.
    #[error("the release has no build for this platform (looked for: {0})")]
    NoPlatform(String),
    /// Signature missing, malformed, made with another key or not matching the file.
    #[error("signature verification failed: {0}")]
    BadSignature(String),
    /// A local installer has no `.sig` file next to it.
    #[error("the installer is not signed (no .sig file next to it)")]
    Unsigned,
    /// The download was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The data schema of this machine is newer than this build understands.
    #[error("the Orkestr data was written by a newer version ({0}); refusing to open it with an older build")]
    DatabaseNewer(String),
    /// File-system or database failure.
    #[error("{0}")]
    Io(String),
}

impl UpdateError {
    /// Stable machine-readable code (`snake_case`).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::InvalidEndpoint(_) => "invalid_endpoint",
            Self::Offline(_) => "offline",
            Self::Server(_) => "server_error",
            Self::BadManifest(_) => "bad_manifest",
            Self::NoPlatform(_) => "no_platform",
            Self::BadSignature(_) => "bad_signature",
            Self::Unsigned => "unsigned",
            Self::Cancelled => "cancelled",
            Self::DatabaseNewer(_) => "database_newer",
            Self::Io(_) => "io",
        }
    }

    /// `{code, message}` for the UI.
    #[must_use]
    pub fn to_info(&self) -> ErrorInfo {
        ErrorInfo { code: self.code().to_owned(), message: self.to_string() }
    }

    /// Classifies a `reqwest` failure: connectivity problems become [`UpdateError::Offline`].
    #[must_use]
    pub fn from_http(e: &reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            return Self::Server(status.as_u16());
        }
        if e.is_decode() || e.is_body() && !e.is_timeout() {
            return Self::BadManifest(e.to_string());
        }
        // connect / timeout / request (DNS) / redirect loops: the user can only retry later.
        Self::Offline(e.to_string())
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<rusqlite::Error> for UpdateError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Io(format!("database: {e}"))
    }
}

impl From<zip::result::ZipError> for UpdateError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Io(format!("archive: {e}"))
    }
}

/// Serializable error for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ErrorInfo {
    /// [`UpdateError::code`].
    pub code: String,
    /// English detail (paths, HTTP status…); the UI localizes by `code`.
    pub message: String,
}
