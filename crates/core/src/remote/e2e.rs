//! `e2e-v1`: the end-to-end channel between a paired device and this Core
//! (remote-access.md §6, frozen wire spec of the Phase 4 plan).
//!
//! * Keys are raw SEC1 uncompressed P-256 points (65 bytes), base64url (no padding).
//! * Signatures are ECDSA P-256 / SHA-256 in IEEE P1363 form (`r ‖ s`, 64 bytes).
//! * `transcript = SHA-256("orkestr-e2e-v1" ‖ coreId ‖ deviceId ‖ ephD ‖ nonceD ‖ ts_be64 ‖ ephC ‖ nonceC)`;
//!   both sides sign the 32-byte transcript hash (as the ECDSA message).
//! * `HKDF-SHA256(ikm = ECDH(ephD, ephC).x, salt = nonceD ‖ nonceC,
//!   info = "orkestr-e2e-v1" ‖ coreId ‖ deviceId)` → 64 bytes = `kD2C ‖ kC2D`.
//! * Record = `ctr_be64 ‖ AES-256-GCM(key, nonce = dir_be32 ‖ ctr_be64, aad = ctr_be64)`;
//!   `dir` is 1 for device→core and 2 for core→device; counters start at 0 and
//!   must be strictly sequential (gap / reuse ⇒ the session is closed).
//!
//! The deterministic vector is published in `testdata/e2e-vector.json`.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use rand_core06::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Protocol label (also the HKDF info / transcript prefix).
pub const PROTOCOL: &str = "orkestr-e2e-v1";
/// Direction id of device → core records.
pub const DIR_D2C: u32 = 1;
/// Direction id of core → device records.
pub const DIR_C2D: u32 = 2;
/// Allowed clock skew of `clientHello.ts` (milliseconds).
pub const MAX_SKEW_MS: u64 = 5 * 60 * 1000;
/// Records per direction before the session must be re-established.
pub const MAX_RECORDS: u64 = 1 << 20;
/// Maximum record size (1 MiB).
pub const MAX_RECORD_BYTES: usize = 1 << 20;

/// E2E failure (the session is closed on any of these).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum E2eError {
    /// Malformed frame / key / encoding.
    #[error("malformed: {0}")]
    Malformed(String),
    /// Signature did not verify.
    #[error("bad signature")]
    BadSignature,
    /// Record authentication failed.
    #[error("record authentication failed")]
    Tamper,
    /// Counter gap or reuse.
    #[error("unexpected record counter {got} (expected {expected})")]
    Counter {
        /// Received counter.
        got: u64,
        /// Expected counter.
        expected: u64,
    },
    /// Timestamp outside the allowed skew.
    #[error("handshake timestamp outside the allowed window")]
    Stale,
    /// Too many records; re-handshake required.
    #[error("record limit reached; re-handshake required")]
    Exhausted,
}

/// Handshake frames (JSON text, `t` discriminator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum HandshakeFrame {
    /// Device → core.
    #[serde(rename_all = "camelCase")]
    ClientHello {
        /// Protocol version (`"e2e-v1"`).
        v: String,
        /// Target core id.
        core_id: String,
        /// Device id.
        device_id: String,
        /// Device ephemeral public key (SEC1 uncompressed, base64url).
        eph_pub: String,
        /// 32 random bytes, base64url.
        nonce: String,
        /// Device clock, unix milliseconds.
        ts: u64,
    },
    /// Core → device.
    #[serde(rename_all = "camelCase")]
    ServerHello {
        /// Core ephemeral public key (base64url).
        eph_pub: String,
        /// 32 random bytes, base64url.
        nonce: String,
        /// `Sign_CIK(transcript)` (P1363, base64url).
        sig: String,
    },
    /// Device → core.
    #[serde(rename_all = "camelCase")]
    ClientFinish {
        /// `Sign_DK(transcript)` (P1363, base64url).
        sig: String,
    },
    /// Either side: handshake refused.
    #[serde(rename_all = "camelCase")]
    Error {
        /// Machine code (`unknown_device`, `revoked`, `bad_signature`, `stale`, …).
        code: String,
    },
}

impl HandshakeFrame {
    /// Parses a frame from bytes.
    ///
    /// # Errors
    /// Malformed JSON.
    pub fn from_bytes(b: &[u8]) -> Result<Self, E2eError> {
        serde_json::from_slice(b).map_err(|e| E2eError::Malformed(e.to_string()))
    }

    /// Serializes to JSON bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// base64url (no padding) encode.
#[must_use]
pub fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// base64url (no padding) decode.
///
/// # Errors
/// Invalid base64url.
pub fn unb64(s: &str) -> Result<Vec<u8>, E2eError> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| E2eError::Malformed(e.to_string()))
}

/// Raw SEC1 uncompressed encoding of a public key.
#[must_use]
pub fn public_raw(pk: &PublicKey) -> Vec<u8> {
    pk.to_encoded_point(false).as_bytes().to_vec()
}

/// Parses a raw SEC1 public key.
///
/// # Errors
/// Not a valid P-256 point.
pub fn parse_public(raw: &[u8]) -> Result<PublicKey, E2eError> {
    PublicKey::from_sec1_bytes(raw).map_err(|_| E2eError::Malformed("public key".into()))
}

/// Parses a base64url verifying key.
///
/// # Errors
/// Invalid encoding / point.
pub fn parse_verifying_b64(s: &str) -> Result<VerifyingKey, E2eError> {
    Ok(VerifyingKey::from(parse_public(&unb64(s)?)?))
}

/// Base64url SEC1 encoding of a verifying key.
#[must_use]
pub fn verifying_b64(vk: &VerifyingKey) -> String {
    b64(vk.to_encoded_point(false).as_bytes())
}

/// Computes the transcript hash.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn transcript(
    core_id: &str,
    device_id: &str,
    eph_d: &[u8],
    nonce_d: &[u8],
    ts: u64,
    eph_c: &[u8],
    nonce_c: &[u8],
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(PROTOCOL.as_bytes());
    h.update(core_id.as_bytes());
    h.update(device_id.as_bytes());
    h.update(eph_d);
    h.update(nonce_d);
    h.update(ts.to_be_bytes());
    h.update(eph_c);
    h.update(nonce_c);
    h.finalize().into()
}

/// Derives `(kD2C, kC2D)`.
#[must_use]
pub fn derive_keys(
    shared_x: &[u8],
    nonce_d: &[u8],
    nonce_c: &[u8],
    core_id: &str,
    device_id: &str,
) -> ([u8; 32], [u8; 32]) {
    let mut salt = Vec::with_capacity(nonce_d.len() + nonce_c.len());
    salt.extend_from_slice(nonce_d);
    salt.extend_from_slice(nonce_c);
    let mut info = Vec::new();
    info.extend_from_slice(PROTOCOL.as_bytes());
    info.extend_from_slice(core_id.as_bytes());
    info.extend_from_slice(device_id.as_bytes());
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared_x);
    let mut okm = [0u8; 64];
    // 64 bytes is far below the HKDF-SHA256 limit.
    let _ = hk.expand(&info, &mut okm);
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    a.copy_from_slice(&okm[..32]);
    b.copy_from_slice(&okm[32..]);
    (a, b)
}

fn ecdh(secret: &SecretKey, peer: &PublicKey) -> [u8; 32] {
    let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine());
    let mut out = [0u8; 32];
    out.copy_from_slice(shared.raw_secret_bytes().as_slice());
    out
}

fn sign(key: &SigningKey, msg: &[u8]) -> String {
    let sig: Signature = key.sign(msg);
    b64(&sig.to_bytes())
}

/// Verifies a P1363 base64url signature.
///
/// # Errors
/// Bad encoding or signature.
pub fn verify(key: &VerifyingKey, msg: &[u8], sig_b64: &str) -> Result<(), E2eError> {
    let raw = unb64(sig_b64)?;
    let sig = Signature::from_slice(&raw).map_err(|_| E2eError::BadSignature)?;
    key.verify(msg, &sig).map_err(|_| E2eError::BadSignature)
}

/// Random 32-byte nonce.
#[must_use]
pub fn random_nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    OsRng.fill_bytes(&mut n);
    n
}

/// Sealing half of a session.
pub struct RecordSealer {
    cipher: Aes256Gcm,
    dir: u32,
    ctr: u64,
}

impl std::fmt::Debug for RecordSealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordSealer").field("dir", &self.dir).field("ctr", &self.ctr).finish_non_exhaustive()
    }
}

fn gcm_nonce(dir: u32, ctr: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&dir.to_be_bytes());
    n[4..].copy_from_slice(&ctr.to_be_bytes());
    n
}

impl RecordSealer {
    /// New sealer for `dir` with `key`.
    #[must_use]
    pub fn new(key: &[u8; 32], dir: u32) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()), dir, ctr: 0 }
    }

    /// Seals one plaintext frame into a record.
    ///
    /// # Errors
    /// Record limit reached.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, E2eError> {
        if self.ctr >= MAX_RECORDS {
            return Err(E2eError::Exhausted);
        }
        let ctr = self.ctr.to_be_bytes();
        let nonce = gcm_nonce(self.dir, self.ctr);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &ctr })
            .map_err(|_| E2eError::Tamper)?;
        self.ctr += 1;
        let mut out = Vec::with_capacity(8 + ct.len());
        out.extend_from_slice(&ctr);
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

/// Opening half of a session.
pub struct RecordOpener {
    cipher: Aes256Gcm,
    dir: u32,
    expected: u64,
}

impl std::fmt::Debug for RecordOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordOpener").field("dir", &self.dir).field("expected", &self.expected).finish_non_exhaustive()
    }
}

impl RecordOpener {
    /// New opener for `dir` with `key`.
    #[must_use]
    pub fn new(key: &[u8; 32], dir: u32) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()), dir, expected: 0 }
    }

    /// Opens one record.
    ///
    /// # Errors
    /// Malformed, out-of-order/replayed counter, or authentication failure.
    pub fn open(&mut self, record: &[u8]) -> Result<Vec<u8>, E2eError> {
        if record.len() < 8 + 16 || record.len() > MAX_RECORD_BYTES + 24 {
            return Err(E2eError::Malformed("record length".into()));
        }
        let mut c = [0u8; 8];
        c.copy_from_slice(&record[..8]);
        let got = u64::from_be_bytes(c);
        if got != self.expected {
            return Err(E2eError::Counter { got, expected: self.expected });
        }
        if got >= MAX_RECORDS {
            return Err(E2eError::Exhausted);
        }
        let nonce = gcm_nonce(self.dir, got);
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), Payload { msg: &record[8..], aad: &c })
            .map_err(|_| E2eError::Tamper)?;
        self.expected += 1;
        Ok(pt)
    }
}

/// An established session (one side).
#[derive(Debug)]
pub struct Session {
    /// Outgoing records.
    pub sealer: RecordSealer,
    /// Incoming records.
    pub opener: RecordOpener,
    /// Transcript hash (session binding, e.g. for step-up challenges).
    pub transcript: [u8; 32],
}

/// Device side, after sending `clientHello`.
pub struct DeviceHandshake {
    core_id: String,
    device_id: String,
    eph: SecretKey,
    nonce: [u8; 32],
    ts: u64,
}

impl std::fmt::Debug for DeviceHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceHandshake")
            .field("core_id", &self.core_id)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

impl DeviceHandshake {
    /// Starts a handshake with fresh randomness.
    #[must_use]
    pub fn start(core_id: &str, device_id: &str, ts_ms: u64) -> (Self, HandshakeFrame) {
        Self::start_with(core_id, device_id, ts_ms, SecretKey::random(&mut OsRng), random_nonce())
    }

    /// Deterministic variant (test vectors).
    #[must_use]
    pub fn start_with(
        core_id: &str,
        device_id: &str,
        ts_ms: u64,
        eph: SecretKey,
        nonce: [u8; 32],
    ) -> (Self, HandshakeFrame) {
        let hello = HandshakeFrame::ClientHello {
            v: "e2e-v1".into(),
            core_id: core_id.into(),
            device_id: device_id.into(),
            eph_pub: b64(&public_raw(&eph.public_key())),
            nonce: b64(&nonce),
            ts: ts_ms,
        };
        (Self { core_id: core_id.into(), device_id: device_id.into(), eph, nonce, ts: ts_ms }, hello)
    }

    /// Processes `serverHello`; returns `clientFinish` and the session.
    ///
    /// # Errors
    /// Malformed frame, or the core signature does not verify against the pinned key.
    pub fn finish(
        self,
        server_hello: &HandshakeFrame,
        device_key: &SigningKey,
        core_key: &VerifyingKey,
    ) -> Result<(HandshakeFrame, Session), E2eError> {
        let HandshakeFrame::ServerHello { eph_pub, nonce, sig } = server_hello else {
            return Err(E2eError::Malformed(format!("expected serverHello, got {server_hello:?}")));
        };
        let eph_c_raw = unb64(eph_pub)?;
        let eph_c = parse_public(&eph_c_raw)?;
        let nonce_c = unb64(nonce)?;
        if nonce_c.len() != 32 {
            return Err(E2eError::Malformed("nonce".into()));
        }
        let eph_d_raw = public_raw(&self.eph.public_key());
        let th = transcript(&self.core_id, &self.device_id, &eph_d_raw, &self.nonce, self.ts, &eph_c_raw, &nonce_c);
        verify(core_key, &th, sig)?;
        let shared = ecdh(&self.eph, &eph_c);
        let (d2c, c2d) = derive_keys(&shared, &self.nonce, &nonce_c, &self.core_id, &self.device_id);
        let finish = HandshakeFrame::ClientFinish { sig: sign(device_key, &th) };
        Ok((
            finish,
            Session {
                sealer: RecordSealer::new(&d2c, DIR_D2C),
                opener: RecordOpener::new(&c2d, DIR_C2D),
                transcript: th,
            },
        ))
    }
}

/// Parsed `clientHello` (core side), before the device is looked up.
#[derive(Debug, Clone)]
pub struct ClientHelloData {
    /// Core id the device targets.
    pub core_id: String,
    /// Device id.
    pub device_id: String,
    /// Raw device ephemeral key.
    pub eph_raw: Vec<u8>,
    /// Device nonce.
    pub nonce: Vec<u8>,
    /// Device timestamp (ms).
    pub ts: u64,
}

impl ClientHelloData {
    /// Validates a `clientHello` frame.
    ///
    /// # Errors
    /// Wrong frame / version / encoding, or `ts` outside ±5 min of `now_ms`.
    pub fn parse(frame: &HandshakeFrame, now_ms: u64) -> Result<Self, E2eError> {
        let HandshakeFrame::ClientHello { v, core_id, device_id, eph_pub, nonce, ts } = frame else {
            return Err(E2eError::Malformed("expected clientHello".into()));
        };
        if v != "e2e-v1" {
            return Err(E2eError::Malformed(format!("unsupported version {v}")));
        }
        let eph_raw = unb64(eph_pub)?;
        parse_public(&eph_raw)?;
        let nonce = unb64(nonce)?;
        if nonce.len() != 32 {
            return Err(E2eError::Malformed("nonce".into()));
        }
        if now_ms.abs_diff(*ts) > MAX_SKEW_MS {
            return Err(E2eError::Stale);
        }
        Ok(Self { core_id: core_id.clone(), device_id: device_id.clone(), eph_raw, nonce, ts: *ts })
    }
}

/// Core side, after sending `serverHello`.
pub struct CoreHandshake {
    transcript: [u8; 32],
    d2c: [u8; 32],
    c2d: [u8; 32],
}

impl std::fmt::Debug for CoreHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreHandshake").finish_non_exhaustive()
    }
}

impl CoreHandshake {
    /// Responds to a validated `clientHello` with fresh randomness.
    ///
    /// # Errors
    /// Malformed hello.
    pub fn respond(hello: &ClientHelloData, cik: &SigningKey) -> Result<(Self, HandshakeFrame), E2eError> {
        Self::respond_with(hello, cik, SecretKey::random(&mut OsRng), random_nonce())
    }

    /// Deterministic variant (test vectors).
    ///
    /// # Errors
    /// Malformed hello.
    pub fn respond_with(
        hello: &ClientHelloData,
        cik: &SigningKey,
        eph: SecretKey,
        nonce: [u8; 32],
    ) -> Result<(Self, HandshakeFrame), E2eError> {
        let eph_d = parse_public(&hello.eph_raw)?;
        let eph_c_raw = public_raw(&eph.public_key());
        let th =
            transcript(&hello.core_id, &hello.device_id, &hello.eph_raw, &hello.nonce, hello.ts, &eph_c_raw, &nonce);
        let shared = ecdh(&eph, &eph_d);
        let (d2c, c2d) = derive_keys(&shared, &hello.nonce, &nonce, &hello.core_id, &hello.device_id);
        let frame = HandshakeFrame::ServerHello { eph_pub: b64(&eph_c_raw), nonce: b64(&nonce), sig: sign(cik, &th) };
        Ok((Self { transcript: th, d2c, c2d }, frame))
    }

    /// Verifies `clientFinish` against the **core-held** device key.
    ///
    /// # Errors
    /// Wrong frame or bad signature.
    pub fn complete(self, finish: &HandshakeFrame, device_key: &VerifyingKey) -> Result<Session, E2eError> {
        let HandshakeFrame::ClientFinish { sig } = finish else {
            return Err(E2eError::Malformed("expected clientFinish".into()));
        };
        verify(device_key, &self.transcript, sig)?;
        Ok(Session {
            sealer: RecordSealer::new(&self.c2d, DIR_C2D),
            opener: RecordOpener::new(&self.d2c, DIR_D2C),
            transcript: self.transcript,
        })
    }
}

/// Deterministic secret key from a label (test vectors only).
#[must_use]
pub fn test_key(label: &str) -> SecretKey {
    let mut ctr = 0u32;
    loop {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        h.update(ctr.to_be_bytes());
        let d: [u8; 32] = h.finalize().into();
        if let Ok(k) = SecretKey::from_slice(&d) {
            return k;
        }
        ctr += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE: &str = "core-0001";
    const DEV: &str = "0192f5d4-0000-7000-8000-000000000001";
    const TS: u64 = 1_790_000_000_000;

    fn handshake() -> (Session, Session) {
        let cik = SigningKey::from(test_key("cik"));
        let dk = SigningKey::from(test_key("dk"));
        let (dh, hello) = DeviceHandshake::start(CORE, DEV, TS);
        let parsed =
            ClientHelloData::parse(&HandshakeFrame::from_bytes(&hello.to_bytes()).unwrap(), TS + 1000).unwrap();
        let (ch, sh) = CoreHandshake::respond(&parsed, &cik).unwrap();
        let (fin, dev) = dh.finish(&sh, &dk, cik.verifying_key()).unwrap();
        let core = ch.complete(&fin, dk.verifying_key()).unwrap();
        (dev, core)
    }

    #[test]
    fn round_trip_both_directions() {
        let (mut dev, mut core) = handshake();
        assert_eq!(dev.transcript, core.transcript);
        for i in 0..3 {
            let msg = format!("{{\"type\":\"req\",\"id\":\"{i}\"}}");
            let rec = dev.sealer.seal(msg.as_bytes()).unwrap();
            assert_eq!(core.opener.open(&rec).unwrap(), msg.as_bytes());
            let rec = core.sealer.seal(b"pong").unwrap();
            assert_eq!(dev.opener.open(&rec).unwrap(), b"pong");
        }
    }

    #[test]
    fn replayed_or_skipped_counter_rejected() {
        let (mut dev, mut core) = handshake();
        let r0 = dev.sealer.seal(b"a").unwrap();
        let r1 = dev.sealer.seal(b"b").unwrap();
        let r2 = dev.sealer.seal(b"c").unwrap();
        core.opener.open(&r0).unwrap();
        assert!(matches!(core.opener.open(&r0), Err(E2eError::Counter { got: 0, expected: 1 })));
        assert!(matches!(core.opener.open(&r2), Err(E2eError::Counter { .. })));
        core.opener.open(&r1).unwrap();
    }

    #[test]
    fn tampered_record_rejected() {
        let (mut dev, mut core) = handshake();
        let mut rec = dev.sealer.seal(b"hello").unwrap();
        let last = rec.len() - 1;
        rec[last] ^= 1;
        assert_eq!(core.opener.open(&rec), Err(E2eError::Tamper));
        // Reflection: a core→device record cannot be opened by the core.
        let (mut dev, mut core) = handshake();
        let rec = core.sealer.seal(b"x").unwrap();
        assert_eq!(core.opener.open(&rec), Err(E2eError::Tamper));
        let _ = dev.sealer.seal(b"y");
    }

    #[test]
    fn wrong_device_key_or_core_key_rejected() {
        let cik = SigningKey::from(test_key("cik"));
        let dk = SigningKey::from(test_key("dk"));
        let other = SigningKey::from(test_key("other"));
        let (dh, hello) = DeviceHandshake::start(CORE, DEV, TS);
        let parsed = ClientHelloData::parse(&hello, TS).unwrap();
        let (ch, sh) = CoreHandshake::respond(&parsed, &cik).unwrap();
        let (fin, _) = dh.finish(&sh, &dk, cik.verifying_key()).unwrap();
        assert_eq!(ch.complete(&fin, other.verifying_key()).unwrap_err(), E2eError::BadSignature);
        let (dh, hello) = DeviceHandshake::start(CORE, DEV, TS);
        let parsed = ClientHelloData::parse(&hello, TS).unwrap();
        let (_, sh) = CoreHandshake::respond(&parsed, &other).unwrap();
        assert_eq!(dh.finish(&sh, &dk, cik.verifying_key()).unwrap_err(), E2eError::BadSignature);
    }

    #[test]
    fn stale_hello_rejected() {
        let (_, hello) = DeviceHandshake::start(CORE, DEV, TS);
        assert_eq!(ClientHelloData::parse(&hello, TS + MAX_SKEW_MS + 1).unwrap_err(), E2eError::Stale);
    }

    /// Builds the published deterministic vector.
    fn vector() -> serde_json::Value {
        let cik_sk = test_key("orkestr-e2e-v1 vector cik");
        let dk_sk = test_key("orkestr-e2e-v1 vector device");
        let eph_d = test_key("orkestr-e2e-v1 vector ephD");
        let eph_c = test_key("orkestr-e2e-v1 vector ephC");
        let nonce_d: [u8; 32] = Sha256::digest(b"orkestr-e2e-v1 vector nonceD").into();
        let nonce_c: [u8; 32] = Sha256::digest(b"orkestr-e2e-v1 vector nonceC").into();
        let cik = SigningKey::from(cik_sk.clone());
        let dk = SigningKey::from(dk_sk.clone());
        let (dh, hello) = DeviceHandshake::start_with(CORE, DEV, TS, eph_d.clone(), nonce_d);
        let parsed = ClientHelloData::parse(&hello, TS).unwrap();
        let (ch, sh) = CoreHandshake::respond_with(&parsed, &cik, eph_c.clone(), nonce_c).unwrap();
        let (fin, mut dev) = dh.finish(&sh, &dk, cik.verifying_key()).unwrap();
        let mut core = ch.complete(&fin, dk.verifying_key()).unwrap();
        let shared = ecdh(&eph_d, &eph_c.public_key());
        let (d2c, c2d) = derive_keys(&shared, &nonce_d, &nonce_c, CORE, DEV);
        let pt_d2c = r#"{"type":"req","id":"1","method":"projects.list","params":{}}"#;
        let pt_c2d = r#"{"type":"res","id":"1","ok":true,"result":{"projects":[]}}"#;
        let rec_d2c = dev.sealer.seal(pt_d2c.as_bytes()).unwrap();
        let rec_c2d = core.sealer.seal(pt_c2d.as_bytes()).unwrap();
        assert_eq!(core.opener.open(&rec_d2c).unwrap(), pt_d2c.as_bytes());
        assert_eq!(dev.opener.open(&rec_c2d).unwrap(), pt_c2d.as_bytes());
        serde_json::json!({
            "protocol": PROTOCOL,
            "encoding": "binary fields are base64url without padding; keys are raw SEC1 uncompressed P-256; private keys are the raw 32-byte scalar",
            "coreId": CORE,
            "deviceId": DEV,
            "ts": TS,
            "cikPrivate": b64(&cik_sk.to_bytes()),
            "cikPublic": b64(&public_raw(&cik_sk.public_key())),
            "devicePrivate": b64(&dk_sk.to_bytes()),
            "devicePublic": b64(&public_raw(&dk_sk.public_key())),
            "ephDPrivate": b64(&eph_d.to_bytes()),
            "ephDPublic": b64(&public_raw(&eph_d.public_key())),
            "nonceD": b64(&nonce_d),
            "ephCPrivate": b64(&eph_c.to_bytes()),
            "ephCPublic": b64(&public_raw(&eph_c.public_key())),
            "nonceC": b64(&nonce_c),
            "transcriptHash": b64(&dev.transcript),
            "ecdhSharedX": b64(&shared),
            "hkdfSalt": b64(&[nonce_d.as_slice(), nonce_c.as_slice()].concat()),
            "hkdfInfo": b64(format!("{PROTOCOL}{CORE}{DEV}").as_bytes()),
            "kD2C": b64(&d2c),
            "kC2D": b64(&c2d),
            "clientHello": hello,
            "serverHello": sh,
            "clientFinish": fin,
            "records": [
                { "direction": "d2c", "dir": DIR_D2C, "counter": 0, "plaintext": pt_d2c, "record": b64(&rec_d2c) },
                { "direction": "c2d", "dir": DIR_C2D, "counter": 0, "plaintext": pt_c2d, "record": b64(&rec_c2d) }
            ]
        })
    }

    #[test]
    fn published_vector_matches() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote/testdata/e2e-vector.json");
        let v = vector();
        if std::env::var_os("ORKESTR_WRITE_E2E_VECTOR").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        }
        let published: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(published, v);
        // Independent re-verification of the published signatures and records.
        let cik_pub = parse_verifying_b64(published["cikPublic"].as_str().unwrap()).unwrap();
        let th = unb64(published["transcriptHash"].as_str().unwrap()).unwrap();
        verify(&cik_pub, &th, published["serverHello"]["sig"].as_str().unwrap()).unwrap();
        let k: [u8; 32] = unb64(published["kD2C"].as_str().unwrap()).unwrap().try_into().unwrap();
        let mut op = RecordOpener::new(&k, DIR_D2C);
        let rec = unb64(published["records"][0]["record"].as_str().unwrap()).unwrap();
        assert_eq!(op.open(&rec).unwrap(), published["records"][0]["plaintext"].as_str().unwrap().as_bytes());
    }
}
