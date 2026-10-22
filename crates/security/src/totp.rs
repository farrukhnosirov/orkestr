//! TOTP (RFC 6238, HMAC-SHA1, 6 digits, 30 s) — gateway fallback login factor.

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// Time step in seconds.
pub const STEP: u64 = 30;

/// HOTP value (RFC 4226) for `counter`.
#[must_use]
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> String {
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else { return String::new() };
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = usize::from(h[19] & 0x0f);
    let bin = (u32::from(h[off]) & 0x7f) << 24
        | u32::from(h[off + 1]) << 16
        | u32::from(h[off + 2]) << 8
        | u32::from(h[off + 3]);
    let code = bin % 10u32.pow(digits);
    format!("{code:0width$}", width = digits as usize)
}

/// TOTP code at `unix_secs`.
#[must_use]
pub fn totp(secret: &[u8], unix_secs: u64) -> String {
    hotp(secret, unix_secs / STEP, 6)
}

/// Verifies `code` within ±1 step; returns the matched step (callers store it
/// and reject codes with a step ≤ the last accepted one — no reuse).
#[must_use]
pub fn verify(secret: &[u8], code: &str, unix_secs: u64) -> Option<u64> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = unix_secs / STEP;
    [now.saturating_sub(1), now, now + 1].into_iter().find(|&step| {
        let expected = hotp(secret, step, 6);
        bool::from(subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), code.as_bytes()))
    })
}

/// New random 20-byte secret.
#[must_use]
pub fn generate_secret() -> Vec<u8> {
    use rand_core06::RngCore;
    let mut s = vec![0u8; 20];
    rand_core06::OsRng.fill_bytes(&mut s);
    s
}

/// RFC 4648 base32 (no padding), as used in `otpauth://` URIs.
#[must_use]
pub fn base32(data: &[u8]) -> String {
    const ALPHA: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            out.push(char::from(ALPHA[((buf >> (bits - 5)) & 31) as usize]));
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHA[((buf << (5 - bits)) & 31) as usize]));
    }
    out
}

/// `otpauth://totp/...` provisioning URI.
#[must_use]
pub fn otpauth_uri(secret: &[u8], account: &str, issuer: &str) -> String {
    let enc = |s: &str| s.replace('%', "%25").replace(' ', "%20").replace(':', "%3A").replace('&', "%26");
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B (SHA-1, 8 digits).
    #[test]
    fn rfc6238_vectors() {
        let secret = b"12345678901234567890";
        for (t, code) in
            [(59u64, "94287082"), (1_111_111_109, "07081804"), (1_234_567_890, "89005924"), (2_000_000_000, "69279037")]
        {
            assert_eq!(hotp(secret, t / STEP, 8), code, "t={t}");
        }
    }

    #[test]
    fn verify_window_and_format() {
        let s = b"12345678901234567890";
        let now = 1_234_567_890;
        assert_eq!(verify(s, &totp(s, now), now), Some(now / STEP));
        assert!(verify(s, &totp(s, now - 30), now).is_some());
        assert!(verify(s, &totp(s, now - 90), now).is_none());
        assert!(verify(s, "12345", now).is_none());
        assert!(verify(s, "abcdef", now).is_none());
    }

    #[test]
    fn base32_rfc4648() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }
}
