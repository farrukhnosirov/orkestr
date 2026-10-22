//! Registry sources: bundled official catalogue, directory, HTTP. **Owner: WP2.**

use async_trait::async_trait;

use crate::MarketplaceError;

/// Id of the bundled official source.
pub const OFFICIAL_SOURCE_ID: &str = "official";

/// Reads raw registry files (`index.json`, `index.sig`, packages).
#[async_trait]
pub trait RegistryTransport: Send + Sync {
    /// Reads `path` relative to the registry root.
    async fn fetch(&self, path: &str) -> Result<Vec<u8>, MarketplaceError>;
}
