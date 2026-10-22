//! Keys and sealing (`sync-protocol.md` §3.2, §4; ADR-0031).
//!
//! * Workspace data key `WDK = HKDF(remote_key)`; per-purpose subkeys via HKDF.
//! * Remote chunk ids are keyed BLAKE3 (`K_id`) of the plaintext (SR-73).
//! * Sealing: optional zstd(3) → length prefix → zero padding to a 4 KiB
//!   multiple → XChaCha20-Poly1305 with a random 192-bit nonce and a
//!   purpose-specific AAD.
//! * Device keys: Ed25519 signatures over manifests / refs.
//! * Key envelopes: X25519 ephemeral-static + HKDF-SHA256 + XChaCha20-Poly1305
//!   (a simplified HPKE base mode, ADR-0031).

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signer as _, Verifier as _};
use hkdf::Hkdf;
use rand_core06::{OsRng, RngCore};
use sha2::Sha256;

use crate::SyncError;

const PAD: usize = 4096;
const NONCE: usize = 24;

/// base64url (no padding).
#[must_use]
pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes base64url (padding tolerated).
///
/// # Errors
/// `Integrity` when the input is not base64url.
pub fn unb64(s: &str) -> Result<Vec<u8>, SyncError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| SyncError::Integrity(format!("bad base64: {e}")))
}

/// Fresh random bytes.
#[must_use]
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// Generates a new remote key (256-bit, base64url).
#[must_use]
pub fn generate_remote_key() -> String {
    b64(&random::<32>())
}

fn hkdf32(ikm: &[u8], salt: &[u8], info: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(Some(salt), ikm).expand(info.as_bytes(), &mut out).expect("32 bytes is a valid HKDF length");
    out
}

/// Keys derived from the shared remote key.
#[derive(Clone)]
pub struct WorkspaceKeys {
    chunk: [u8; 32],
    id: [u8; 32],
    manifest: [u8; 32],
    record: [u8; 32],
    ring: [u8; 32],
    stream: [u8; 32],
}

impl std::fmt::Debug for WorkspaceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkspaceKeys(..)")
    }
}

/// Purpose of a sealed blob (selects key + AAD prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// File / record value chunk.
    Chunk,
    /// Stream manifest.
    Manifest,
    /// Key ring.
    Ring,
}

impl WorkspaceKeys {
    /// Derives the key set from a base64url remote key (≥ 32 bytes).
    ///
    /// # Errors
    /// `Config` when the key is malformed or too short.
    pub fn from_remote_key(remote_key: &str) -> Result<Self, SyncError> {
        let raw = unb64(remote_key).map_err(|_| SyncError::Config("remote key is not base64url".into()))?;
        if raw.len() < 32 {
            return Err(SyncError::Config("remote key must be at least 256 bits".into()));
        }
        let wdk = hkdf32(&raw, b"orkestr-wdk-v1", "orkestr/wdk/v1");
        Ok(Self {
            chunk: hkdf32(&wdk, b"", "orkestr/chunk/v1"),
            id: hkdf32(&wdk, b"", "orkestr/chunk-id/v1"),
            manifest: hkdf32(&wdk, b"", "orkestr/manifest/v1"),
            record: hkdf32(&wdk, b"", "orkestr/record/v1"),
            ring: hkdf32(&wdk, b"", "orkestr/ring/v1"),
            stream: hkdf32(&wdk, b"", "orkestr/stream-id/v1"),
        })
    }

    /// Remote chunk id: keyed BLAKE3 of the plaintext (hex).
    #[must_use]
    pub fn chunk_id(&self, plaintext: &[u8]) -> String {
        blake3::keyed_hash(&self.id, plaintext).to_hex().to_string()
    }

    /// Opaque stream id for a logical stream name (e.g. `workspace:<key>`).
    #[must_use]
    pub fn stream_id(&self, name: &str) -> String {
        let h = blake3::keyed_hash(&self.stream, name.as_bytes()).to_hex();
        format!("s_{}", &h[..32])
    }

    fn key(&self, p: Purpose, record: bool) -> &[u8; 32] {
        match p {
            Purpose::Chunk if record => &self.record,
            Purpose::Chunk => &self.chunk,
            Purpose::Manifest => &self.manifest,
            Purpose::Ring => &self.ring,
        }
    }

    /// Seals `plaintext` (compressed when `compress`).
    ///
    /// # Errors
    /// `Storage` when compression fails.
    pub fn seal(&self, p: Purpose, aad: &[u8], plaintext: &[u8], compress: bool) -> Result<Vec<u8>, SyncError> {
        self.seal_with(p, false, aad, plaintext, compress)
    }

    /// Seals with an explicit record-key choice for chunks.
    ///
    /// # Errors
    /// `Storage` when compression fails.
    pub fn seal_with(
        &self,
        p: Purpose,
        record: bool,
        aad: &[u8],
        plaintext: &[u8],
        compress: bool,
    ) -> Result<Vec<u8>, SyncError> {
        let payload = if compress {
            zstd::bulk::compress(plaintext, 3).map_err(|e| SyncError::Storage(format!("zstd: {e}")))?
        } else {
            plaintext.to_vec()
        };
        let mut body = Vec::with_capacity(payload.len() + 5 + PAD);
        body.push(u8::from(compress));
        body.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(u32::MAX).to_be_bytes());
        body.extend_from_slice(&payload);
        let padded = body.len().div_ceil(PAD) * PAD;
        body.resize(padded, 0);
        let nonce = random::<NONCE>();
        let cipher = XChaCha20Poly1305::new(self.key(p, record).into());
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &body, aad })
            .map_err(|_| SyncError::Storage("encryption failed".into()))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Opens a sealed blob.
    ///
    /// # Errors
    /// `Integrity` on a bad tag, wrong AAD or malformed body.
    pub fn open(&self, p: Purpose, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SyncError> {
        self.open_with(p, false, aad, sealed)
    }

    /// Opens with an explicit record-key choice for chunks.
    ///
    /// # Errors
    /// `Integrity` on a bad tag, wrong AAD or malformed body.
    pub fn open_with(&self, p: Purpose, record: bool, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SyncError> {
        if sealed.len() < NONCE + 16 {
            return Err(SyncError::Integrity("sealed blob too short".into()));
        }
        let (nonce, ct) = sealed.split_at(NONCE);
        let cipher = XChaCha20Poly1305::new(self.key(p, record).into());
        let body = cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| SyncError::Integrity("authentication tag mismatch".into()))?;
        if body.len() < 5 {
            return Err(SyncError::Integrity("sealed body too short".into()));
        }
        let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
        let payload = body.get(5..5 + len).ok_or_else(|| SyncError::Integrity("bad sealed length".into()))?;
        if body[0] == 1 {
            zstd::stream::decode_all(payload).map_err(|e| SyncError::Integrity(format!("zstd: {e}")))
        } else {
            Ok(payload.to_vec())
        }
    }
}

/// A device's Ed25519 signing key.
#[derive(Clone)]
pub struct DeviceKey(ed25519_dalek::SigningKey);

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceKey({})", self.public())
    }
}

impl DeviceKey {
    /// Generates a new key.
    #[must_use]
    pub fn generate() -> Self {
        Self(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    /// Restores from the 32-byte secret.
    #[must_use]
    pub fn from_bytes(b: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(b))
    }

    /// Secret bytes (stored in the local sync db only).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Public key, base64url.
    #[must_use]
    pub fn public(&self) -> String {
        b64(self.0.verifying_key().as_bytes())
    }

    /// Signs `msg`, base64url signature.
    #[must_use]
    pub fn sign(&self, msg: &[u8]) -> String {
        b64(&self.0.sign(msg).to_bytes())
    }
}

/// Verifies an Ed25519 signature (base64url key + signature).
///
/// # Errors
/// `Integrity` when the key or signature is malformed or invalid.
pub fn verify(public: &str, msg: &[u8], sig: &str) -> Result<(), SyncError> {
    let pk: [u8; 32] = unb64(public)?.try_into().map_err(|_| SyncError::Integrity("bad signer key length".into()))?;
    let pk = ed25519_dalek::VerifyingKey::from_bytes(&pk).map_err(|_| SyncError::Integrity("bad signer key".into()))?;
    let sig: [u8; 64] = unb64(sig)?.try_into().map_err(|_| SyncError::Integrity("bad signature length".into()))?;
    pk.verify(msg, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| SyncError::Integrity("signature verification failed".into()))
}

/// Seals `secret` to an X25519 public key: `eph_pub(32) ‖ nonce(24) ‖ ct`.
///
/// # Errors
/// `Storage` when encryption fails.
pub fn seal_envelope(recipient: &[u8; 32], secret: &[u8]) -> Result<Vec<u8>, SyncError> {
    let eph = x25519_dalek::StaticSecret::random_from_rng(OsRng);
    let eph_pub = x25519_dalek::PublicKey::from(&eph);
    let shared = eph.diffie_hellman(&x25519_dalek::PublicKey::from(*recipient));
    let key = envelope_key(shared.as_bytes(), eph_pub.as_bytes(), recipient);
    let nonce = random::<NONCE>();
    let ct = XChaCha20Poly1305::new((&key).into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: secret, aad: b"orkestr-envelope-v1" })
        .map_err(|_| SyncError::Storage("envelope encryption failed".into()))?;
    let mut out = eph_pub.as_bytes().to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens an envelope with the recipient's X25519 secret.
///
/// # Errors
/// `Integrity` when malformed or not addressed to this key.
pub fn open_envelope(secret: &x25519_dalek::StaticSecret, env: &[u8]) -> Result<Vec<u8>, SyncError> {
    if env.len() < 32 + NONCE + 16 {
        return Err(SyncError::Integrity("envelope too short".into()));
    }
    let eph: [u8; 32] = env[..32].try_into().map_err(|_| SyncError::Integrity("envelope".into()))?;
    let me = x25519_dalek::PublicKey::from(secret);
    let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(eph));
    let key = envelope_key(shared.as_bytes(), &eph, me.as_bytes());
    XChaCha20Poly1305::new((&key).into())
        .decrypt(
            XNonce::from_slice(&env[32..32 + NONCE]),
            Payload { msg: &env[32 + NONCE..], aad: b"orkestr-envelope-v1" },
        )
        .map_err(|_| SyncError::Integrity("envelope authentication failed".into()))
}

fn envelope_key(shared: &[u8], eph: &[u8], recipient: &[u8]) -> [u8; 32] {
    let mut salt = eph.to_vec();
    salt.extend_from_slice(recipient);
    hkdf32(shared, &salt, "orkestr/envelope/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip_padding_and_tamper() {
        let k = WorkspaceKeys::from_remote_key(&generate_remote_key()).unwrap();
        let data = b"hello hello hello hello".repeat(100);
        let s = k.seal(Purpose::Chunk, b"aad", &data, true).unwrap();
        assert_eq!((s.len() - NONCE - 16) % PAD, 0);
        assert_eq!(k.open(Purpose::Chunk, b"aad", &s).unwrap(), data);
        assert!(matches!(k.open(Purpose::Chunk, b"other", &s), Err(SyncError::Integrity(_))));
        let mut t = s.clone();
        t[40] ^= 1;
        assert!(matches!(k.open(Purpose::Chunk, b"aad", &t), Err(SyncError::Integrity(_))));
        // Different purpose key cannot open it either.
        assert!(k.open(Purpose::Manifest, b"aad", &s).is_err());
    }

    #[test]
    fn chunk_ids_are_keyed() {
        let a = WorkspaceKeys::from_remote_key(&generate_remote_key()).unwrap();
        let b = WorkspaceKeys::from_remote_key(&generate_remote_key()).unwrap();
        assert_ne!(a.chunk_id(b"x"), b.chunk_id(b"x"));
        assert_ne!(a.chunk_id(b"x"), blake3::hash(b"x").to_hex().to_string());
        assert!(WorkspaceKeys::from_remote_key("c2hvcnQ").is_err());
    }

    #[test]
    fn signatures_and_envelopes() {
        let d = DeviceKey::generate();
        let sig = d.sign(b"m");
        verify(&d.public(), b"m", &sig).unwrap();
        assert!(verify(&d.public(), b"n", &sig).is_err());
        assert!(verify(&DeviceKey::generate().public(), b"m", &sig).is_err());

        let sk = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        let pk = x25519_dalek::PublicKey::from(&sk);
        let env = seal_envelope(pk.as_bytes(), b"wdk").unwrap();
        assert_eq!(open_envelope(&sk, &env).unwrap(), b"wdk");
        let other = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        assert!(open_envelope(&other, &env).is_err());
    }
}
