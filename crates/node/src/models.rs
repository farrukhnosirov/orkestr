//! Local model store (Phase 6, ADR-0039). **Owner: WP-D.**
//!
//! A model is a [`ModelManifest`] (`{name, tag, format, layers[{digest, size,
//! mediaType}]}`) whose layers are blobs in the [`crate::blobs::BlobStore`];
//! the manifest's own digest (SHA-256 of its canonical JSON) identifies it.
//! Installed models feed `NodeInfo.localModels` (scheduler input).

use orkestr_domain::Digest;
pub use orkestr_protocol::methods::models::{ModelLayer, ModelManifest, StoredModel};

/// Media type of a GGUF weights layer.
pub const MEDIA_TYPE_GGUF: &str = "application/vnd.orkestr.model.gguf";
/// Media type of a mock weights layer (tests / acceptance).
pub const MEDIA_TYPE_MOCK: &str = "application/vnd.orkestr.model.mock";

/// Digest of a manifest (SHA-256 of its canonical JSON).
#[must_use]
pub fn manifest_digest(manifest: &ModelManifest) -> Digest {
    use sha2::{Digest as _, Sha256};
    let bytes = serde_json::to_vec(manifest).unwrap_or_default();
    Digest::from_sha256(&Sha256::digest(bytes).into())
}
