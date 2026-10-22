//! Update signatures — the format of `tauri-plugin-updater`: a minisign
//! (Ed25519) signature, base64-encoded as a whole, verified against the
//! base64-encoded public key from `tauri.conf.json` (`plugins.updater.pubkey`).
//!
//! The trusted comment (covered by the global signature) carries the release
//! version (`version:0.1.1`): a tampered manifest cannot pair a new version
//! number with an older signed installer. `restore_update` of the plugin
//! requires it, so the release tool always writes it.

use base64::Engine as _;
use semver::Version;

use crate::error::{UpdateError, UpdateResult};
use crate::manifest::parse_version;

fn b64_text(what: &str, value: &str) -> UpdateResult<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|e| UpdateError::BadSignature(format!("{what} is not base64: {e}")))?;
    String::from_utf8(bytes).map_err(|_| UpdateError::BadSignature(format!("{what} is not UTF-8")))
}

/// The version recorded in a trusted comment (`…\tversion:1.2.3`).
#[must_use]
pub fn signed_version(trusted_comment: &str) -> Option<&str> {
    trusted_comment.split('\t').find_map(|f| f.strip_prefix("version:"))
}

/// Verifies `data` against a base64 signature and public key, and that the
/// signed version equals `expected_version`. Returns the trusted comment.
pub fn verify(data: &[u8], signature_b64: &str, pubkey_b64: &str, expected_version: &Version) -> UpdateResult<String> {
    let pk_text = b64_text("public key", pubkey_b64)?;
    let pk = minisign_verify::PublicKey::decode(&pk_text)
        .map_err(|e| UpdateError::BadSignature(format!("invalid public key: {e}")))?;
    let sig_text = b64_text("signature", signature_b64)?;
    let sig = minisign_verify::Signature::decode(&sig_text)
        .map_err(|e| UpdateError::BadSignature(format!("invalid signature: {e}")))?;
    pk.verify(data, &sig, true)
        .map_err(|e| UpdateError::BadSignature(format!("the file does not match its signature: {e}")))?;
    let comment = sig.trusted_comment().to_owned();
    match signed_version(&comment) {
        Some(v) if parse_version(v).is_ok_and(|v| &v == expected_version) => Ok(comment),
        Some(v) => Err(UpdateError::BadSignature(format!(
            "the file was signed for version {v}, the manifest announces {expected_version}"
        ))),
        None => Err(UpdateError::BadSignature("the signature does not record the release version".into())),
    }
}

/// Signs `data` with a Tauri updater private key (`tauri signer generate`
/// format: base64 of a password-protected minisign secret key box). The
/// trusted comment records the file name and version. Returns the base64
/// signature (contents of the `.sig` file / manifest `signature`).
pub fn sign(
    data: &[u8],
    private_key_b64: &str,
    password: &str,
    file_name: &str,
    version: &Version,
) -> UpdateResult<String> {
    let key_box_text = b64_text("private key", private_key_b64)?;
    let key_box = minisign::SecretKeyBox::from_string(&key_box_text)
        .map_err(|e| UpdateError::BadSignature(format!("invalid private key: {e}")))?;
    let sk = key_box
        .into_secret_key(Some(password.to_owned()))
        .map_err(|e| UpdateError::BadSignature(format!("cannot decrypt the private key (wrong password?): {e}")))?;
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let trusted = format!("timestamp:{timestamp}\tfile:{file_name}\tversion:{version}");
    let sig =
        minisign::sign(None, &sk, std::io::Cursor::new(data), Some(&trusted), Some("signature from tauri secret key"))
            .map_err(|e| UpdateError::BadSignature(format!("signing failed: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sig.to_string()))
}

/// Test/dev helper: a fresh password-protected key pair in Tauri's format,
/// `(private_key_b64, public_key_b64)`.
pub fn generate_keypair(password: &str) -> UpdateResult<(String, String)> {
    let kp = minisign::KeyPair::generate_encrypted_keypair(Some(password.to_owned()))
        .map_err(|e| UpdateError::Io(format!("key generation failed: {e}")))?;
    let sk_box = kp
        .sk
        .to_box(Some("rsign encrypted secret key"))
        .map_err(|e| UpdateError::Io(format!("key encoding failed: {e}")))?;
    let pk_box = kp.pk.to_box().map_err(|e| UpdateError::Io(format!("key encoding failed: {e}")))?;
    let e = base64::engine::general_purpose::STANDARD;
    Ok((e.encode(sk_box.to_string()), e.encode(pk_box.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_and_reject_tampering() {
        let (sk, pk) = generate_keypair("pw").unwrap();
        let v = Version::new(0, 1, 1);
        let sig = sign(b"installer bytes", &sk, "pw", "Orkestr_0.1.1_x64-setup.exe", &v).unwrap();
        let comment = verify(b"installer bytes", &sig, &pk, &v).unwrap();
        assert!(comment.contains("version:0.1.1") && comment.contains("file:Orkestr_0.1.1_x64-setup.exe"));

        assert!(matches!(verify(b"installer bytez", &sig, &pk, &v), Err(UpdateError::BadSignature(_))));
        // Signed for 0.1.1 but announced as 0.2.0 (manifest tampering).
        assert!(matches!(
            verify(b"installer bytes", &sig, &pk, &Version::new(0, 2, 0)),
            Err(UpdateError::BadSignature(_))
        ));
        // Another key.
        let (_, other_pk) = generate_keypair("x").unwrap();
        assert!(matches!(verify(b"installer bytes", &sig, &other_pk, &v), Err(UpdateError::BadSignature(_))));
        // Garbage.
        assert!(matches!(verify(b"x", "not base64!", &pk, &v), Err(UpdateError::BadSignature(_))));
        // Wrong password.
        assert!(sign(b"x", &sk, "nope", "f", &v).is_err());
    }

    #[test]
    fn reads_the_signed_version() {
        assert_eq!(signed_version("timestamp:1\tfile:a.exe\tversion:1.2.3"), Some("1.2.3"));
        assert_eq!(signed_version("timestamp:1\tfile:a.exe"), None);
    }
}
