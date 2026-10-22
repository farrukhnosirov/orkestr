//! Publisher signatures (ed25519) and the trust store, shared with
//! `orkestr-marketplace` (ADR-0040). **Owner: WP1** (WP2 reuses it).

use crate::PluginError;

/// A detached signature over a package digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSignature {
    /// Key id (first 16 hex chars of the sha256 of the public key).
    pub key_id: String,
    /// Public key (base64).
    pub public_key: String,
    /// Signature (base64) over the digest.
    pub signature: String,
}

/// Verifies `sig` over `message` with the embedded public key.
///
/// # Errors
/// Not implemented in the step-0 skeleton.
pub fn verify(_sig: &PackageSignature, _message: &[u8]) -> Result<(), PluginError> {
    Err(PluginError::Unimplemented("plugins.signing"))
}
