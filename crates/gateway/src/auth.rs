//! Account authentication: passkeys (WebAuthn ES256, attestation `none`),
//! TOTP fallback, session cookies, core enrollment tokens.
//!
//! Endpoints (JSON; binary values base64url):
//! * `GET  /v1/auth/state` → `{hasAccount, authenticated, accountId?, rpId, origins, totpEnabled}`
//! * `POST /v1/auth/register/begin {name?}` → `{ceremonyId, publicKey}` (first account, or signed in to add a passkey)
//! * `POST /v1/auth/register/finish {ceremonyId, credential}` → `{token, accountId}` + cookie
//! * `POST /v1/auth/login/begin` → `{ceremonyId, publicKey}`
//! * `POST /v1/auth/login/finish {ceremonyId, credential}` → `{token, accountId}` + cookie
//! * `POST /v1/auth/totp/setup` (session) → `{secret, uri}`; `POST /v1/auth/totp/confirm {code}` (session)
//! * `POST /v1/auth/totp/login {code}` → `{token, accountId}` + cookie
//! * `POST /v1/auth/logout`; `POST /v1/auth/panic` (session): drop every session and tunnel of the account
//! * `POST /v1/cores/enrollment-token` (session) → `{token, expiresAt}`; `GET /v1/cores` (session)
//! * `GET  /v1/audit` (session)

use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use orkestr_security::{totp, webauthn};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{random_bytes, ApiError, ClientIp, GatewayState};

/// Session cookie name.
pub const COOKIE: &str = "orkestr_session";
const CEREMONY_TTL: Duration = Duration::from_secs(5 * 60);

/// A pending WebAuthn ceremony.
#[derive(Debug, Clone)]
pub struct Ceremony {
    challenge: Vec<u8>,
    register: bool,
    account: Option<String>,
    name: String,
    created: Instant,
}

/// Extracts the bearer token: `Authorization: Bearer`, cookie, or `?token=`.
#[must_use]
pub fn bearer(headers: &HeaderMap, query_token: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_owned());
        }
    }
    if let Some(c) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for part in c.split(';') {
            if let Some(v) = part.trim().strip_prefix(&format!("{COOKIE}=")) {
                return Some(v.to_owned());
            }
        }
    }
    query_token.map(str::to_owned)
}

/// Requires a valid session; returns the account id.
///
/// # Errors
/// 401.
pub fn require(st: &GatewayState, headers: &HeaderMap) -> Result<String, ApiError> {
    bearer(headers, None)
        .and_then(|t| st.store.session(&t))
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "sign in first"))
}

fn session_response(st: &GatewayState, account: &str) -> Response {
    let token = st.store.create_session(account);
    let secure = if st.secure_cookie { "; Secure" } else { "" };
    let cookie = format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        crate::store::SESSION_TTL_SECS
    );
    let mut res = Json(json!({ "token": token, "accountId": account })).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

fn login_limits(st: &GatewayState, ip: &str) -> Result<(), ApiError> {
    if let Err(d) = st.lockout.check(&format!("ip:{ip}")) {
        return Err(ApiError::too_many(d));
    }
    st.windows.hit(&format!("login-ip:{ip}"), 10, Duration::from_secs(60)).map_err(ApiError::too_many)
}

fn account_limit(st: &GatewayState, account: &str) -> Result<(), ApiError> {
    st.windows.hit(&format!("login-acct:{account}"), 20, Duration::from_secs(3600)).map_err(ApiError::too_many)
}

fn new_ceremony(st: &GatewayState, c: Ceremony) -> String {
    let id = crate::random_token();
    let mut map = st.ceremonies.lock();
    map.retain(|_, c| c.created.elapsed() < CEREMONY_TTL);
    map.insert(id.clone(), c);
    id
}

fn take_ceremony(st: &GatewayState, id: &str, register: bool) -> Result<Ceremony, ApiError> {
    let c = st.ceremonies.lock().remove(id).ok_or_else(|| ApiError::bad("unknown or expired ceremony"))?;
    if c.register != register || c.created.elapsed() >= CEREMONY_TTL {
        return Err(ApiError::bad("unknown or expired ceremony"));
    }
    Ok(c)
}

/// `GET /v1/auth/state`.
pub async fn state(State(st): State<GatewayState>, headers: HeaderMap) -> Json<Value> {
    let account = bearer(&headers, None).and_then(|t| st.store.session(&t));
    let owner = st.store.account();
    let totp_enabled = owner.as_deref().and_then(|a| st.store.totp(a)).is_some_and(|(_, e, _)| e);
    Json(json!({
        "hasAccount": owner.is_some(), "authenticated": account.is_some(), "accountId": account,
        "rpId": st.rp.rp_id, "origins": st.rp.origins, "totpEnabled": totp_enabled
    }))
}

/// Body of `register/begin`.
#[derive(Debug, Default, Deserialize)]
pub struct RegisterBegin {
    #[serde(default)]
    name: Option<String>,
}

/// `POST /v1/auth/register/begin`.
pub async fn register_begin(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: Option<Json<RegisterBegin>>,
) -> Result<Json<Value>, ApiError> {
    login_limits(&st, &ip)?;
    let signed_in = bearer(&headers, None).and_then(|t| st.store.session(&t));
    if st.store.account().is_some() && signed_in.is_none() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "an account exists: sign in to add a passkey"));
    }
    let name = body.and_then(|b| b.0.name).unwrap_or_else(|| "owner".into()).chars().take(64).collect::<String>();
    let challenge = random_bytes(32);
    let user_id = signed_in.clone().unwrap_or_else(|| "new-owner".into());
    let exclude: Vec<Value> =
        st.store.credential_ids().iter().map(|id| json!({ "type": "public-key", "id": webauthn::b64(id) })).collect();
    let id = new_ceremony(
        &st,
        Ceremony {
            challenge: challenge.clone(),
            register: true,
            account: signed_in,
            name: name.clone(),
            created: Instant::now(),
        },
    );
    Ok(Json(json!({
        "ceremonyId": id,
        "publicKey": {
            "challenge": webauthn::b64(&challenge),
            "rp": { "id": st.rp.rp_id, "name": "Orkestr" },
            "user": { "id": webauthn::b64(user_id.as_bytes()), "name": name, "displayName": name },
            "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "preferred", "userVerification": "required" },
            "excludeCredentials": exclude,
            "timeout": 60000
        }
    })))
}

/// Body of the `*/finish` calls.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finish {
    ceremony_id: String,
    credential: Value,
}

/// `POST /v1/auth/register/finish`.
pub async fn register_finish(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    Json(f): Json<Finish>,
) -> Result<Response, ApiError> {
    login_limits(&st, &ip)?;
    let c = take_ceremony(&st, &f.ceremony_id, true)?;
    let reg: webauthn::RegistrationJson =
        serde_json::from_value(f.credential).map_err(|e| ApiError::bad(format!("credential: {e}")))?;
    let cd = webauthn::unb64(&reg.response.client_data_json).map_err(ApiError::bad_e)?;
    let ao = webauthn::unb64(&reg.response.attestation_object).map_err(ApiError::bad_e)?;
    let cred = match webauthn::verify_registration(&st.rp, Some(&c.challenge), &cd, &ao) {
        Ok(c) => c,
        Err(e) => {
            st.lockout.fail(&format!("ip:{ip}"));
            st.store.audit(None, "auth.register.failed", Some(&ip), &json!({ "reason": e.to_string() }));
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, e.to_string()));
        }
    };
    let account = match c.account {
        Some(a) => a,
        None => {
            if st.store.account().is_some() {
                return Err(ApiError::new(StatusCode::FORBIDDEN, "an account already exists"));
            }
            st.store.create_account(&c.name).map_err(ApiError::internal)?
        }
    };
    st.store
        .add_credential(&account, &cred)
        .map_err(|_| ApiError::new(StatusCode::CONFLICT, "credential already registered"))?;
    st.lockout.success(&format!("ip:{ip}"));
    st.store.audit(Some(&account), "auth.register", Some(&ip), &json!({ "credentialId": webauthn::b64(&cred.id) }));
    Ok(session_response(&st, &account))
}

/// `POST /v1/auth/login/begin`.
pub async fn login_begin(State(st): State<GatewayState>, ClientIp(ip): ClientIp) -> Result<Json<Value>, ApiError> {
    login_limits(&st, &ip)?;
    let challenge = random_bytes(32);
    let allow: Vec<Value> =
        st.store.credential_ids().iter().map(|id| json!({ "type": "public-key", "id": webauthn::b64(id) })).collect();
    let id = new_ceremony(
        &st,
        Ceremony {
            challenge: challenge.clone(),
            register: false,
            account: None,
            name: String::new(),
            created: Instant::now(),
        },
    );
    Ok(Json(json!({
        "ceremonyId": id,
        "publicKey": { "challenge": webauthn::b64(&challenge), "rpId": st.rp.rp_id, "allowCredentials": allow, "userVerification": "required", "timeout": 60000 }
    })))
}

/// `POST /v1/auth/login/finish`.
pub async fn login_finish(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    Json(f): Json<Finish>,
) -> Result<Response, ApiError> {
    login_limits(&st, &ip)?;
    let c = take_ceremony(&st, &f.ceremony_id, false)?;
    let a = webauthn::AssertionJson::from_value(&f.credential).map_err(ApiError::bad_e)?;
    let id = webauthn::unb64(&a.credential_id).map_err(ApiError::bad_e)?;
    let fail = |st: &GatewayState, reason: String| {
        st.lockout.fail(&format!("ip:{ip}"));
        st.store.audit(None, "auth.login.failed", Some(&ip), &json!({ "reason": reason }));
        ApiError::new(StatusCode::UNAUTHORIZED, "passkey sign-in failed")
    };
    let Some(row) = st.store.credential(&id) else { return Err(fail(&st, "unknown credential".into())) };
    account_limit(&st, &row.account_id)?;
    match webauthn::verify_assertion_json(&st.rp, &c.challenge, &row.credential, &a) {
        Ok(count) => {
            st.store.set_sign_count(&id, count);
            st.lockout.success(&format!("ip:{ip}"));
            st.store.audit(Some(&row.account_id), "auth.login", Some(&ip), &json!({ "method": "passkey" }));
            Ok(session_response(&st, &row.account_id))
        }
        Err(e) => Err(fail(&st, e.to_string())),
    }
}

/// `POST /v1/auth/totp/setup` (session): new secret (enabled after `confirm`).
pub async fn totp_setup(State(st): State<GatewayState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    let secret = totp::generate_secret();
    st.store.set_totp(&account, &secret, false);
    Ok(Json(json!({ "secret": totp::base32(&secret), "uri": totp::otpauth_uri(&secret, "owner", "Orkestr") })))
}

/// Body with a TOTP code.
#[derive(Debug, Deserialize)]
pub struct Code {
    code: String,
}

fn check_totp(st: &GatewayState, account: &str, code: &str, require_enabled: bool) -> bool {
    let Some((secret, enabled, last)) = st.store.totp(account) else { return false };
    if require_enabled && !enabled {
        return false;
    }
    let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0);
    match totp::verify(&secret, code.trim(), now) {
        Some(step) if step > last => {
            st.store.set_totp_step(account, step);
            true
        }
        _ => false,
    }
}

/// `POST /v1/auth/totp/confirm` (session).
pub async fn totp_confirm(
    State(st): State<GatewayState>,
    headers: HeaderMap,
    Json(c): Json<Code>,
) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    let Some((secret, _, _)) = st.store.totp(&account) else { return Err(ApiError::bad("call totp/setup first")) };
    if !check_totp(&st, &account, &c.code, false) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong code"));
    }
    st.store.set_totp(&account, &secret, true);
    st.store.audit(Some(&account), "auth.totp.enabled", None, &Value::Null);
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/auth/totp/login`.
pub async fn totp_login(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    Json(c): Json<Code>,
) -> Result<Response, ApiError> {
    login_limits(&st, &ip)?;
    let Some(account) = st.store.account() else { return Err(ApiError::new(StatusCode::UNAUTHORIZED, "no account")) };
    account_limit(&st, &account)?;
    if check_totp(&st, &account, &c.code, true) {
        st.lockout.success(&format!("ip:{ip}"));
        st.store.audit(Some(&account), "auth.login", Some(&ip), &json!({ "method": "totp" }));
        Ok(session_response(&st, &account))
    } else {
        st.lockout.fail(&format!("ip:{ip}"));
        st.store.audit(Some(&account), "auth.login.failed", Some(&ip), &json!({ "method": "totp" }));
        Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong code"))
    }
}

/// `POST /v1/auth/logout`.
pub async fn logout(State(st): State<GatewayState>, headers: HeaderMap) -> Response {
    if let Some(t) = bearer(&headers, None) {
        st.store.delete_session(&t);
    }
    let mut res = Json(json!({ "ok": true })).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/// `POST /v1/cores/enrollment-token` (session).
pub async fn enrollment_token(State(st): State<GatewayState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    let (token, exp) = st.store.create_enrollment_token(&account);
    st.store.audit(Some(&account), "cores.enrollmentToken", None, &Value::Null);
    Ok(Json(json!({ "token": token, "expiresAt": exp })))
}

/// `GET /v1/cores` (session).
pub async fn cores(State(st): State<GatewayState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    let list: Vec<Value> = st
        .store
        .cores(&account)
        .into_iter()
        .map(|c| {
            let devices: Vec<Value> = st
                .store
                .devices(&c.core_id)
                .into_iter()
                .map(|(id, name, role, status)| json!({ "deviceId": id, "name": name, "role": role, "status": status }))
                .collect();
            json!({
                "coreId": c.core_id, "name": c.name, "online": st.relay.is_online(&c.core_id),
                "lastSeen": c.last_seen, "cikPublic": c.cik_public, "devices": devices
            })
        })
        .collect();
    Ok(Json(json!({ "cores": list })))
}

/// `GET /v1/audit` (session).
pub async fn audit(State(st): State<GatewayState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    Ok(Json(json!({ "entries": st.store.audit_list(&account, 500) })))
}

/// `POST /v1/auth/panic` (session): drops every session and core tunnel of
/// the account (remote-access.md §8 "account panic").
pub async fn panic(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account = require(&st, &headers)?;
    let sessions = st.store.delete_sessions(&account);
    st.relay.drop_account(&account);
    st.store.audit(Some(&account), "auth.panic", Some(&ip), &json!({ "sessions": sessions }));
    Ok(Json(json!({ "ok": true, "sessionsDropped": sessions })))
}
