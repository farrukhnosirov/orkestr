//! Registry sources: bundled official catalogue, directory, HTTP. **Owner: WP2.**

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::index::safe_relative_path;
use crate::MarketplaceError;

/// Id of the bundled official source.
pub const OFFICIAL_SOURCE_ID: &str = "official";

/// Public key (base64) of the official registry. The private key is kept
/// offline; `resources/official-registry` holds only signed output.
pub const OFFICIAL_PUBLIC_KEY: &str = "2BDN46nRbghtBEQTw587wSzluwozuAXZBpAeoKunFRE=";

/// Largest package accepted from any source.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;

/// Bundled official catalogue (`resources/official-registry`).
pub const OFFICIAL_FILES: &[(&str, &[u8])] = &[
    ("index.json", include_bytes!("../../../resources/official-registry/index.json")),
    ("index.sig", include_bytes!("../../../resources/official-registry/index.sig")),
    (
        "packages/orkestr.strict-code-reviewer-1.0.0.agent.json",
        include_bytes!("../../../resources/official-registry/packages/orkestr.strict-code-reviewer-1.0.0.agent.json"),
    ),
    (
        "packages/orkestr.docs-writer-1.0.0.agent.json",
        include_bytes!("../../../resources/official-registry/packages/orkestr.docs-writer-1.0.0.agent.json"),
    ),
];

/// Reads raw registry files (`index.json`, `index.sig`, packages).
#[async_trait]
pub trait RegistryTransport: Send + Sync {
    /// Reads `path` relative to the registry root.
    async fn fetch(&self, path: &str) -> Result<Vec<u8>, MarketplaceError>;
}

/// Embedded official catalogue.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfficialTransport;

#[async_trait]
impl RegistryTransport for OfficialTransport {
    async fn fetch(&self, path: &str) -> Result<Vec<u8>, MarketplaceError> {
        OFFICIAL_FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, b)| b.to_vec())
            .ok_or_else(|| MarketplaceError::NotFound(format!("official registry file {path}")))
    }
}

/// A local directory.
#[derive(Debug, Clone)]
pub struct DirectoryTransport {
    root: PathBuf,
}

impl DirectoryTransport {
    /// Directory registry at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl RegistryTransport for DirectoryTransport {
    async fn fetch(&self, path: &str) -> Result<Vec<u8>, MarketplaceError> {
        if !safe_relative_path(path) {
            return Err(MarketplaceError::Invalid(format!("unsafe registry path {path}")));
        }
        let full = self.root.join(path);
        let meta = tokio::fs::metadata(&full)
            .await
            .map_err(|e| MarketplaceError::Source(format!("{}: {e}", full.display())))?;
        if meta.len() > MAX_PACKAGE_BYTES as u64 {
            return Err(MarketplaceError::Source(format!("{path} is too large")));
        }
        tokio::fs::read(&full).await.map_err(|e| MarketplaceError::Source(format!("{}: {e}", full.display())))
    }
}

/// HTTP(S) registry (`<base>/index.json`), e.g. `http://host:8787/registry`.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    base: String,
    client: reqwest::Client,
}

impl HttpTransport {
    /// Registry at `base`.
    ///
    /// # Errors
    /// Not an http(s) URL.
    pub fn new(base: &str) -> Result<Self, MarketplaceError> {
        let base = normalize_http_base(base)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| MarketplaceError::Internal(e.to_string()))?;
        Ok(Self { base, client })
    }

    /// Base URL without trailing slash.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// HTTP client.
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

/// Validates and normalizes an HTTP registry URL.
///
/// # Errors
/// Not an http(s) URL.
pub fn normalize_http_base(base: &str) -> Result<String, MarketplaceError> {
    let b = base.trim().trim_end_matches('/');
    let ok = (b.starts_with("http://") || b.starts_with("https://")) && b.len() > "https://".len();
    if !ok {
        return Err(MarketplaceError::Invalid(format!("`{base}` is not an http(s) registry URL")));
    }
    Ok(b.to_owned())
}

#[async_trait]
impl RegistryTransport for HttpTransport {
    async fn fetch(&self, path: &str) -> Result<Vec<u8>, MarketplaceError> {
        if !safe_relative_path(path) {
            return Err(MarketplaceError::Invalid(format!("unsafe registry path {path}")));
        }
        let url = format!("{}/{path}", self.base);
        let resp = self.client.get(&url).send().await.map_err(|e| MarketplaceError::Source(format!("{url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(MarketplaceError::Source(format!("{url}: HTTP {}", resp.status())));
        }
        if resp.content_length().is_some_and(|l| l > MAX_PACKAGE_BYTES as u64) {
            return Err(MarketplaceError::Source(format!("{url}: too large")));
        }
        let bytes = resp.bytes().await.map_err(|e| MarketplaceError::Source(format!("{url}: {e}")))?;
        if bytes.len() > MAX_PACKAGE_BYTES {
            return Err(MarketplaceError::Source(format!("{url}: too large")));
        }
        Ok(bytes.to_vec())
    }
}
