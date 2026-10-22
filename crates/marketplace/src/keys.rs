//! Publisher key trust: fingerprint display, trust on first use, pinning for
//! updates, revocation. **Owner: WP2.**

/// Human-comparable fingerprint of a public key (`ab12 cd34 ...`).
///
/// Step-0 skeleton: formatting only; WP2 derives it from the sha256 of the key.
#[must_use]
pub fn format_fingerprint(hex_digest: &str) -> String {
    hex_digest.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}
