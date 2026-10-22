//! Web Push delivery (Phase 4, ADR-0036): VAPID (RFC 8292, ES256 JWT) and
//! the POST to the push service (RFC 8030 `TTL` / `Urgency` / `Topic`).
//! Payloads arrive already encrypted by the core (RFC 8291 `aes128gcm`);
//! the gateway never sees plaintext.
//!
//! Integration: call [`init`] once at startup (key persisted under
//! `<data-dir>/push/vapid_private_key`), mount [`vapid_handler`] at
//! `GET /v1/push/vapid`, and send [`vapid_public_key`] to cores so
//! `push.vapidKey` can answer. Until [`init`] runs, [`deliver`] returns
//! [`PushError::NotConfigured`].

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use rand_core06::OsRng;

/// An encrypted push message to deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDelivery {
    /// Push service endpoint.
    pub endpoint: String,
    /// RFC 8291 `aes128gcm` body.
    pub body: Vec<u8>,
    /// `Topic` header.
    pub topic: Option<String>,
    /// `Urgency` header.
    pub urgency: String,
    /// `TTL` header (seconds).
    pub ttl_secs: u32,
}

/// Push delivery errors.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// [`init`] was not called.
    #[error("web push is not configured on this gateway")]
    NotConfigured,
    /// Endpoint is not an absolute https URL (http allowed for loopback only).
    #[error("invalid push endpoint: {0}")]
    BadEndpoint(String),
    /// The subscription is gone (404/410): the core should drop it.
    #[error("push subscription expired ({0})")]
    Gone(u16),
    /// The push service refused the message.
    #[error("push service rejected the message ({0}): {1}")]
    Rejected(u16, String),
    /// Transport error.
    #[error("push request failed: {0}")]
    Http(String),
    /// Key storage error.
    #[error("vapid key: {0}")]
    Key(String),
}

const URGENCIES: [&str; 4] = ["very-low", "low", "normal", "high"];

/// VAPID signer + HTTP client.
#[derive(Debug, Clone)]
pub struct PushSender {
    key: SigningKey,
    subject: String,
    http: reqwest::Client,
}

impl PushSender {
    /// New sender from a key.
    #[must_use]
    pub fn new(key: SigningKey, subject: impl Into<String>) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap_or_default();
        Self { key, subject: subject.into(), http }
    }

    /// Loads `<data_dir>/push/vapid_private_key` or creates it.
    ///
    /// # Errors
    /// Unreadable / invalid key file.
    pub fn load_or_create(data_dir: &Path, subject: impl Into<String>) -> Result<Self, PushError> {
        let dir = data_dir.join("push");
        let path = dir.join("vapid_private_key");
        let key = match std::fs::read_to_string(&path) {
            Ok(s) => {
                let raw = URL_SAFE_NO_PAD.decode(s.trim()).map_err(|e| PushError::Key(e.to_string()))?;
                SigningKey::from_slice(&raw).map_err(|e| PushError::Key(e.to_string()))?
            }
            Err(_) => {
                let k = SigningKey::random(&mut OsRng);
                std::fs::create_dir_all(&dir).map_err(|e| PushError::Key(e.to_string()))?;
                std::fs::write(&path, URL_SAFE_NO_PAD.encode(k.to_bytes()))
                    .map_err(|e| PushError::Key(e.to_string()))?;
                k
            }
        };
        Ok(Self::new(key, subject))
    }

    /// Application server public key (SEC1 uncompressed, base64url).
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().to_encoded_point(false).as_bytes())
    }

    /// RFC 8292 JWT for `audience` (origin of the endpoint), valid 12 h from `now_secs`.
    #[must_use]
    pub fn jwt(&self, audience: &str, now_secs: u64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({ "aud": audience, "exp": now_secs + 12 * 3600, "sub": self.subject });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{header}.{claims}");
        let sig: Signature = self.key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }

    /// POSTs one message.
    ///
    /// # Errors
    /// See [`PushError`].
    pub async fn deliver(&self, msg: &PushDelivery) -> Result<(), PushError> {
        let url = reqwest::Url::parse(&msg.endpoint).map_err(|e| PushError::BadEndpoint(e.to_string()))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err(PushError::BadEndpoint("https required".into()));
        }
        let audience = url.origin().ascii_serialization();
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
        let urgency = if URGENCIES.contains(&msg.urgency.as_str()) { msg.urgency.as_str() } else { "normal" };
        let mut req = self
            .http
            .post(url)
            .header("TTL", msg.ttl_secs.to_string())
            .header("Urgency", urgency)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", format!("vapid t={}, k={}", self.jwt(&audience, now), self.public_key()))
            .body(msg.body.clone());
        if let Some(t) = msg
            .topic
            .as_deref()
            .filter(|t| t.len() <= 32 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        {
            req = req.header("Topic", t);
        }
        let res = req.send().await.map_err(|e| PushError::Http(e.to_string()))?;
        let status = res.status().as_u16();
        match status {
            200..=299 => Ok(()),
            404 | 410 => Err(PushError::Gone(status)),
            _ => Err(PushError::Rejected(status, res.text().await.unwrap_or_default())),
        }
    }
}

static SENDER: OnceLock<PushSender> = OnceLock::new();

/// Configures the process-wide sender (idempotent: first call wins).
///
/// # Errors
/// Key file problems.
pub fn init(data_dir: &Path, subject: &str) -> Result<(), PushError> {
    if SENDER.get().is_none() {
        let _ = SENDER.set(PushSender::load_or_create(data_dir, subject)?);
    }
    Ok(())
}

/// VAPID public key of the configured sender.
#[must_use]
pub fn vapid_public_key() -> Option<String> {
    SENDER.get().map(PushSender::public_key)
}

/// `GET /v1/push/vapid` -> `{publicKey}` (503 when not configured).
pub async fn vapid_handler() -> axum::response::Response {
    use axum::response::IntoResponse;
    match vapid_public_key() {
        Some(k) => axum::Json(serde_json::json!({ "publicKey": k })).into_response(),
        None => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "web push not configured").into_response(),
    }
}

/// Delivers `msg` to its push service with the process-wide VAPID key.
///
/// # Errors
/// [`PushError::NotConfigured`] before [`init`]; otherwise see [`PushSender::deliver`].
pub async fn deliver(msg: &PushDelivery) -> Result<(), PushError> {
    SENDER.get().ok_or(PushError::NotConfigured)?.deliver(msg).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Bytes;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;
    use parking_lot::Mutex;

    use super::*;

    type Seen = Arc<Mutex<Vec<(HeaderMap, Vec<u8>)>>>;

    async fn mock() -> (String, Seen) {
        let seen: Seen = Arc::default();
        let s1 = Arc::clone(&seen);
        let app = axum::Router::new()
            .route(
                "/push/ok",
                post(move |h: HeaderMap, b: Bytes| {
                    let s = Arc::clone(&s1);
                    async move {
                        s.lock().push((h, b.to_vec()));
                        StatusCode::CREATED
                    }
                }),
            )
            .route("/push/gone", post(|| async { StatusCode::GONE }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://127.0.0.1:{}", addr.port()), seen)
    }

    #[tokio::test]
    async fn posts_with_vapid_jwt_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let sender = PushSender::load_or_create(dir.path(), "mailto:ops@example.org").unwrap();
        let again = PushSender::load_or_create(dir.path(), "mailto:ops@example.org").unwrap();
        assert_eq!(sender.public_key(), again.public_key(), "key is persisted");

        let (base, seen) = mock().await;
        let msg = PushDelivery {
            endpoint: format!("{base}/push/ok"),
            body: vec![1, 2, 3],
            topic: Some("abc_DEF-1".into()),
            urgency: "high".into(),
            ttl_secs: 60,
        };
        sender.deliver(&msg).await.unwrap();
        let (h, body) = seen.lock().pop().unwrap();
        assert_eq!(body, vec![1, 2, 3]);
        assert_eq!(h["ttl"], "60");
        assert_eq!(h["urgency"], "high");
        assert_eq!(h["topic"], "abc_DEF-1");
        assert_eq!(h["content-encoding"], "aes128gcm");

        let auth = h["authorization"].to_str().unwrap();
        let rest = auth.strip_prefix("vapid t=").unwrap();
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, sender.public_key());
        let parts: Vec<_> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        let claims: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], base.as_str());
        assert_eq!(claims["sub"], "mailto:ops@example.org");
        let vk = VerifyingKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(k).unwrap()).unwrap();
        let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).unwrap();
    }

    #[tokio::test]
    async fn maps_errors() {
        let sender = PushSender::new(SigningKey::random(&mut OsRng), "mailto:a@b");
        let (base, _) = mock().await;
        let mut msg = PushDelivery {
            endpoint: format!("{base}/push/gone"),
            body: vec![],
            topic: Some("not a valid topic!".into()),
            urgency: "bogus".into(),
            ttl_secs: 0,
        };
        assert!(matches!(sender.deliver(&msg).await, Err(PushError::Gone(410))));
        msg.endpoint = "http://push.example.org/x".into();
        assert!(matches!(sender.deliver(&msg).await, Err(PushError::BadEndpoint(_))));
        assert!(matches!(deliver(&msg).await, Err(PushError::NotConfigured)));
    }
}
