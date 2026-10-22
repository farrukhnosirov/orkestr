//! Chunking, manifests, signed refs and the key ring (`sync-protocol.md` §3.2–§3.3, §5.3).

use serde::{Deserialize, Serialize};

use crate::crypto::{self, DeviceKey, Purpose, WorkspaceKeys};
use crate::vv::VersionVector;
use crate::SyncError;

/// FastCDC parameters (§3.2): min 16 KiB / avg 64 KiB / max 256 KiB.
pub const CDC_MIN: u32 = 16 * 1024;
/// Average chunk size.
pub const CDC_AVG: u32 = 64 * 1024;
/// Maximum chunk size.
pub const CDC_MAX: u32 = 256 * 1024;

/// Content-defined chunk boundaries `(offset, len)`; files < 16 KiB are one chunk.
#[must_use]
pub fn chunk_bounds(data: &[u8]) -> Vec<(usize, usize)> {
    if data.len() < CDC_MIN as usize {
        return vec![(0, data.len())];
    }
    fastcdc::v2020::FastCDC::with_level(data, CDC_MIN, CDC_AVG, CDC_MAX, fastcdc::v2020::Normalization::Level2)
        .map(|c| (c.offset, c.length))
        .collect()
}

/// A chunk reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    /// Remote (keyed) chunk id.
    pub id: String,
    /// Plaintext length.
    pub len: u64,
}

/// One entry of a stream manifest: a file (workspace) or a record (records).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    /// Relative POSIX path or record key.
    pub path: String,
    /// Plaintext BLAKE3 of the whole content (hex).
    pub content: String,
    /// Size in bytes.
    pub size: u64,
    /// Executable bit.
    #[serde(default)]
    pub exec: bool,
    /// Chunks in order.
    pub chunks: Vec<ChunkRef>,
    /// Record HLC (records stream; LWW).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hlc: Option<String>,
}

/// A stream state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version (1).
    pub format: u16,
    /// Opaque stream id.
    pub stream: String,
    /// Parent manifest ids.
    pub parent: Vec<String>,
    /// Version vector of this state.
    pub version: VersionVector,
    /// Author device.
    pub device_id: String,
    /// Creation time (RFC 3339).
    pub created_at: String,
    /// Entries sorted by path.
    pub entries: Vec<TreeEntry>,
}

/// A manifest as stored on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedManifest {
    /// `blake3(ciphertext)` hex.
    pub manifest_id: String,
    /// Sealed manifest, base64url.
    pub ciphertext: String,
    /// Author device.
    pub device: String,
    /// Ed25519 over `manifest_id ‖ stream ‖ version`.
    pub sig: String,
}

/// A stream head (§5.3), plaintext metadata + signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedRef {
    /// Stream id.
    pub stream_id: String,
    /// Head manifest.
    pub manifest_id: String,
    /// Version vector.
    pub version: VersionVector,
    /// Key epoch.
    pub epoch: u32,
    /// Signing device.
    pub device: String,
    /// Time.
    pub ts: String,
    /// Ed25519 signature.
    pub sig: String,
}

fn manifest_sig_msg(id: &str, stream: &str, v: &VersionVector) -> Vec<u8> {
    format!("orkestr-manifest-v1|{id}|{stream}|{}", v.canonical()).into_bytes()
}

impl SignedRef {
    fn msg(&self) -> Vec<u8> {
        format!(
            "orkestr-ref-v1|{}|{}|{}|{}|{}|{}",
            self.stream_id,
            self.manifest_id,
            self.version.canonical(),
            self.epoch,
            self.device,
            self.ts
        )
        .into_bytes()
    }

    /// Builds and signs a ref.
    #[must_use]
    pub fn sign(key: &DeviceKey, device: &str, stream_id: &str, manifest_id: &str, version: VersionVector) -> Self {
        let mut r = Self {
            stream_id: stream_id.to_owned(),
            manifest_id: manifest_id.to_owned(),
            version,
            epoch: 1,
            device: device.to_owned(),
            ts: orkestr_domain::now().to_rfc3339(),
            sig: String::new(),
        };
        r.sig = key.sign(&r.msg());
        r
    }

    /// Verifies the signature against the key ring.
    ///
    /// # Errors
    /// `Integrity` when the signer is unknown or the signature is invalid.
    pub fn verify(&self, ring: &KeyRing) -> Result<(), SyncError> {
        let pk = ring.key_of(&self.device)?;
        crypto::verify(pk, &self.msg(), &self.sig)
    }
}

/// Seals and signs a manifest.
///
/// # Errors
/// Serialization / sealing failures.
pub fn seal_manifest(keys: &WorkspaceKeys, dk: &DeviceKey, m: &Manifest) -> Result<SignedManifest, SyncError> {
    let json = serde_json::to_vec(m).map_err(|e| SyncError::Storage(e.to_string()))?;
    let ct = keys.seal(Purpose::Manifest, manifest_aad(&m.stream).as_bytes(), &json, true)?;
    let manifest_id = blake3::hash(&ct).to_hex().to_string();
    let sig = dk.sign(&manifest_sig_msg(&manifest_id, &m.stream, &m.version));
    Ok(SignedManifest { manifest_id, ciphertext: crypto::b64(&ct), device: m.device_id.clone(), sig })
}

fn manifest_aad(stream: &str) -> String {
    format!("orkestr-manifest-v1|{stream}")
}

/// Verifies (id, signature against the ring) and opens a manifest.
///
/// # Errors
/// `Integrity` on any mismatch.
pub fn open_manifest(
    keys: &WorkspaceKeys,
    ring: &KeyRing,
    stream: &str,
    expected_id: &str,
    sm: &SignedManifest,
) -> Result<Manifest, SyncError> {
    let ct = crypto::unb64(&sm.ciphertext)?;
    let id = blake3::hash(&ct).to_hex().to_string();
    if id != expected_id || sm.manifest_id != expected_id {
        return Err(SyncError::Integrity("manifest id mismatch".into()));
    }
    let json = keys.open(Purpose::Manifest, manifest_aad(stream).as_bytes(), &ct)?;
    let m: Manifest = serde_json::from_slice(&json).map_err(|e| SyncError::Integrity(format!("manifest: {e}")))?;
    if m.stream != stream || m.device_id != sm.device {
        return Err(SyncError::Integrity("manifest stream/device mismatch".into()));
    }
    crypto::verify(ring.key_of(&sm.device)?, &manifest_sig_msg(&id, stream, &m.version), &sm.sig)?;
    for e in &m.entries {
        validate_path(&e.path)?;
    }
    Ok(m)
}

/// Rejects absolute paths, `..`, backslashes and empty segments.
///
/// # Errors
/// `Integrity` for an unsafe path.
pub fn validate_path(p: &str) -> Result<(), SyncError> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains(':')
        || p.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        return Err(SyncError::Integrity(format!("unsafe path in manifest: {p}")));
    }
    Ok(())
}

/// A key-ring member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RingMember {
    /// Device id.
    pub device_id: String,
    /// Ed25519 public key (base64url).
    pub ed25519: String,
}

/// The key ring (sealed with `K_ring`, so only key holders can add members).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRing {
    /// Epoch.
    pub epoch: u32,
    /// Members.
    pub devices: Vec<RingMember>,
}

impl KeyRing {
    /// Public key of a member.
    ///
    /// # Errors
    /// `Integrity("unknown signer")` when the device is not in the ring.
    pub fn key_of(&self, device: &str) -> Result<&str, SyncError> {
        self.devices
            .iter()
            .find(|d| d.device_id == device)
            .map(|d| d.ed25519.as_str())
            .ok_or_else(|| SyncError::Integrity(format!("unknown signer {device}")))
    }

    /// Seals the ring.
    ///
    /// # Errors
    /// Sealing failures.
    pub fn seal(&self, keys: &WorkspaceKeys) -> Result<Vec<u8>, SyncError> {
        let json = serde_json::to_vec(self).map_err(|e| SyncError::Storage(e.to_string()))?;
        keys.seal(Purpose::Ring, b"orkestr-ring-v1", &json, false)
    }

    /// Opens a sealed ring.
    ///
    /// # Errors
    /// `Integrity` when it does not authenticate.
    pub fn open(keys: &WorkspaceKeys, sealed: &[u8]) -> Result<Self, SyncError> {
        let json = keys.open(Purpose::Ring, b"orkestr-ring-v1", sealed)?;
        serde_json::from_slice(&json).map_err(|e| SyncError::Integrity(format!("ring: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x & 0xff) as u8
            })
            .collect()
    }

    #[test]
    fn fastcdc_is_shift_stable() {
        let data = pseudo_random(2 * 1024 * 1024, 42);
        let mut shifted = b"inserted prefix bytes!".to_vec();
        shifted.extend_from_slice(&data);
        let hashes = |d: &[u8]| -> std::collections::HashSet<blake3::Hash> {
            chunk_bounds(d).into_iter().map(|(o, l)| blake3::hash(&d[o..o + l])).collect()
        };
        let a = hashes(&data);
        let b = hashes(&shifted);
        let shared = a.intersection(&b).count();
        assert!(a.len() > 8, "expected many chunks, got {}", a.len());
        // Only the chunk(s) around the insertion point change.
        assert!(shared + 2 >= a.len(), "shared {shared} of {}", a.len());
        for (_, l) in chunk_bounds(&data) {
            assert!(l <= CDC_MAX as usize);
        }
        assert_eq!(chunk_bounds(b"small"), vec![(0, 5)]);
    }

    #[test]
    fn path_validation() {
        for ok in ["a", "a/b.txt", "src/.hidden"] {
            validate_path(ok).unwrap();
        }
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a\\b", "C:/x", "a//b"] {
            assert!(validate_path(bad).is_err(), "{bad}");
        }
    }
}
