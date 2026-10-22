//! Publisher signatures (ed25519) and the trust store, shared with
//! `orkestr-marketplace` (ADR-0034, ADR-0040). **Owner: WP1** (WP2 reuses it).
//!
//! # What is signed
//!
//! * **Packages** (`.orkp`): the publisher signs [`package_message`] of the
//!   package's canonical [`content_digest`]: sha256 over the sorted list of
//!   `path "\n" hex(sha256(file)) "\n"` lines of every file except
//!   `signature.json`, prefixed with `orkestr-plugin-content-v1\n`. Re-zipping,
//!   reordering or recompressing a package keeps the signature valid;
//!   changing, adding or removing any file breaks it.
//! * **Anything else** (WP2: `index.json` of a registry): sign arbitrary bytes
//!   with [`PublisherKeyPair::sign`] and check them with [`verify`] /
//!   [`TrustStore::verify_trusted`]. Use a domain-separation prefix of your
//!   own (e.g. `orkestr-registry-index-v1\n`) in the signed message.
//!
//! # Keys
//!
//! Public keys travel as base64 (standard alphabet) of the 32 raw ed25519
//! bytes. [`key_id`] = first 16 hex chars of sha256(public key);
//! [`fingerprint`] = the full sha256 hex, shown to the user before trusting.
//!
//! # Trust store
//!
//! [`TrustStore`] = persisted keys (`plugins_trusted_keys`, managed by the user
//! / marketplace: `trust`, `untrust`, `list`) plus in-memory built-in keys
//! (`add_builtin`: the embedded official catalogue key, keys from
//! `ORKESTR_PLUGINS_TRUSTED_KEYS`). Built-ins cannot be untrusted.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use orkestr_domain::{now, Timestamp};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::Db;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::PluginError;

/// Domain-separation prefix of the package content digest.
pub const CONTENT_DIGEST_PREFIX: &str = "orkestr-plugin-content-v1\n";
/// Domain-separation prefix of the signed package message.
pub const PACKAGE_SIGNATURE_PREFIX: &str = "orkestr-package-signature-v1\n";
/// Env var: comma-separated base64 public keys trusted as built-ins (dev / acceptance fixtures).
pub const TRUSTED_KEYS_ENV: &str = "ORKESTR_PLUGINS_TRUSTED_KEYS";

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A detached signature over a message (package digest, registry index, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSignature {
    /// Key id (first 16 hex chars of the sha256 of the public key).
    pub key_id: String,
    /// Public key (base64).
    pub public_key: String,
    /// Signature (base64) over the message.
    pub signature: String,
}

/// The verified signer of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignerInfo {
    /// Key id.
    pub key_id: String,
    /// Full fingerprint (sha256 hex of the public key).
    pub fingerprint: String,
    /// Public key (base64).
    pub public_key: String,
}

/// A trusted publisher key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedKey {
    /// Key id.
    pub key_id: String,
    /// Public key (base64).
    pub public_key: String,
    /// Fingerprint (sha256 hex).
    pub fingerprint: String,
    /// Label (publisher name).
    pub label: String,
    /// Where the trust came from (`builtin`, `env`, `user`, `tofu:<source>`).
    pub source: String,
    /// Built-in (not persisted, cannot be untrusted).
    pub builtin: bool,
    /// When it was trusted.
    pub added_at: Timestamp,
}

/// sha256 hex of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Key id of a raw public key: first 16 hex chars of its sha256.
#[must_use]
pub fn key_id(public_key: &[u8; 32]) -> String {
    sha256_hex(public_key)[..16].to_owned()
}

/// Fingerprint of a raw public key: full sha256 hex.
#[must_use]
pub fn fingerprint(public_key: &[u8; 32]) -> String {
    sha256_hex(public_key)
}

/// Parses a base64 ed25519 public key.
///
/// # Errors
/// Not base64 / not 32 bytes / not a valid curve point.
pub fn parse_public_key(b64: &str) -> Result<VerifyingKey, PluginError> {
    let raw = B64.decode(b64.trim()).map_err(|_| PluginError::Signature("public key is not base64".into()))?;
    let arr: [u8; 32] =
        raw.try_into().map_err(|_| PluginError::Signature("public key must be 32 bytes (ed25519)".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|_| PluginError::Signature("invalid ed25519 public key".into()))
}

/// Canonical content digest (hex) of a package's files (`signature.json` excluded).
#[must_use]
pub fn content_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut h = Sha256::new();
    h.update(CONTENT_DIGEST_PREFIX.as_bytes());
    for (path, bytes) in files {
        if path == crate::package::SIGNATURE_FILE {
            continue;
        }
        h.update(path.as_bytes());
        h.update(b"\n");
        h.update(sha256_hex(bytes).as_bytes());
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// The message a publisher signs for a package with content digest `digest_hex`.
#[must_use]
pub fn package_message(digest_hex: &str) -> Vec<u8> {
    format!("{PACKAGE_SIGNATURE_PREFIX}{digest_hex}").into_bytes()
}

/// A publisher key pair (secret part zeroized on drop by `ed25519-dalek`).
#[derive(Clone)]
pub struct PublisherKeyPair {
    key: SigningKey,
}

impl std::fmt::Debug for PublisherKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublisherKeyPair").field("key_id", &self.key_id()).finish_non_exhaustive()
    }
}

impl PublisherKeyPair {
    /// New random key pair.
    #[must_use]
    pub fn generate() -> Self {
        Self { key: SigningKey::generate(&mut rand_core06::OsRng) }
    }

    /// From a 32-byte seed.
    #[must_use]
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(seed) }
    }

    /// From a base64 seed (as stored in the vault).
    ///
    /// # Errors
    /// Not base64 / not 32 bytes.
    pub fn from_seed_b64(b64: &str) -> Result<Self, PluginError> {
        let raw = B64.decode(b64.trim()).map_err(|_| PluginError::Signature("key seed is not base64".into()))?;
        let arr: [u8; 32] = raw.try_into().map_err(|_| PluginError::Signature("key seed must be 32 bytes".into()))?;
        Ok(Self::from_seed(&arr))
    }

    /// The seed as base64 (secret: store it in the vault only).
    #[must_use]
    pub fn seed_b64(&self) -> String {
        B64.encode(self.key.to_bytes())
    }

    /// Public key (base64).
    #[must_use]
    pub fn public_key_b64(&self) -> String {
        B64.encode(self.key.verifying_key().to_bytes())
    }

    /// Key id.
    #[must_use]
    pub fn key_id(&self) -> String {
        key_id(&self.key.verifying_key().to_bytes())
    }

    /// Fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.key.verifying_key().to_bytes())
    }

    /// Signs `message`.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> PackageSignature {
        PackageSignature {
            key_id: self.key_id(),
            public_key: self.public_key_b64(),
            signature: B64.encode(self.key.sign(message).to_bytes()),
        }
    }
}

/// Verifies `sig` over `message` with the embedded public key (no trust decision).
///
/// # Errors
/// Malformed key / signature, key id mismatch or bad signature.
pub fn verify(sig: &PackageSignature, message: &[u8]) -> Result<SignerInfo, PluginError> {
    let vk = parse_public_key(&sig.public_key)?;
    let raw = vk.to_bytes();
    if key_id(&raw) != sig.key_id {
        return Err(PluginError::Signature("key id does not match the public key".into()));
    }
    let bytes =
        B64.decode(sig.signature.trim()).map_err(|_| PluginError::Signature("signature is not base64".into()))?;
    let arr: [u8; 64] = bytes.try_into().map_err(|_| PluginError::Signature("signature must be 64 bytes".into()))?;
    vk.verify(message, &ed25519_dalek::Signature::from_bytes(&arr))
        .map_err(|_| PluginError::Signature("signature does not match the content (tampered?)".into()))?;
    Ok(SignerInfo { key_id: sig.key_id.clone(), fingerprint: fingerprint(&raw), public_key: sig.public_key.clone() })
}

/// Publisher trust store (persisted + built-in keys). Cheap to clone.
#[derive(Clone)]
pub struct TrustStore {
    db: Db,
    builtin: Arc<RwLock<BTreeMap<String, TrustedKey>>>,
}

impl std::fmt::Debug for TrustStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustStore").field("builtin", &self.builtin.read().len()).finish_non_exhaustive()
    }
}

fn trusted_from(public_key: &str, label: &str, source: &str, builtin: bool) -> Result<TrustedKey, PluginError> {
    let vk = parse_public_key(public_key)?;
    let raw = vk.to_bytes();
    Ok(TrustedKey {
        key_id: key_id(&raw),
        public_key: B64.encode(raw),
        fingerprint: fingerprint(&raw),
        label: label.chars().take(200).collect(),
        source: source.chars().take(200).collect(),
        builtin,
        added_at: now(),
    })
}

impl TrustStore {
    /// Store over the (migrated) database. Keys listed in
    /// [`TRUSTED_KEYS_ENV`] are added as built-ins (source `env`).
    #[must_use]
    pub fn new(db: Db) -> Self {
        let s = Self { db, builtin: Arc::new(RwLock::new(BTreeMap::new())) };
        if let Ok(v) = std::env::var(TRUSTED_KEYS_ENV) {
            for k in v.split(',').map(str::trim).filter(|k| !k.is_empty()) {
                if let Err(e) = s.add_builtin(k, "environment", "env") {
                    tracing::warn!(error = %e, "ignoring invalid key in {TRUSTED_KEYS_ENV}");
                }
            }
        }
        s
    }

    /// Adds an in-memory built-in key (embedded official catalogue key, ...).
    ///
    /// # Errors
    /// Invalid key.
    pub fn add_builtin(&self, public_key: &str, label: &str, source: &str) -> Result<TrustedKey, PluginError> {
        let k = trusted_from(public_key, label, source, true)?;
        self.builtin.write().insert(k.key_id.clone(), k.clone());
        Ok(k)
    }

    /// Trusts (persists) a key. Idempotent: re-trusting updates the label.
    ///
    /// # Errors
    /// Invalid key / storage.
    pub async fn trust(&self, public_key: &str, label: &str, source: &str) -> Result<TrustedKey, PluginError> {
        let k = trusted_from(public_key, label, source, false)?;
        let rec = k.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO plugins_trusted_keys (key_id, public_key, label, source, added_at) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(key_id) DO UPDATE SET label = excluded.label, source = excluded.source",
                    params![rec.key_id, rec.public_key, rec.label, rec.source, rec.added_at.to_rfc3339()],
                )?;
                Ok(())
            })
            .await?;
        Ok(self.get(&k.key_id).await?.unwrap_or(k))
    }

    /// Removes a persisted key. Returns whether it existed. Built-ins cannot be removed.
    ///
    /// # Errors
    /// Built-in key / storage.
    pub async fn untrust(&self, key_id: &str) -> Result<bool, PluginError> {
        if self.builtin.read().contains_key(key_id) {
            return Err(PluginError::Denied(format!("key {key_id} is built in and cannot be untrusted")));
        }
        let id = key_id.to_owned();
        let n =
            self.db.call(move |c| Ok(c.execute("DELETE FROM plugins_trusted_keys WHERE key_id = ?1", [id])?)).await?;
        Ok(n > 0)
    }

    /// Every trusted key (built-ins first).
    ///
    /// # Errors
    /// Storage.
    pub async fn list(&self) -> Result<Vec<TrustedKey>, PluginError> {
        let mut out: Vec<TrustedKey> = self.builtin.read().values().cloned().collect();
        let rows: Vec<(String, String, String, String, String)> = self
            .db
            .call(|c| {
                let mut st = c.prepare(
                    "SELECT key_id, public_key, label, source, added_at FROM plugins_trusted_keys ORDER BY added_at",
                )?;
                let rows = st
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        for (key_id, public_key, label, source, added_at) in rows {
            if out.iter().any(|k| k.key_id == key_id) {
                continue;
            }
            let fp = parse_public_key(&public_key).map(|vk| fingerprint(&vk.to_bytes())).unwrap_or_default();
            out.push(TrustedKey {
                key_id,
                public_key,
                fingerprint: fp,
                label,
                source,
                builtin: false,
                added_at: chrono::DateTime::parse_from_rfc3339(&added_at).map_or_else(|_| now(), |d| d.to_utc()),
            });
        }
        Ok(out)
    }

    /// One trusted key by id.
    ///
    /// # Errors
    /// Storage.
    pub async fn get(&self, key_id: &str) -> Result<Option<TrustedKey>, PluginError> {
        if let Some(k) = self.builtin.read().get(key_id) {
            return Ok(Some(k.clone()));
        }
        let id = key_id.to_owned();
        let found: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT key_id FROM plugins_trusted_keys WHERE key_id = ?1", [id], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        if found.is_none() {
            return Ok(None);
        }
        Ok(self.list().await?.into_iter().find(|k| k.key_id == key_id))
    }

    /// Verifies `sig` over `message` **and** requires its key to be trusted.
    ///
    /// # Errors
    /// [`PluginError::Signature`] when invalid or untrusted (the message names the fingerprint).
    pub async fn verify_trusted(&self, sig: &PackageSignature, message: &[u8]) -> Result<TrustedKey, PluginError> {
        let signer = verify(sig, message)?;
        match self.get(&signer.key_id).await? {
            Some(k) if k.public_key == signer.public_key => Ok(k),
            _ => Err(PluginError::Signature(format!(
                "publisher key {} (fingerprint {}) is not trusted",
                signer.key_id, signer.fingerprint
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip_and_tamper() {
        let kp = PublisherKeyPair::from_seed(&[7u8; 32]);
        let sig = kp.sign(b"hello");
        let who = verify(&sig, b"hello").unwrap();
        assert_eq!(who.key_id, kp.key_id());
        assert_eq!(who.fingerprint.len(), 64);
        assert!(matches!(verify(&sig, b"hellO"), Err(PluginError::Signature(_))));
        let mut bad = sig.clone();
        bad.key_id = "0000000000000000".into();
        assert!(verify(&bad, b"hello").is_err());
        let other = PublisherKeyPair::generate();
        let mut swapped = sig;
        swapped.public_key = other.public_key_b64();
        swapped.key_id = other.key_id();
        assert!(verify(&swapped, b"hello").is_err());
        assert_eq!(PublisherKeyPair::from_seed_b64(&kp.seed_b64()).unwrap().key_id(), kp.key_id());
    }

    #[test]
    fn content_digest_ignores_signature_and_order() {
        let mut a = BTreeMap::new();
        a.insert("b.txt".to_owned(), b"2".to_vec());
        a.insert("a.txt".to_owned(), b"1".to_vec());
        let d1 = content_digest(&a);
        a.insert(crate::package::SIGNATURE_FILE.to_owned(), b"{}".to_vec());
        assert_eq!(content_digest(&a), d1);
        a.insert("a.txt".to_owned(), b"x".to_vec());
        assert_ne!(content_digest(&a), d1);
    }

    #[tokio::test]
    async fn trust_store_persists_and_refuses_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.db")).unwrap();
        db.migrate(crate::store::migrations()).await.unwrap();
        let ts = TrustStore::new(db.clone());
        let kp = PublisherKeyPair::generate();
        let sig = kp.sign(b"m");
        assert!(ts.verify_trusted(&sig, b"m").await.is_err());
        ts.trust(&kp.public_key_b64(), "Acme", "user").await.unwrap();
        assert_eq!(ts.verify_trusted(&sig, b"m").await.unwrap().label, "Acme");
        assert_eq!(TrustStore::new(db.clone()).list().await.unwrap().len(), 1);
        assert!(ts.untrust(&kp.key_id()).await.unwrap());
        assert!(ts.verify_trusted(&sig, b"m").await.is_err());
        let b = PublisherKeyPair::generate();
        ts.add_builtin(&b.public_key_b64(), "Official", "builtin").unwrap();
        assert!(ts.untrust(&b.key_id()).await.is_err());
        assert!(ts.verify_trusted(&b.sign(b"z"), b"z").await.is_ok());
    }
}
