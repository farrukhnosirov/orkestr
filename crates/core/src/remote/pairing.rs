//! Pairing primitives (remote-access.md §4.2). Shared by the core and by
//! device implementations (the Rust test client; the TS client mirrors them).
//!
//! * `codeMac = HMAC-SHA256(key = code (UTF-8), msg = devicePub ‖ pairingId)`.
//! * `SAS = u32_be(SHA-256(corePub ‖ devicePub ‖ pairingId)[0..4]) mod 10^6`, 6 digits.
//! * `fingerprint = base64url(SHA-256(corePub))`.
//! * completion: `Sign_CIK(SHA-256("orkestr-pairing-v1" ‖ pairingId ‖ deviceId ‖ devicePub ‖ role))`
//!   with `role` the lowercase name (`viewer` | `operator` | `admin`).

use hmac::{Hmac, Mac};
use orkestr_protocol::policy::Role;
use rand_core06::RngCore;
use sha2::{Digest, Sha256};

/// `platform` a coordinator core reports when it claims a pairing to enroll
/// the pairing core as a worker node (`kind: "node"`, Phase 6).
pub const NODE_PLATFORM: &str = "orkestr-node";

/// Code alphabet (Crockford base32 without ambiguous characters).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// New 8-character pairing code.
#[must_use]
pub fn generate_code() -> String {
    let mut b = [0u8; 8];
    rand_core06::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| char::from(ALPHABET[usize::from(x & 31)])).collect()
}

/// `codeMac`.
#[must_use]
pub fn code_mac(code: &str, device_pub: &[u8], pairing_id: &str) -> [u8; 32] {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(code.as_bytes()) else { return [0; 32] };
    mac.update(device_pub);
    mac.update(pairing_id.as_bytes());
    mac.finalize().into_bytes().into()
}

/// Constant-time `codeMac` check.
#[must_use]
pub fn verify_code_mac(code: &str, device_pub: &[u8], pairing_id: &str, mac: &[u8]) -> bool {
    let expected = code_mac(code, device_pub, pairing_id);
    bool::from(subtle::ConstantTimeEq::ct_eq(expected.as_slice(), mac))
}

/// Short authentication string.
#[must_use]
pub fn sas(core_pub: &[u8], device_pub: &[u8], pairing_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(core_pub);
    h.update(device_pub);
    h.update(pairing_id.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Core key fingerprint.
#[must_use]
pub fn fingerprint(core_pub: &[u8]) -> String {
    super::e2e::b64(&Sha256::digest(core_pub))
}

/// Lowercase role name.
#[must_use]
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Viewer => "viewer",
        Role::Operator => "operator",
        Role::Admin => "admin",
    }
}

/// Message signed by the CIK when a pairing is approved.
#[must_use]
pub fn completion_message(pairing_id: &str, device_id: &str, device_pub: &[u8], role: Role) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"orkestr-pairing-v1");
    h.update(pairing_id.as_bytes());
    h.update(device_id.as_bytes());
    h.update(device_pub);
    h.update(role_name(role).as_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_mac_and_sas() {
        let code = generate_code();
        assert_eq!(code.len(), 8);
        let mac = code_mac(&code, b"dev", "p1");
        assert!(verify_code_mac(&code, b"dev", "p1", &mac));
        assert!(!verify_code_mac("WRONGCOD", b"dev", "p1", &mac));
        assert!(!verify_code_mac(&code, b"dev2", "p1", &mac));
        let s = sas(b"core", b"dev", "p1");
        assert_eq!(s.len(), 6);
        assert_eq!(s, sas(b"core", b"dev", "p1"));
        assert_ne!(s, sas(b"core", b"dev", "p2"));
    }
}
