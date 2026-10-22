//! Registry index document (`index.json`) and its detached signature (`index.sig`).
//!
//! `index.sig` is a JSON [`DetachedSignature`] whose ed25519 signature covers
//! the exact bytes of `index.json` (written by [`sign_index`] as canonical
//! pretty JSON). Every entry additionally carries the publisher's signature
//! over the raw sha256 digest of the package file.

use serde::{Deserialize, Serialize};

use orkestr_domain::PermissionRequest;
use orkestr_protocol::methods::marketplace::PackageKind;

use crate::signing::{self, DetachedSignature, KeyPair, SignatureVerifier};
use crate::MarketplaceError;

/// Index format version.
pub const INDEX_FORMAT: u32 = 1;
/// Largest accepted `index.json`.
pub const MAX_INDEX_BYTES: usize = 8 * 1024 * 1024;

/// One package version in an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    /// Package id.
    pub id: String,
    /// Kind.
    pub kind: PackageKind,
    /// Display name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Publisher.
    pub publisher: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Permissions the package requests (shown before install).
    #[serde(default)]
    pub permissions: Vec<PermissionRequest>,
    /// Signing key id (must be listed in [`IndexDocument::keys`]).
    pub key_id: String,
    /// Package sha256 (hex).
    pub sha256: String,
    /// Publisher signature (base64) over the raw sha256 digest.
    pub signature: String,
    /// Package path relative to the index (`packages/...`).
    pub url: String,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
}

/// A publisher key listed in the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexKey {
    /// Key id.
    pub key_id: String,
    /// Publisher name.
    pub publisher: String,
    /// Public key (base64).
    pub public_key: String,
}

/// `index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDocument {
    /// Format version ([`INDEX_FORMAT`]).
    pub format: u32,
    /// Registry name.
    pub name: String,
    /// Publisher keys.
    #[serde(default)]
    pub keys: Vec<IndexKey>,
    /// Entries.
    pub entries: Vec<IndexEntry>,
    /// Revoked key ids.
    #[serde(default)]
    pub revoked_keys: Vec<String>,
}

impl IndexDocument {
    /// Empty index.
    #[must_use]
    pub fn empty(name: &str) -> Self {
        Self {
            format: INDEX_FORMAT,
            name: name.to_owned(),
            keys: Vec::new(),
            entries: Vec::new(),
            revoked_keys: Vec::new(),
        }
    }

    /// Key by id.
    #[must_use]
    pub fn key(&self, key_id: &str) -> Option<&IndexKey> {
        self.keys.iter().find(|k| k.key_id == key_id)
    }

    /// Entry revoked by flag or by key revocation.
    #[must_use]
    pub fn is_revoked(&self, e: &IndexEntry) -> bool {
        e.revoked || self.revoked_keys.contains(&e.key_id)
    }
}

/// A verified index.
#[derive(Debug, Clone)]
pub struct VerifiedIndex {
    /// Document.
    pub doc: IndexDocument,
    /// Registry key that signed it.
    pub signer: DetachedSignature,
}

fn bad(m: impl Into<String>) -> MarketplaceError {
    MarketplaceError::Signature(m.into())
}

/// Package paths must stay inside the registry.
#[must_use]
pub fn safe_relative_path(url: &str) -> bool {
    !url.is_empty()
        && !url.starts_with('/')
        && !url.starts_with('\\')
        && !url.contains(':')
        && !url.contains('\\')
        && url.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Verifies `index.json` + `index.sig`. With `expected_key` (embedded official
/// key or the key pinned on first refresh) a different registry key is refused.
///
/// # Errors
/// [`MarketplaceError::Signature`] on any mismatch; the index is not used.
pub fn verify_index(
    index: &[u8],
    sig: &[u8],
    verifier: &dyn SignatureVerifier,
    expected_key: Option<&str>,
) -> Result<VerifiedIndex, MarketplaceError> {
    if index.len() > MAX_INDEX_BYTES {
        return Err(bad("index.json is too large"));
    }
    let signer: DetachedSignature =
        serde_json::from_slice(sig).map_err(|e| bad(format!("index.sig is not a signature document: {e}")))?;
    if let Some(expected) = expected_key {
        if signer.public_key.trim() != expected.trim() {
            return Err(bad(format!(
                "registry key changed (expected {}, index signed by {})",
                signing::key_id(expected).unwrap_or_default(),
                signer.key_id
            )));
        }
    }
    if signing::key_id(&signer.public_key)? != signer.key_id {
        return Err(bad("index.sig key id does not match its public key"));
    }
    verifier
        .verify(&signer.public_key, index, &signer.signature)
        .map_err(|_| bad("index signature does not verify"))?;
    let doc: IndexDocument = serde_json::from_slice(index).map_err(|e| bad(format!("index.json is malformed: {e}")))?;
    if doc.format != INDEX_FORMAT {
        return Err(bad(format!("unsupported index format {}", doc.format)));
    }
    for k in &doc.keys {
        if signing::key_id(&k.public_key)? != k.key_id {
            return Err(bad(format!("key {} does not match its public key", k.key_id)));
        }
    }
    for e in &doc.entries {
        if doc.key(&e.key_id).is_none() {
            return Err(bad(format!("{}@{}: unknown key {}", e.id, e.version, e.key_id)));
        }
        if e.sha256.len() != 64 || !e.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad(format!("{}@{}: bad sha256", e.id, e.version)));
        }
        if !safe_relative_path(&e.url) {
            return Err(bad(format!("{}@{}: unsafe package path", e.id, e.version)));
        }
    }
    Ok(VerifiedIndex { doc, signer })
}

/// Serializes and signs an index: `(index.json, index.sig)`.
///
/// # Errors
/// Serialization failure.
pub fn sign_index(doc: &IndexDocument, key: &KeyPair) -> Result<(Vec<u8>, Vec<u8>), MarketplaceError> {
    let mut bytes = serde_json::to_vec_pretty(doc).map_err(|e| MarketplaceError::Internal(e.to_string()))?;
    bytes.push(b'\n');
    let sig =
        serde_json::to_vec_pretty(&key.detached(&bytes)).map_err(|e| MarketplaceError::Internal(e.to_string()))?;
    Ok((bytes, sig))
}

/// Verifies a downloaded package against its entry: sha256, then the
/// publisher signature over the digest.
///
/// # Errors
/// [`MarketplaceError::Signature`].
pub fn verify_package(
    doc: &IndexDocument,
    entry: &IndexEntry,
    bytes: &[u8],
    verifier: &dyn SignatureVerifier,
) -> Result<(), MarketplaceError> {
    let actual = signing::sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(&entry.sha256) {
        return Err(bad(format!(
            "{}@{}: sha256 mismatch (index {}, package {actual})",
            entry.id, entry.version, entry.sha256
        )));
    }
    let key = doc.key(&entry.key_id).ok_or_else(|| bad("unknown key"))?;
    verifier
        .verify(&key.public_key, &signing::package_message(&entry.sha256)?, &entry.signature)
        .map_err(|_| bad(format!("{}@{}: publisher signature does not verify", entry.id, entry.version)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::Ed25519Verifier;

    #[test]
    fn tampered_index_is_rejected() {
        let k = KeyPair::generate();
        let mut doc = IndexDocument::empty("t");
        doc.keys.push(IndexKey { key_id: k.key_id(), publisher: "p".into(), public_key: k.public_key() });
        let (bytes, sig) = sign_index(&doc, &k).unwrap();
        verify_index(&bytes, &sig, &Ed25519Verifier, None).unwrap();
        verify_index(&bytes, &sig, &Ed25519Verifier, Some(&k.public_key())).unwrap();
        let tampered = String::from_utf8(bytes.clone()).unwrap().replace("\"t\"", "\"x\"");
        assert!(verify_index(tampered.as_bytes(), &sig, &Ed25519Verifier, None).is_err());
        let other = KeyPair::generate();
        assert!(verify_index(&bytes, &sig, &Ed25519Verifier, Some(&other.public_key())).is_err());
    }

    #[test]
    fn paths_stay_inside() {
        assert!(safe_relative_path("packages/abc.json"));
        for p in ["../x", "/etc/passwd", "a/../../b", "C:/x", "a\\b", ""] {
            assert!(!safe_relative_path(p), "{p}");
        }
    }
}
