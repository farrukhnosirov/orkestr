//! Native push delivery (Phase 5, ADR-0037): FCM HTTP v1 (Android) and APNs
//! (iOS). The core sends an envelope `{"k": category, "p": {"n", "c"}}` whose
//! `p` is AES-256-GCM ciphertext under a key only the phone and the core
//! know; the gateway forwards `p` (as JSON text) and `k` next to a generic
//! visible alert ("Orkestr" / "Action needed") and never sees plaintext.
//!
//! * FCM: service-account JSON → RS256 JWT → OAuth token (`token_uri`, cached)
//!   → `POST {endpoint}/v1/projects/{project_id}/messages:send`.
//! * APNs: ES256 provider JWT (`.p8` key, cached ~50 min) → HTTP/2
//!   `POST {endpoint}/3/device/{token}` (prior-knowledge h2c for `http://`
//!   mock endpoints).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use orkestr_protocol::methods::push::PushTransport;
use serde::Deserialize;

/// Default FCM API base.
pub const DEFAULT_FCM_ENDPOINT: &str = "https://fcm.googleapis.com";
/// Default APNs API base (production).
pub const DEFAULT_APNS_ENDPOINT: &str = "https://api.push.apple.com";
/// OAuth scope for FCM HTTP v1.
pub const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Visible alert title (generic: lock screens and push providers see only this).
pub const ALERT_TITLE: &str = "Orkestr";
/// Visible alert body.
pub const ALERT_BODY: &str = "Action needed";

/// Native push errors.
#[derive(Debug, thiserror::Error)]
pub enum NativePushError {
    /// No sender configured for this transport.
    #[error("{0} push is not configured on this gateway")]
    NotConfigured(&'static str),
    /// Bad configuration (key files, service account).
    #[error("native push config: {0}")]
    Config(String),
    /// Malformed envelope from the core.
    #[error("invalid push envelope: {0}")]
    Envelope(String),
    /// The token is no longer valid (FCM `UNREGISTERED`/404, APNs 410/`Unregistered`).
    #[error("device token is gone ({0})")]
    Gone(u16),
    /// Temporary or unknown failure; may be retried.
    #[error("push delivery failed: {0}")]
    Retryable(String),
}

/// One native message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDelivery {
    /// FCM registration token / APNs device token.
    pub token: String,
    /// Category (`k`).
    pub category: String,
    /// Encrypted payload as JSON text (`p` = `{"n","c"}`).
    pub payload: String,
    /// Collapse key (already hashed by the core).
    pub collapse_key: Option<String>,
    /// `high` ⇒ FCM `HIGH` / APNs priority 10; otherwise `NORMAL` / 5.
    pub urgency: String,
}

impl NativeDelivery {
    /// Builds a delivery from the core's envelope bytes.
    ///
    /// # Errors
    /// Not `{"k": string, "p": {"n": string, "c": string}}`.
    pub fn from_envelope(
        token: String,
        envelope: &[u8],
        collapse_key: Option<String>,
        urgency: String,
    ) -> Result<Self, NativePushError> {
        let v: serde_json::Value =
            serde_json::from_slice(envelope).map_err(|e| NativePushError::Envelope(e.to_string()))?;
        let category = v.get("k").and_then(|k| k.as_str()).ok_or_else(|| NativePushError::Envelope("k".into()))?;
        let p = v
            .get("p")
            .filter(|p| p.get("n").is_some_and(serde_json::Value::is_string))
            .filter(|p| p.get("c").is_some_and(serde_json::Value::is_string))
            .ok_or_else(|| NativePushError::Envelope("p".into()))?;
        Ok(Self { token, category: category.to_owned(), payload: p.to_string(), collapse_key, urgency })
    }

    fn high(&self) -> bool {
        self.urgency == "high"
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

fn b64json(v: &serde_json::Value) -> String {
    URL_SAFE_NO_PAD.encode(v.to_string())
}

fn http_client(h2_prior_knowledge: bool) -> reqwest::Client {
    let b = reqwest::Client::builder().timeout(Duration::from_secs(15));
    let b = if h2_prior_knowledge { b.http2_prior_knowledge() } else { b };
    b.build().unwrap_or_default()
}

// ---------------------------------------------------------------- FCM

/// The fields of a Google service-account JSON the sender needs.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceAccount {
    /// Firebase project id.
    pub project_id: String,
    /// PKCS#8 PEM RSA key.
    pub private_key: String,
    /// JWT `iss`.
    pub client_email: String,
    /// OAuth token endpoint (JWT `aud`).
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".into()
}

/// FCM HTTP v1 sender.
pub struct FcmSender {
    sa: ServiceAccount,
    key: rsa::pkcs1v15::SigningKey<sha2::Sha256>,
    endpoint: String,
    http: reqwest::Client,
    token: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl std::fmt::Debug for FcmSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcmSender").field("project_id", &self.sa.project_id).finish_non_exhaustive()
    }
}

impl FcmSender {
    /// Sender from service-account JSON text.
    ///
    /// # Errors
    /// Invalid JSON or key.
    pub fn from_json(json: &str, endpoint: &str) -> Result<Self, NativePushError> {
        use rsa::pkcs8::DecodePrivateKey as _;
        let sa: ServiceAccount =
            serde_json::from_str(json).map_err(|e| NativePushError::Config(format!("service account: {e}")))?;
        let key = rsa::RsaPrivateKey::from_pkcs8_pem(&sa.private_key)
            .map_err(|e| NativePushError::Config(format!("service account private_key: {e}")))?;
        Ok(Self {
            sa,
            key: rsa::pkcs1v15::SigningKey::new(key),
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            http: http_client(false),
            token: tokio::sync::Mutex::new(None),
        })
    }

    /// Sender from a service-account JSON file.
    ///
    /// # Errors
    /// Unreadable file, invalid JSON or key.
    pub fn from_file(path: &Path, endpoint: &str) -> Result<Self, NativePushError> {
        let json =
            std::fs::read_to_string(path).map_err(|e| NativePushError::Config(format!("{}: {e}", path.display())))?;
        Self::from_json(&json, endpoint)
    }

    /// RS256 JWT for the OAuth token exchange, valid 1 h from `now`.
    #[must_use]
    pub fn jwt(&self, now: u64) -> String {
        use rsa::signature::{SignatureEncoding as _, Signer as _};
        let header = b64json(&serde_json::json!({ "alg": "RS256", "typ": "JWT" }));
        let claims = b64json(&serde_json::json!({
            "iss": self.sa.client_email, "scope": FCM_SCOPE, "aud": self.sa.token_uri, "iat": now, "exp": now + 3600,
        }));
        let input = format!("{header}.{claims}");
        let sig = self.key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }

    async fn access_token(&self) -> Result<String, NativePushError> {
        let mut cached = self.token.lock().await;
        if let Some((t, until)) = cached.as_ref() {
            if Instant::now() < *until {
                return Ok(t.clone());
            }
        }
        let body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}",
            self.jwt(now_secs())
        );
        let res = self
            .http
            .post(&self.sa.token_uri)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| NativePushError::Retryable(format!("fcm token: {e}")))?;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap_or_default();
        let Some(token) = v.get("access_token").and_then(|t| t.as_str()).filter(|_| status.is_success()) else {
            return Err(NativePushError::Retryable(format!("fcm token exchange failed ({status})")));
        };
        let ttl = v.get("expires_in").and_then(serde_json::Value::as_u64).unwrap_or(3600).saturating_sub(60).max(1);
        *cached = Some((token.to_owned(), Instant::now() + Duration::from_secs(ttl)));
        Ok(token.to_owned())
    }

    /// The FCM v1 request body for `msg`.
    #[must_use]
    pub fn message(msg: &NativeDelivery) -> serde_json::Value {
        let mut android = serde_json::json!({ "priority": if msg.high() { "HIGH" } else { "NORMAL" } });
        if let Some(ck) = &msg.collapse_key {
            android["collapse_key"] = ck.clone().into();
        }
        serde_json::json!({ "message": {
            "token": msg.token,
            "android": android,
            "notification": { "title": ALERT_TITLE, "body": ALERT_BODY },
            "data": { "p": msg.payload, "k": msg.category },
        }})
    }

    /// Sends one message.
    ///
    /// # Errors
    /// See [`NativePushError`].
    pub async fn send(&self, msg: &NativeDelivery) -> Result<(), NativePushError> {
        let token = self.access_token().await?;
        let url = format!("{}/v1/projects/{}/messages:send", self.endpoint, self.sa.project_id);
        let res = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&Self::message(msg))
            .send()
            .await
            .map_err(|e| NativePushError::Retryable(e.to_string()))?;
        let status = res.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let text = res.text().await.unwrap_or_default();
        if status == 401 {
            *self.token.lock().await = None;
        }
        if status == 404 || text.contains("UNREGISTERED") {
            return Err(NativePushError::Gone(status));
        }
        Err(NativePushError::Retryable(format!("fcm {status}: {}", text.chars().take(300).collect::<String>())))
    }
}

// ---------------------------------------------------------------- APNs

/// APNs configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApnsConfig {
    /// `.p8` token-signing key (PKCS#8 PEM).
    pub key_path: std::path::PathBuf,
    /// Key id (`kid`).
    pub key_id: String,
    /// Team id (`iss`).
    pub team_id: String,
    /// `apns-topic` (app bundle id).
    pub topic: String,
    /// API base.
    pub endpoint: String,
}

/// APNs provider-token sender (HTTP/2).
pub struct ApnsSender {
    key: p256::ecdsa::SigningKey,
    key_id: String,
    team_id: String,
    topic: String,
    endpoint: String,
    http: reqwest::Client,
    jwt: parking_lot::Mutex<Option<(String, Instant)>>,
}

impl std::fmt::Debug for ApnsSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsSender").field("topic", &self.topic).finish_non_exhaustive()
    }
}

/// Provider tokens are refreshed after this (Apple: between 20 and 60 min).
const APNS_JWT_TTL: Duration = Duration::from_secs(50 * 60);

impl ApnsSender {
    /// Sender from a PEM key.
    ///
    /// # Errors
    /// Invalid key.
    pub fn from_pem(
        pem: &str,
        key_id: &str,
        team_id: &str,
        topic: &str,
        endpoint: &str,
    ) -> Result<Self, NativePushError> {
        use p256::pkcs8::DecodePrivateKey as _;
        let key = p256::ecdsa::SigningKey::from_pkcs8_pem(pem.trim())
            .map_err(|e| NativePushError::Config(format!("apns key: {e}")))?;
        // HTTP/2 only; `http://` (mocks) means prior-knowledge h2c.
        Ok(Self {
            key,
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            http: http_client(true),
            jwt: parking_lot::Mutex::new(None),
        })
    }

    /// Sender from [`ApnsConfig`].
    ///
    /// # Errors
    /// Unreadable or invalid key.
    pub fn from_config(c: &ApnsConfig) -> Result<Self, NativePushError> {
        let pem = std::fs::read_to_string(&c.key_path)
            .map_err(|e| NativePushError::Config(format!("{}: {e}", c.key_path.display())))?;
        Self::from_pem(&pem, &c.key_id, &c.team_id, &c.topic, &c.endpoint)
    }

    /// ES256 provider JWT issued at `now`.
    #[must_use]
    pub fn jwt(&self, now: u64) -> String {
        use p256::ecdsa::signature::Signer as _;
        let header = b64json(&serde_json::json!({ "alg": "ES256", "kid": self.key_id }));
        let claims = b64json(&serde_json::json!({ "iss": self.team_id, "iat": now }));
        let input = format!("{header}.{claims}");
        let sig: p256::ecdsa::Signature = self.key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }

    fn provider_token(&self) -> String {
        let mut cached = self.jwt.lock();
        match cached.as_ref() {
            Some((t, at)) if at.elapsed() < APNS_JWT_TTL => t.clone(),
            _ => {
                let t = self.jwt(now_secs());
                *cached = Some((t.clone(), Instant::now()));
                t
            }
        }
    }

    /// The APNs JSON payload for `msg`.
    #[must_use]
    pub fn payload(msg: &NativeDelivery) -> serde_json::Value {
        serde_json::json!({
            "aps": { "alert": { "title": ALERT_TITLE, "body": ALERT_BODY }, "mutable-content": 1, "sound": "default" },
            "p": msg.payload,
            "k": msg.category,
        })
    }

    /// Sends one notification.
    ///
    /// # Errors
    /// See [`NativePushError`].
    pub async fn send(&self, msg: &NativeDelivery) -> Result<(), NativePushError> {
        if msg.token.is_empty() || !msg.token.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(NativePushError::Gone(400));
        }
        let url = format!("{}/3/device/{}", self.endpoint, msg.token);
        let mut req = self
            .http
            .post(url)
            .header("authorization", format!("bearer {}", self.provider_token()))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", if msg.high() { "10" } else { "5" })
            .json(&Self::payload(msg));
        if let Some(ck) = msg.collapse_key.as_deref().filter(|c| c.len() <= 64) {
            req = req.header("apns-collapse-id", ck);
        }
        let res = req.send().await.map_err(|e| NativePushError::Retryable(e.to_string()))?;
        let status = res.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let text = res.text().await.unwrap_or_default();
        if status == 403 && text.contains("ExpiredProviderToken") {
            *self.jwt.lock() = None;
        }
        if status == 410 || text.contains("Unregistered") || text.contains("BadDeviceToken") {
            return Err(NativePushError::Gone(status));
        }
        Err(NativePushError::Retryable(format!("apns {status}: {}", text.chars().take(300).collect::<String>())))
    }
}

// ---------------------------------------------------------------- senders

/// Configured native senders (announced to cores in `Welcome.nativePush`).
#[derive(Debug, Clone, Default)]
pub struct NativeSenders {
    /// FCM sender.
    pub fcm: Option<Arc<FcmSender>>,
    /// APNs sender.
    pub apns: Option<Arc<ApnsSender>>,
}

impl NativeSenders {
    /// Transports with a sender (`fcm`, `apns`).
    #[must_use]
    pub fn transports(&self) -> Vec<String> {
        let mut t = Vec::new();
        if self.fcm.is_some() {
            t.push("fcm".to_owned());
        }
        if self.apns.is_some() {
            t.push("apns".to_owned());
        }
        t
    }

    /// Delivers through the sender for `transport`.
    ///
    /// # Errors
    /// [`NativePushError::NotConfigured`] or the sender's error.
    pub async fn deliver(&self, transport: PushTransport, msg: &NativeDelivery) -> Result<(), NativePushError> {
        match transport {
            PushTransport::Fcm => self.fcm.as_ref().ok_or(NativePushError::NotConfigured("fcm"))?.send(msg).await,
            PushTransport::Apns => self.apns.as_ref().ok_or(NativePushError::NotConfigured("apns"))?.send(msg).await,
            PushTransport::WebPush => Err(NativePushError::NotConfigured("webPush")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Bytes;
    use axum::extract::Path as AxPath;
    use axum::http::{HeaderMap, StatusCode, Version};
    use axum::routing::post;
    use parking_lot::Mutex;
    use rand_core06::OsRng;

    use super::*;

    type Seen = Arc<Mutex<Vec<(String, HeaderMap, Vec<u8>, Version)>>>;

    async fn serve(app: axum::Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn part(jwt: &str, i: usize) -> serde_json::Value {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(jwt.split('.').nth(i).unwrap()).unwrap()).unwrap()
    }

    fn delivery(token: &str) -> NativeDelivery {
        let env = br#"{"k":"approval","p":{"n":"AAAA","c":"BBBB"}}"#;
        NativeDelivery::from_envelope(token.into(), env, Some("abc_DEF".into()), "high".into()).unwrap()
    }

    #[test]
    fn envelope_validation() {
        let d = delivery("t");
        assert_eq!(d.category, "approval");
        let p: serde_json::Value = serde_json::from_str(&d.payload).unwrap();
        assert_eq!(p, serde_json::json!({"n": "AAAA", "c": "BBBB"}));
        for bad in [&b"nope"[..], br#"{"k":"a"}"#, br#"{"k":1,"p":{"n":"a","c":"b"}}"#, br#"{"k":"a","p":{"n":"a"}}"#] {
            assert!(NativeDelivery::from_envelope("t".into(), bad, None, "high".into()).is_err());
        }
    }

    #[tokio::test]
    async fn fcm_jwt_token_exchange_and_message_shape() {
        use rsa::pkcs8::{EncodePrivateKey as _, LineEnding};
        use rsa::signature::Verifier as _;

        let key = rsa::RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let seen: Seen = Arc::default();
        let token_hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let (s1, t1) = (Arc::clone(&seen), Arc::clone(&token_hits));
        let app = axum::Router::new()
            .route(
                "/token",
                post(move |b: String| {
                    let t = Arc::clone(&t1);
                    async move {
                        t.lock().push(b);
                        axum::Json(
                            serde_json::json!({"access_token": "at-1", "expires_in": 3600, "token_type": "Bearer"}),
                        )
                    }
                }),
            )
            .route(
                "/v1/projects/{pid}/messages:send",
                post(move |AxPath(pid): AxPath<String>, h: HeaderMap, b: Bytes| {
                    let s = Arc::clone(&s1);
                    async move {
                        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
                        s.lock().push((pid, h, b.to_vec(), Version::HTTP_11));
                        match v["message"]["token"].as_str() {
                            Some("dead") => (
                                StatusCode::NOT_FOUND,
                                r#"{"error":{"status":"NOT_FOUND","details":[{"errorCode":"UNREGISTERED"}]}}"#,
                            ),
                            Some("flaky") => (StatusCode::SERVICE_UNAVAILABLE, "{}"),
                            _ => (StatusCode::OK, r#"{"name":"projects/demo/messages/1"}"#),
                        }
                    }
                }),
            );
        let base = serve(app).await;
        let sa = serde_json::json!({
            "type": "service_account", "project_id": "demo", "private_key": pem,
            "client_email": "push@demo.iam.gserviceaccount.com", "token_uri": format!("{base}/token"),
        });
        let fcm = FcmSender::from_json(&sa.to_string(), &format!("{base}/")).unwrap();

        fcm.send(&delivery("tok-1")).await.unwrap();
        fcm.send(&delivery("tok-2")).await.unwrap();
        assert_eq!(token_hits.lock().len(), 1, "access token is cached");

        // Token request: JWT bearer grant with an RS256 JWT that verifies.
        let form = token_hits.lock()[0].clone();
        assert!(form.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="));
        let jwt = form.split("assertion=").nth(1).unwrap();
        assert_eq!(part(jwt, 0), serde_json::json!({"alg": "RS256", "typ": "JWT"}));
        let claims = part(jwt, 1);
        assert_eq!(claims["iss"], "push@demo.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], FCM_SCOPE);
        assert_eq!(claims["aud"], format!("{base}/token"));
        assert_eq!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(), 3600);
        let (input, sig) = jwt.rsplit_once('.').unwrap();
        let vk = rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key.to_public_key());
        let sig = rsa::pkcs1v15::Signature::try_from(URL_SAFE_NO_PAD.decode(sig).unwrap().as_slice()).unwrap();
        vk.verify(input.as_bytes(), &sig).unwrap();

        // Send request shape.
        let (pid, h, body, _) = seen.lock()[0].clone();
        assert_eq!(pid, "demo");
        assert_eq!(h["authorization"], "Bearer at-1");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let m = &v["message"];
        assert_eq!(m["token"], "tok-1");
        assert_eq!(m["android"], serde_json::json!({"priority": "HIGH", "collapse_key": "abc_DEF"}));
        assert_eq!(m["notification"], serde_json::json!({"title": "Orkestr", "body": "Action needed"}));
        assert_eq!(m["data"]["k"], "approval");
        assert_eq!(m["data"]["p"], r#"{"c":"BBBB","n":"AAAA"}"#);

        let mut low = delivery("tok-3");
        low.urgency = "normal".into();
        assert_eq!(FcmSender::message(&low)["message"]["android"]["priority"], "NORMAL");

        assert!(matches!(fcm.send(&delivery("dead")).await, Err(NativePushError::Gone(404))));
        assert!(matches!(fcm.send(&delivery("flaky")).await, Err(NativePushError::Retryable(_))));
        assert!(FcmSender::from_json(r#"{"project_id":"x","private_key":"nope","client_email":"a"}"#, "x").is_err());
    }

    #[tokio::test]
    async fn apns_es256_jwt_over_h2c() {
        use p256::ecdsa::signature::Verifier as _;
        use p256::pkcs8::{EncodePrivateKey as _, LineEnding};

        let key = p256::ecdsa::SigningKey::random(&mut OsRng);
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let seen: Seen = Arc::default();
        let s1 = Arc::clone(&seen);
        let app = axum::Router::new().route(
            "/3/device/{token}",
            post(move |AxPath(token): AxPath<String>, req: axum::extract::Request| {
                let s = Arc::clone(&s1);
                async move {
                    let version = req.version();
                    let h = req.headers().clone();
                    let b = axum::body::to_bytes(req.into_body(), 1 << 16).await.unwrap();
                    s.lock().push((token.clone(), h, b.to_vec(), version));
                    match token.as_str() {
                        "gone" => (StatusCode::GONE, r#"{"reason":"Unregistered"}"#),
                        "busy" => (StatusCode::TOO_MANY_REQUESTS, r#"{"reason":"TooManyRequests"}"#),
                        _ => (StatusCode::OK, ""),
                    }
                }
            }),
        );
        let base = serve(app).await;
        let apns = ApnsSender::from_pem(&pem, "KEY123", "TEAM456", "dev.orkestr.mobile", &base).unwrap();
        apns.send(&delivery("abcdef0123")).await.unwrap();
        apns.send(&delivery("abcdef0124")).await.unwrap();

        let (token, h, body, version) = seen.lock()[0].clone();
        assert_eq!(version, Version::HTTP_2, "APNs is HTTP/2 (h2c prior knowledge on http://)");
        assert_eq!(token, "abcdef0123");
        assert_eq!(h["apns-push-type"], "alert");
        assert_eq!(h["apns-priority"], "10");
        assert_eq!(h["apns-topic"], "dev.orkestr.mobile");
        assert_eq!(h["apns-collapse-id"], "abc_DEF");
        let auth = h["authorization"].to_str().unwrap();
        let jwt = auth.strip_prefix("bearer ").unwrap();
        assert_eq!(seen.lock()[1].1["authorization"], auth, "provider token is cached");
        assert_eq!(part(jwt, 0), serde_json::json!({"alg": "ES256", "kid": "KEY123"}));
        assert_eq!(part(jwt, 1)["iss"], "TEAM456");
        let (input, sig) = jwt.rsplit_once('.').unwrap();
        let sig = p256::ecdsa::Signature::from_slice(&URL_SAFE_NO_PAD.decode(sig).unwrap()).unwrap();
        key.verifying_key().verify(input.as_bytes(), &sig).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["aps"]["mutable-content"], 1);
        assert_eq!(v["aps"]["alert"], serde_json::json!({"title": "Orkestr", "body": "Action needed"}));
        assert_eq!(v["k"], "approval");
        assert_eq!(v["p"], r#"{"c":"BBBB","n":"AAAA"}"#);

        assert!(matches!(apns.send(&delivery("gone")).await, Err(NativePushError::Gone(410))));
        assert!(matches!(apns.send(&delivery("busy")).await, Err(NativePushError::Retryable(_))));
        assert!(matches!(apns.send(&delivery("../x")).await, Err(NativePushError::Gone(400))));

        let senders = NativeSenders { fcm: None, apns: Some(Arc::new(apns)) };
        assert_eq!(senders.transports(), vec!["apns".to_owned()]);
        assert!(matches!(
            senders.deliver(PushTransport::Fcm, &delivery("x")).await,
            Err(NativePushError::NotConfigured("fcm"))
        ));
    }
}
