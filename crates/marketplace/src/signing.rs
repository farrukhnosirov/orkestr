//! ed25519 signatures for registry indexes and packages (ADR-0040).
//!
//! The plugin host owns the shared trust store (`orkestr_plugins::signing`);
//! until both are unified the marketplace verifies through the small
//! [`SignatureVerifier`] trait so the implementation can be swapped without
//! touching the registry code.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::MarketplaceError;

/// A detached signature (`index.sig`, package signatures): same shape as
/// `orkestr_plugins::signing::PackageSignature`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachedSignature {
    /// Key id ([`key_id`] of the public key).
    pub key_id: String,
    /// Public key (base64).
    pub public_key: String,
    /// Signature (base64).
    pub signature: String,
}

/// Verifies ed25519 signatures.
pub trait SignatureVerifier: Send + Sync {
    /// Verifies `signature` (base64) over `message` with `public_key` (base64).
    ///
    /// # Errors
    /// Malformed key / signature, or the signature does not verify.
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> Result<(), MarketplaceError>;
}

/// The built-in verifier (`ed25519-dalek`, strict verification).
#[derive(Debug, Clone, Copy, Default)]
pub struct Ed25519Verifier;

impl SignatureVerifier for Ed25519Verifier {
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> Result<(), MarketplaceError> {
        let key = decode_public_key(public_key)?;
        let sig = B64.decode(signature.trim()).map_err(|e| sig_err(format!("signature is not base64: {e}")))?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| sig_err("signature must be 64 bytes"))?;
        key.verify_strict(message, &Signature::from_bytes(&sig)).map_err(|_| sig_err("signature does not verify"))
    }
}

fn sig_err(m: impl Into<String>) -> MarketplaceError {
    MarketplaceError::Signature(m.into())
}

/// Decodes a base64 ed25519 public key.
///
/// # Errors
/// Not base64 / not 32 bytes / not a curve point.
pub fn decode_public_key(public_key: &str) -> Result<VerifyingKey, MarketplaceError> {
    let raw = B64.decode(public_key.trim()).map_err(|e| sig_err(format!("public key is not base64: {e}")))?;
    let raw: [u8; 32] = raw.try_into().map_err(|_| sig_err("public key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&raw).map_err(|e| sig_err(format!("invalid public key: {e}")))
}

/// sha256 of `bytes` (lower hex).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Full fingerprint digest of a public key (sha256 of the raw key, hex).
///
/// # Errors
/// Malformed key.
pub fn key_digest(public_key: &str) -> Result<String, MarketplaceError> {
    let raw = B64.decode(public_key.trim()).map_err(|e| sig_err(format!("public key is not base64: {e}")))?;
    Ok(sha256_hex(&raw))
}

/// Key id: first 16 hex chars of the sha256 of the raw public key.
///
/// # Errors
/// Malformed key.
pub fn key_id(public_key: &str) -> Result<String, MarketplaceError> {
    Ok(key_digest(public_key)?[..16].to_owned())
}

/// Human-comparable fingerprint (`ab12 cd34 ...`) of a public key.
///
/// # Errors
/// Malformed key.
pub fn fingerprint(public_key: &str) -> Result<String, MarketplaceError> {
    Ok(crate::keys::format_fingerprint(&key_digest(public_key)?))
}

/// The message a package signature covers: the raw 32-byte sha256 digest.
///
/// # Errors
/// `sha256` is not 64 hex chars.
pub fn package_message(sha256: &str) -> Result<Vec<u8>, MarketplaceError> {
    let d = hex::decode(sha256).map_err(|_| sig_err("sha256 is not hex"))?;
    if d.len() != 32 {
        return Err(sig_err("sha256 must be 32 bytes"));
    }
    Ok(d)
}

/// An ed25519 signing key (publisher or registry key).
pub struct KeyPair {
    key: SigningKey,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair").field("public_key", &self.public_key()).finish_non_exhaustive()
    }
}

impl KeyPair {
    /// New random key.
    #[must_use]
    pub fn generate() -> Self {
        Self { key: SigningKey::generate(&mut rand_core06::OsRng) }
    }

    /// From the 32-byte seed (hex or base64).
    ///
    /// # Errors
    /// Malformed seed.
    pub fn from_seed(seed: &str) -> Result<Self, MarketplaceError> {
        let s = seed.trim();
        let raw = if s.len() == 64 { hex::decode(s).ok() } else { None }.or_else(|| B64.decode(s).ok());
        let raw: [u8; 32] = raw
            .and_then(|r| r.try_into().ok())
            .ok_or_else(|| MarketplaceError::Invalid("signing key seed must be 32 bytes (hex or base64)".into()))?;
        Ok(Self { key: SigningKey::from_bytes(&raw) })
    }

    /// Seed (base64) for the vault.
    #[must_use]
    pub fn seed_b64(&self) -> String {
        B64.encode(self.key.to_bytes())
    }

    /// Public key (base64).
    #[must_use]
    pub fn public_key(&self) -> String {
        B64.encode(self.key.verifying_key().to_bytes())
    }

    /// Key id.
    #[must_use]
    pub fn key_id(&self) -> String {
        key_id(&self.public_key()).unwrap_or_default()
    }

    /// Signs `message` (base64 signature).
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> String {
        B64.encode(self.key.sign(message).to_bytes())
    }

    /// Detached signature over `message`.
    #[must_use]
    pub fn detached(&self, message: &[u8]) -> DetachedSignature {
        DetachedSignature { key_id: self.key_id(), public_key: self.public_key(), signature: self.sign(message) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip_and_tamper() {
        let k = KeyPair::generate();
        let v = Ed25519Verifier;
        let s = k.sign(b"hello");
        v.verify(&k.public_key(), b"hello", &s).unwrap();
        assert!(v.verify(&k.public_key(), b"hellO", &s).is_err());
        let other = KeyPair::generate();
        assert!(v.verify(&other.public_key(), b"hello", &s).is_err());
        let again = KeyPair::from_seed(&k.seed_b64()).unwrap();
        assert_eq!(again.public_key(), k.public_key());
        assert_eq!(k.key_id().len(), 16);
        assert_eq!(fingerprint(&k.public_key()).unwrap().split(' ').count(), 16);
    }
}
