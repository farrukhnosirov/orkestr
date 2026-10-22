//! Minimal WebAuthn (passkey) verifier: ES256 only, attestation `none`
//! (Phase 4, ADR-0030). Used by the gateway (account login) and by the core
//! (step-up assertions verified against core-held credentials, remote-access.md §6.3).
//!
//! Checks performed:
//! * `clientDataJSON`: `type`, `challenge` (base64url), `origin` ∈ allowed origins;
//! * `authenticatorData`: `rpIdHash == SHA-256(rpId)`, flags UP + UV, sign counter
//!   strictly increasing (unless both are 0);
//! * registration: `attestationObject` CBOR with `fmt = "none"` and an EC2/ES256 COSE key;
//! * assertion: ECDSA P-256 (DER) over `authenticatorData ‖ SHA-256(clientDataJSON)`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ciborium::value::Value as Cbor;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

/// Verification failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebAuthnError {
    /// Malformed input.
    #[error("malformed WebAuthn data: {0}")]
    Malformed(String),
    /// A check failed.
    #[error("WebAuthn verification failed: {0}")]
    Invalid(&'static str),
}

fn bad(m: impl Into<String>) -> WebAuthnError {
    WebAuthnError::Malformed(m.into())
}

/// Relying party settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelyingParty {
    /// RP id (`localhost` by default).
    pub rp_id: String,
    /// Allowed origins (`http://localhost:8787`, …).
    pub origins: Vec<String>,
}

/// A registered ES256 credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    /// Credential id (raw bytes).
    #[serde(with = "b64")]
    pub id: Vec<u8>,
    /// SEC1 uncompressed P-256 public key.
    #[serde(with = "b64")]
    pub public_key: Vec<u8>,
    /// Last seen signature counter.
    pub sign_count: u32,
}

mod b64 {
    use super::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(serde::de::Error::custom)
    }
}

/// base64url decode (padding tolerated).
///
/// # Errors
/// Invalid base64url.
pub fn unb64(s: &str) -> Result<Vec<u8>, WebAuthnError> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| bad(format!("base64url: {e}")))
}

/// base64url encode (no padding).
#[must_use]
pub fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// `PublicKeyCredential` JSON of a registration (`navigator.credentials.create`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationJson {
    /// Response.
    pub response: RegistrationResponseJson,
}

/// `AuthenticatorAttestationResponse` (base64url fields).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponseJson {
    /// `clientDataJSON`.
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    /// `attestationObject`.
    pub attestation_object: String,
}

/// Assertion in flat JSON form (base64url fields) — also accepts the
/// `PublicKeyCredential` shape via [`AssertionJson::from_value`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionJson {
    /// Credential id.
    pub credential_id: String,
    /// `authenticatorData`.
    pub authenticator_data: String,
    /// `clientDataJSON`.
    #[serde(alias = "clientDataJSON")]
    pub client_data_json: String,
    /// DER signature.
    pub signature: String,
}

impl AssertionJson {
    /// Accepts either the flat form or a `PublicKeyCredential` JSON
    /// (`{id, response:{authenticatorData, clientDataJSON, signature}}`).
    ///
    /// # Errors
    /// Neither shape matches.
    pub fn from_value(v: &serde_json::Value) -> Result<Self, WebAuthnError> {
        if let Ok(a) = serde_json::from_value::<Self>(v.clone()) {
            return Ok(a);
        }
        let r = v.get("response").ok_or_else(|| bad("assertion"))?;
        let s = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        Ok(Self {
            credential_id: s(v, "rawId").or_else(|| s(v, "id")).ok_or_else(|| bad("assertion id"))?,
            authenticator_data: s(r, "authenticatorData").ok_or_else(|| bad("authenticatorData"))?,
            client_data_json: s(r, "clientDataJSON").ok_or_else(|| bad("clientDataJSON"))?,
            signature: s(r, "signature").ok_or_else(|| bad("signature"))?,
        })
    }
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    typ: String,
    challenge: String,
    origin: String,
}

/// Returns the raw challenge carried in `clientDataJSON`.
///
/// # Errors
/// Malformed client data.
pub fn client_data_challenge(client_data_json: &[u8]) -> Result<Vec<u8>, WebAuthnError> {
    let cd: ClientData = serde_json::from_slice(client_data_json).map_err(|e| bad(format!("clientDataJSON: {e}")))?;
    unb64(&cd.challenge)
}

fn check_client_data(rp: &RelyingParty, raw: &[u8], typ: &str, challenge: Option<&[u8]>) -> Result<(), WebAuthnError> {
    let cd: ClientData = serde_json::from_slice(raw).map_err(|e| bad(format!("clientDataJSON: {e}")))?;
    if cd.typ != typ {
        return Err(WebAuthnError::Invalid("clientData.type"));
    }
    if let Some(expected) = challenge {
        let got = unb64(&cd.challenge)?;
        if !bool::from(subtle::ConstantTimeEq::ct_eq(got.as_slice(), expected)) {
            return Err(WebAuthnError::Invalid("challenge"));
        }
    }
    if !rp.origins.iter().any(|o| o == &cd.origin) {
        return Err(WebAuthnError::Invalid("origin"));
    }
    Ok(())
}

struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    attested: &'a [u8],
}

fn parse_auth_data<'a>(rp: &RelyingParty, ad: &'a [u8]) -> Result<AuthData<'a>, WebAuthnError> {
    if ad.len() < 37 {
        return Err(bad("authenticatorData too short"));
    }
    let rp_hash = Sha256::digest(rp.rp_id.as_bytes());
    if ad[..32] != rp_hash[..] {
        return Err(WebAuthnError::Invalid("rpIdHash"));
    }
    let flags = ad[32];
    if flags & FLAG_UP == 0 {
        return Err(WebAuthnError::Invalid("user presence"));
    }
    if flags & FLAG_UV == 0 {
        return Err(WebAuthnError::Invalid("user verification"));
    }
    let sign_count = u32::from_be_bytes([ad[33], ad[34], ad[35], ad[36]]);
    Ok(AuthData { flags, sign_count, attested: &ad[37..] })
}

fn cbor_get<'a>(map: &'a [(Cbor, Cbor)], key: &Cbor) -> Option<&'a Cbor> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn cose_es256_to_sec1(cose: &Cbor) -> Result<Vec<u8>, WebAuthnError> {
    let Cbor::Map(m) = cose else { return Err(bad("COSE key")) };
    let int = |i: i64| Cbor::Integer(i.into());
    if cbor_get(m, &int(1)) != Some(&int(2))
        || cbor_get(m, &int(3)) != Some(&int(-7))
        || cbor_get(m, &int(-1)) != Some(&int(1))
    {
        return Err(WebAuthnError::Invalid("only ES256 (EC2 P-256) credentials are supported"));
    }
    let (Some(Cbor::Bytes(x)), Some(Cbor::Bytes(y))) = (cbor_get(m, &int(-2)), cbor_get(m, &int(-3))) else {
        return Err(bad("COSE x/y"));
    };
    if x.len() != 32 || y.len() != 32 {
        return Err(bad("COSE coordinate length"));
    }
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(4);
    sec1.extend_from_slice(x);
    sec1.extend_from_slice(y);
    VerifyingKey::from_sec1_bytes(&sec1).map_err(|_| bad("public key not on curve"))?;
    Ok(sec1)
}

/// Verifies a registration (`attestation: "none"`) and returns the credential.
/// `challenge = None` skips the challenge check (only for registrations made
/// inside an already-authenticated channel, e.g. the E2E session).
///
/// # Errors
/// Any failed check.
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: Option<&[u8]>,
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<Credential, WebAuthnError> {
    check_client_data(rp, client_data_json, "webauthn.create", challenge)?;
    let att: Cbor =
        ciborium::de::from_reader(attestation_object).map_err(|e| bad(format!("attestationObject: {e}")))?;
    let Cbor::Map(att) = att else { return Err(bad("attestationObject")) };
    let fmt = cbor_get(&att, &Cbor::Text("fmt".into()));
    if fmt != Some(&Cbor::Text("none".into())) {
        return Err(WebAuthnError::Invalid("only attestation format \"none\" is supported"));
    }
    let Some(Cbor::Bytes(auth_data)) = cbor_get(&att, &Cbor::Text("authData".into())) else {
        return Err(bad("authData"));
    };
    let ad = parse_auth_data(rp, auth_data)?;
    if ad.flags & FLAG_AT == 0 || ad.attested.len() < 18 {
        return Err(WebAuthnError::Invalid("attested credential data missing"));
    }
    let id_len = usize::from(u16::from_be_bytes([ad.attested[16], ad.attested[17]]));
    let rest = &ad.attested[18..];
    if rest.len() < id_len || id_len == 0 || id_len > 1023 {
        return Err(bad("credential id"));
    }
    let id = rest[..id_len].to_vec();
    let cose: Cbor = ciborium::de::from_reader(&rest[id_len..]).map_err(|e| bad(format!("COSE: {e}")))?;
    let public_key = cose_es256_to_sec1(&cose)?;
    Ok(Credential { id, public_key, sign_count: ad.sign_count })
}

/// Verifies an assertion for `cred`; returns the new signature counter.
///
/// # Errors
/// Any failed check.
pub fn verify_assertion(
    rp: &RelyingParty,
    challenge: &[u8],
    cred: &Credential,
    authenticator_data: &[u8],
    client_data_json: &[u8],
    signature_der: &[u8],
) -> Result<u32, WebAuthnError> {
    check_client_data(rp, client_data_json, "webauthn.get", Some(challenge))?;
    let ad = parse_auth_data(rp, authenticator_data)?;
    if (ad.sign_count != 0 || cred.sign_count != 0) && ad.sign_count <= cred.sign_count {
        return Err(WebAuthnError::Invalid("signature counter did not increase (cloned authenticator?)"));
    }
    let key = VerifyingKey::from_sec1_bytes(&cred.public_key).map_err(|_| bad("stored public key"))?;
    let sig = Signature::from_der(signature_der)
        .or_else(|_| Signature::from_slice(signature_der))
        .map_err(|_| bad("signature encoding"))?;
    let mut msg = authenticator_data.to_vec();
    msg.extend_from_slice(&Sha256::digest(client_data_json));
    key.verify(&msg, &sig).map_err(|_| WebAuthnError::Invalid("signature"))?;
    Ok(ad.sign_count)
}

/// Verifies an [`AssertionJson`]; checks the credential id matches `cred`.
///
/// # Errors
/// Any failed check.
pub fn verify_assertion_json(
    rp: &RelyingParty,
    challenge: &[u8],
    cred: &Credential,
    a: &AssertionJson,
) -> Result<u32, WebAuthnError> {
    if unb64(&a.credential_id)? != cred.id {
        return Err(WebAuthnError::Invalid("unknown credential"));
    }
    verify_assertion(
        rp,
        challenge,
        cred,
        &unb64(&a.authenticator_data)?,
        &unb64(&a.client_data_json)?,
        &unb64(&a.signature)?,
    )
}

/// A software authenticator (tests and the Rust device test client only).
pub mod testing {
    use super::{b64, AssertionJson, Cbor, RelyingParty, FLAG_AT, FLAG_UP, FLAG_UV};
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};
    use sha2::{Digest, Sha256};

    /// Soft ES256 authenticator with user verification.
    #[derive(Debug, Clone)]
    pub struct SoftAuthenticator {
        key: SigningKey,
        /// Credential id.
        pub credential_id: Vec<u8>,
        rp: RelyingParty,
        counter: u32,
    }

    impl SoftAuthenticator {
        /// New authenticator for `rp` (first origin is used).
        #[must_use]
        pub fn new(rp: RelyingParty) -> Self {
            let key = SigningKey::random(&mut rand_core06::OsRng);
            let credential_id = Sha256::digest(key.verifying_key().to_encoded_point(false).as_bytes())[..16].to_vec();
            Self { key, credential_id, rp, counter: 0 }
        }

        fn client_data(&self, typ: &str, challenge: &[u8]) -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({
                "type": typ, "challenge": b64(challenge), "origin": self.rp.origins[0], "crossOrigin": false
            }))
            .unwrap_or_default()
        }

        fn auth_data(&self, flags: u8, extra: &[u8]) -> Vec<u8> {
            let mut ad = Sha256::digest(self.rp.rp_id.as_bytes()).to_vec();
            ad.push(flags);
            ad.extend_from_slice(&self.counter.to_be_bytes());
            ad.extend_from_slice(extra);
            ad
        }

        /// `navigator.credentials.create` → `PublicKeyCredential` JSON.
        #[must_use]
        pub fn register(&mut self, challenge: &[u8]) -> serde_json::Value {
            let point = self.key.verifying_key().to_encoded_point(false);
            let int = |i: i64| Cbor::Integer(i.into());
            let cose = Cbor::Map(vec![
                (int(1), int(2)),
                (int(3), int(-7)),
                (int(-1), int(1)),
                (int(-2), Cbor::Bytes(point.x().map(|x| x.to_vec()).unwrap_or_default())),
                (int(-3), Cbor::Bytes(point.y().map(|y| y.to_vec()).unwrap_or_default())),
            ]);
            let mut attested = vec![0u8; 16];
            #[allow(clippy::cast_possible_truncation)]
            attested.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
            attested.extend_from_slice(&self.credential_id);
            let _ = ciborium::ser::into_writer(&cose, &mut attested);
            let auth_data = self.auth_data(FLAG_UP | FLAG_UV | FLAG_AT, &attested);
            let att = Cbor::Map(vec![
                (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
                (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
                (Cbor::Text("authData".into()), Cbor::Bytes(auth_data)),
            ]);
            let mut att_bytes = Vec::new();
            let _ = ciborium::ser::into_writer(&att, &mut att_bytes);
            serde_json::json!({
                "id": b64(&self.credential_id), "rawId": b64(&self.credential_id), "type": "public-key",
                "response": { "clientDataJSON": b64(&self.client_data("webauthn.create", challenge)), "attestationObject": b64(&att_bytes) }
            })
        }

        /// `navigator.credentials.get` → flat assertion.
        pub fn assert(&mut self, challenge: &[u8]) -> AssertionJson {
            self.counter += 1;
            let ad = self.auth_data(FLAG_UP | FLAG_UV, &[]);
            let cd = self.client_data("webauthn.get", challenge);
            let mut msg = ad.clone();
            msg.extend_from_slice(&Sha256::digest(&cd));
            let sig: Signature = self.key.sign(&msg);
            AssertionJson {
                credential_id: b64(&self.credential_id),
                authenticator_data: b64(&ad),
                client_data_json: b64(&cd),
                signature: b64(sig.to_der().as_bytes()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::SoftAuthenticator;
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty { rp_id: "localhost".into(), origins: vec!["http://localhost:8787".into()] }
    }

    fn registered() -> (SoftAuthenticator, Credential) {
        let mut a = SoftAuthenticator::new(rp());
        let j = a.register(b"reg-challenge");
        let r: RegistrationJson = serde_json::from_value(j).unwrap();
        let cred = verify_registration(
            &rp(),
            Some(b"reg-challenge"),
            &unb64(&r.response.client_data_json).unwrap(),
            &unb64(&r.response.attestation_object).unwrap(),
        )
        .unwrap();
        assert_eq!(cred.id, a.credential_id);
        (a, cred)
    }

    #[test]
    fn register_and_assert() {
        let (mut a, mut cred) = registered();
        let asn = a.assert(b"login");
        cred.sign_count = verify_assertion_json(&rp(), b"login", &cred, &asn).unwrap();
        assert_eq!(cred.sign_count, 1);
        // replay of the same assertion: counter did not increase
        assert_eq!(
            verify_assertion_json(&rp(), b"login", &cred, &asn),
            Err(WebAuthnError::Invalid("signature counter did not increase (cloned authenticator?)"))
        );
    }

    #[test]
    fn wrong_challenge_origin_rp_rejected() {
        let (mut a, cred) = registered();
        let asn = a.assert(b"one");
        assert_eq!(verify_assertion_json(&rp(), b"two", &cred, &asn), Err(WebAuthnError::Invalid("challenge")));
        let other = RelyingParty { rp_id: "localhost".into(), origins: vec!["https://evil.example".into()] };
        assert_eq!(verify_assertion_json(&other, b"one", &cred, &asn), Err(WebAuthnError::Invalid("origin")));
        let other_rp = RelyingParty { rp_id: "example.com".into(), origins: rp().origins };
        assert_eq!(verify_assertion_json(&other_rp, b"one", &cred, &asn), Err(WebAuthnError::Invalid("rpIdHash")));
    }

    #[test]
    fn tampered_signature_rejected() {
        let (mut a, cred) = registered();
        let mut asn = a.assert(b"c");
        let mut ad = unb64(&asn.authenticator_data).unwrap();
        ad[36] ^= 0x10; // counter byte
        asn.authenticator_data = b64(&ad);
        assert_eq!(verify_assertion_json(&rp(), b"c", &cred, &asn), Err(WebAuthnError::Invalid("signature")));
    }

    /// Fixed vector: P-256 key d = SHA-256("webauthn-vector"), RFC 6979 signature.
    #[test]
    fn fixed_vector_verifies() {
        use p256::ecdsa::signature::Signer;
        use p256::ecdsa::SigningKey;
        let d: [u8; 32] = Sha256::digest(b"webauthn-vector").into();
        let key = SigningKey::from_slice(&d).unwrap();
        let mut ad = Sha256::digest(b"localhost").to_vec();
        ad.extend_from_slice(&[0x05, 0, 0, 0, 7]);
        let cd = br#"{"type":"webauthn.get","challenge":"AAECAw","origin":"http://localhost:8787"}"#;
        let mut msg = ad.clone();
        msg.extend_from_slice(&Sha256::digest(cd));
        let sig: Signature = key.sign(&msg);
        let cred = Credential {
            id: vec![1],
            public_key: key.verifying_key().to_encoded_point(false).as_bytes().to_vec(),
            sign_count: 6,
        };
        assert_eq!(verify_assertion(&rp(), &[0, 1, 2, 3], &cred, &ad, cd, sig.to_der().as_bytes()), Ok(7));
        // UV flag cleared → rejected
        let mut ad2 = ad.clone();
        ad2[32] = 0x01;
        assert_eq!(
            verify_assertion(&rp(), &[0, 1, 2, 3], &cred, &ad2, cd, sig.to_der().as_bytes()),
            Err(WebAuthnError::Invalid("user verification"))
        );
    }
}
