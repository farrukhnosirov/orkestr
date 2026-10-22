//! Native biometric step-up credential `orkestr-native-v1` (ADR-0038).
//!
//! The phone keeps a P-256 key behind the OS biometric gate and produces a
//! WebAuthn-shaped assertion, so the core reuses
//! [`webauthn::verify_assertion_json`] unchanged with a per-device relying
//! party:
//!
//! * `rpId = "orkestr-native-stepup"`, `origin = "orkestr-native:<deviceId>"`;
//! * `authenticatorData = SHA-256(rpId) ‖ 0x05 (UP|UV) ‖ counter_be32`;
//! * `clientDataJSON = {"type":"webauthn.get","challenge":…,"origin":…,"crossOrigin":false}`;
//! * `signature` = ECDSA P-256 / SHA-256 over
//!   `authenticatorData ‖ SHA-256(clientDataJSON)`, DER-encoded as in WebAuthn
//!   (the verifier also accepts raw 64-byte `r ‖ s`).
//!
//! The counter must strictly increase (also from 0). A reference vector lives
//! in `testdata/native-stepup-vector.json`.

use orkestr_domain::DeviceId;
use orkestr_protocol::RpcError;
use orkestr_security::webauthn::{self, AssertionJson, Credential, RelyingParty};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// `credential.type` of a native registration.
pub const NATIVE_TYPE: &str = "orkestr-native-v1";
/// Relying-party id of native assertions.
pub const NATIVE_RP_ID: &str = "orkestr-native-stepup";
/// Accepted `keyStorage` values.
pub const KEY_STORAGES: [&str; 3] = ["android-keystore-wrapped", "ios-keychain-wrapped", "software"];

/// `orkestr-native:<deviceId>`.
#[must_use]
pub fn origin(device: DeviceId) -> String {
    format!("orkestr-native:{device}")
}

/// The per-device relying party native assertions are verified against.
#[must_use]
pub fn relying_party(device: DeviceId) -> RelyingParty {
    RelyingParty { rp_id: NATIVE_RP_ID.to_owned(), origins: vec![origin(device)] }
}

/// Whether a `devices.registerStepUp` credential is a native one.
#[must_use]
pub fn is_native(credential: &serde_json::Value) -> bool {
    credential.get("type").and_then(serde_json::Value::as_str) == Some(NATIVE_TYPE)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationWire {
    credential_id: String,
    public_key: String,
    key_storage: String,
}

/// A validated native registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRegistration {
    /// Credential (SEC1 uncompressed key, counter 0).
    pub credential: Credential,
    /// Declared key storage.
    pub key_storage: String,
}

/// Parses `{type, credentialId, publicKey (SEC1 b64url), keyStorage}`.
///
/// # Errors
/// `bad_request` on any malformed field.
pub fn parse_registration(credential: &serde_json::Value) -> Result<NativeRegistration, RpcError> {
    let w: RegistrationWire = serde_json::from_value(credential.clone())
        .map_err(|e| RpcError::bad_request(format!("native credential: {e}")))?;
    let id = webauthn::unb64(&w.credential_id).map_err(|e| RpcError::bad_request(e.to_string()))?;
    if id.is_empty() || id.len() > 1023 {
        return Err(RpcError::bad_request("credentialId must be 1..=1023 bytes"));
    }
    let pk = webauthn::unb64(&w.public_key).map_err(|e| RpcError::bad_request(e.to_string()))?;
    let vk = VerifyingKey::from_sec1_bytes(&pk).map_err(|_| RpcError::bad_request("publicKey is not a P-256 point"))?;
    if !KEY_STORAGES.contains(&w.key_storage.as_str()) {
        return Err(RpcError::bad_request(format!("keyStorage must be one of {}", KEY_STORAGES.join(", "))));
    }
    Ok(NativeRegistration {
        credential: Credential { id, public_key: vk.to_encoded_point(false).as_bytes().to_vec(), sign_count: 0 },
        key_storage: w.key_storage,
    })
}

/// Native counters must strictly increase, including from 0 (the WebAuthn
/// verifier tolerates `0 → 0` for authenticators without counters).
///
/// # Errors
/// Reason string.
pub fn check_counter(a: &AssertionJson, stored: u32) -> Result<(), String> {
    let ad = webauthn::unb64(&a.authenticator_data).map_err(|e| e.to_string())?;
    if ad.len() < 37 {
        return Err("authenticatorData too short".into());
    }
    let counter = u32::from_be_bytes([ad[33], ad[34], ad[35], ad[36]]);
    if counter <= stored {
        return Err("signature counter did not increase".into());
    }
    Ok(())
}

/// `clientDataJSON` bytes exactly as the mobile client serializes them.
#[must_use]
pub fn client_data_json(challenge_b64: &str, device: DeviceId) -> String {
    format!(
        r#"{{"type":"webauthn.get","challenge":"{challenge_b64}","origin":"{}","crossOrigin":false}}"#,
        origin(device)
    )
}

/// `authenticatorData` of a native assertion.
#[must_use]
pub fn authenticator_data(counter: u32) -> Vec<u8> {
    let mut ad = Sha256::digest(NATIVE_RP_ID.as_bytes()).to_vec();
    ad.push(0x05);
    ad.extend_from_slice(&counter.to_be_bytes());
    ad
}

/// Builds a native assertion (reference implementation of the mobile side;
/// used by tests and Rust device clients).
#[must_use]
pub fn build_assertion(
    key: &SigningKey,
    device: DeviceId,
    credential_id: &[u8],
    challenge: &[u8],
    counter: u32,
) -> AssertionJson {
    let ad = authenticator_data(counter);
    let cd = client_data_json(&webauthn::b64(challenge), device);
    let mut msg = ad.clone();
    msg.extend_from_slice(&Sha256::digest(cd.as_bytes()));
    let sig: Signature = key.sign(&msg);
    AssertionJson {
        credential_id: webauthn::b64(credential_id),
        authenticator_data: webauthn::b64(&ad),
        client_data_json: webauthn::b64(cd.as_bytes()),
        signature: webauthn::b64(sig.to_der().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use orkestr_protocol::policy::Role;
    use serde_json::json;

    use super::*;
    use crate::remote::store::DeviceRecord;
    use crate::remote::{RemoteService, StepUpKind};

    const VECTOR: &str = include_str!("testdata/native-stepup-vector.json");
    const VECTOR_PATH: &str = "src/remote/testdata/native-stepup-vector.json";

    fn vector_key() -> SigningKey {
        SigningKey::from_slice(&Sha256::digest(b"orkestr native step-up vector key")).unwrap()
    }

    fn vector_json() -> serde_json::Value {
        let key = vector_key();
        let device = DeviceId::parse("0192f5d4-0000-7000-8000-00000000a001").unwrap();
        let pubkey = key.verifying_key().to_encoded_point(false);
        let cred_id = Sha256::digest(pubkey.as_bytes())[..16].to_vec();
        let challenge = Sha256::digest(b"orkestr native step-up vector challenge").to_vec();
        let counter = 7u32;
        let a = build_assertion(&key, device, &cred_id, &challenge, counter);
        let sig = Signature::from_der(&webauthn::unb64(&a.signature).unwrap()).unwrap();
        let cd = client_data_json(&webauthn::b64(&challenge), device);
        json!({
            "description": "orkestr-native-v1 step-up assertion (ADR-0038). Signature: ECDSA P-256/SHA-256 (RFC 6979 deterministic, low-S) over authenticatorData || SHA-256(clientDataJSON), DER-encoded (signatureRaw = r||s, also accepted). All binary fields base64url without padding.",
            "privateKey": webauthn::b64(&key.to_bytes()),
            "publicKey": webauthn::b64(pubkey.as_bytes()),
            "deviceId": device.to_string(),
            "credentialId": webauthn::b64(&cred_id),
            "rpId": NATIVE_RP_ID,
            "origin": origin(device),
            "challenge": webauthn::b64(&challenge),
            "counter": counter,
            "registration": { "type": NATIVE_TYPE, "credentialId": webauthn::b64(&cred_id), "publicKey": webauthn::b64(pubkey.as_bytes()), "keyStorage": "software" },
            "clientDataJSONText": cd,
            "authenticatorData": a.authenticator_data,
            "clientDataJSON": a.client_data_json,
            "signature": a.signature,
            "signatureRaw": webauthn::b64(&sig.to_bytes()),
        })
    }

    /// `ORKESTR_WRITE_VECTORS=1 cargo test -p orkestr-core native_stepup` rewrites the file.
    #[test]
    fn vector_matches_and_verifies() {
        let v = vector_json();
        if std::env::var_os("ORKESTR_WRITE_VECTORS").is_some() {
            std::fs::write(VECTOR_PATH, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        } else {
            let file: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
            assert_eq!(file, v, "testdata/native-stepup-vector.json is stale");
        }
        let device = DeviceId::parse(v["deviceId"].as_str().unwrap()).unwrap();
        let reg = parse_registration(&v["registration"]).unwrap();
        let a = AssertionJson::from_value(&v).unwrap();
        let challenge = webauthn::unb64(v["challenge"].as_str().unwrap()).unwrap();
        assert_eq!(
            webauthn::verify_assertion_json(&relying_party(device), &challenge, &reg.credential, &a).unwrap(),
            7
        );
        let mut raw = a.clone();
        raw.signature = v["signatureRaw"].as_str().unwrap().to_owned();
        assert!(webauthn::verify_assertion_json(&relying_party(device), &challenge, &reg.credential, &raw).is_ok());
    }

    #[test]
    fn registration_validation() {
        let key = vector_key();
        let pk = webauthn::b64(key.verifying_key().to_encoded_point(true).as_bytes());
        let ok = json!({"type": NATIVE_TYPE, "credentialId": "AAEC", "publicKey": pk, "keyStorage": "android-keystore-wrapped"});
        assert!(is_native(&ok));
        let r = parse_registration(&ok).unwrap();
        assert_eq!(r.credential.public_key.len(), 65, "normalised to uncompressed");
        for (k, v) in [("publicKey", "AAEC"), ("keyStorage", "cloud"), ("credentialId", "")] {
            let mut bad = ok.clone();
            bad[k] = json!(v);
            assert!(parse_registration(&bad).is_err(), "{k}");
        }
        assert!(!is_native(&json!({"type": "public-key"})));
    }

    struct Dev {
        id: DeviceId,
        key: SigningKey,
        cred_id: Vec<u8>,
    }

    fn device(svc: &RemoteService) -> Dev {
        let key = SigningKey::random(&mut rand_core06::OsRng);
        let id = DeviceId::new();
        svc.state.lock().devices.push(DeviceRecord {
            device_id: id,
            name: "phone".into(),
            platform: "android".into(),
            public_key: String::new(),
            role: Role::Admin,
            remote_terminal: false,
            project_ids: None,
            created_at: chrono::Utc::now(),
            last_seen_at: None,
            revoked: false,
            step_up: None,
            step_up_kind: StepUpKind::default(),
        });
        Dev { id, key, cred_id: vec![9, 8, 7, id.as_uuid().as_bytes()[15]] }
    }

    async fn register(svc: &RemoteService, d: &Dev) {
        let reg = json!({
            "type": NATIVE_TYPE,
            "credentialId": webauthn::b64(&d.cred_id),
            "publicKey": webauthn::b64(d.key.verifying_key().to_encoded_point(false).as_bytes()),
            "keyStorage": "software",
        });
        svc.register_step_up(d.id, &reg).await.unwrap();
    }

    fn sign(d: &Dev, origin_of: DeviceId, challenge: &str, counter: u32) -> serde_json::Value {
        let a = build_assertion(&d.key, origin_of, &d.cred_id, &webauthn::unb64(challenge).unwrap(), counter);
        serde_json::to_value(a).unwrap()
    }

    #[tokio::test]
    async fn native_step_up_flow() {
        let svc = RemoteService::new();
        let (a, b) = (device(&svc), device(&svc));
        register(&svc, &a).await;
        register(&svc, &b).await;
        let info = svc.device(a.id).unwrap().info();
        assert_eq!(info.step_up_kind.as_deref(), Some("native"));
        assert!(info.step_up_registered);

        let m = "approvals.resolve";
        let h = [1u8; 32];
        // ok
        let c = svc.issue_step_up(a.id, m, &h);
        let asn = sign(&a, a.id, &c, 1);
        svc.verify_step_up(a.id, m, &h, &asn).unwrap();
        // replay
        assert!(svc.verify_step_up(a.id, m, &h, &asn).unwrap_err().contains("already used"));
        // counter not increased
        let c = svc.issue_step_up(a.id, m, &h);
        assert!(svc.verify_step_up(a.id, m, &h, &sign(&a, a.id, &c, 1)).unwrap_err().contains("counter"));
        // wrong device: challenge issued to A presented by B
        let c = svc.issue_step_up(a.id, m, &h);
        assert!(svc.verify_step_up(b.id, m, &h, &sign(&b, b.id, &c, 5)).unwrap_err().contains("does not match"));
        // B's key but A's origin
        let c = svc.issue_step_up(b.id, m, &h);
        assert!(svc.verify_step_up(b.id, m, &h, &sign(&b, a.id, &c, 5)).unwrap_err().contains("origin"));
        // A's key presented for B (unknown credential)
        let c = svc.issue_step_up(b.id, m, &h);
        assert!(svc.verify_step_up(b.id, m, &h, &sign(&a, b.id, &c, 5)).is_err());
        // B still fine afterwards, counter persisted
        let c = svc.issue_step_up(b.id, m, &h);
        svc.verify_step_up(b.id, m, &h, &sign(&b, b.id, &c, 5)).unwrap();
        assert_eq!(svc.device(b.id).unwrap().step_up.unwrap().sign_count, 5);
        // A continues with a higher counter
        let c = svc.issue_step_up(a.id, m, &h);
        svc.verify_step_up(a.id, m, &h, &sign(&a, a.id, &c, 2)).unwrap();
    }

    #[tokio::test]
    async fn webauthn_credential_still_works() {
        use orkestr_security::webauthn::testing::SoftAuthenticator;
        let svc = RemoteService::new();
        let rp = RelyingParty { rp_id: "localhost".into(), origins: vec!["http://localhost:8787".into()] };
        svc.state.lock().rp = Some(rp.clone());
        let d = device(&svc);
        let mut auth = SoftAuthenticator::new(rp);
        svc.register_step_up(d.id, &auth.register(b"in-e2e")).await.unwrap();
        assert_eq!(svc.device(d.id).unwrap().info().step_up_kind.as_deref(), Some("webauthn"));
        let h = [2u8; 32];
        let c = svc.issue_step_up(d.id, "devices.revoke", &h);
        let asn = serde_json::to_value(auth.assert(&webauthn::unb64(&c).unwrap())).unwrap();
        svc.verify_step_up(d.id, "devices.revoke", &h, &asn).unwrap();
        // A native-shaped assertion is not accepted for a WebAuthn credential.
        let c = svc.issue_step_up(d.id, "devices.revoke", &h);
        assert!(svc.verify_step_up(d.id, "devices.revoke", &h, &sign(&d, d.id, &c, 9)).is_err());
    }

    #[test]
    fn legacy_record_defaults_to_webauthn() {
        let v = json!({"deviceId": DeviceId::new(), "name": "x", "platform": "web", "publicKey": "", "role": "admin",
            "remoteTerminal": false, "createdAt": chrono::Utc::now()});
        let r: DeviceRecord = serde_json::from_value(v).unwrap();
        assert_eq!(r.step_up_kind, StepUpKind::WebAuthn);
    }
}
