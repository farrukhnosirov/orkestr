//! Web Push in the core (remote-access.md §11, ADR-0036).
//!
//! * [`trigger_for`] maps contract events to notification triggers;
//! * [`encrypt_aes128gcm`] implements RFC 8291 (Message Encryption for Web
//!   Push, `aes128gcm` content coding of RFC 8188) — the gateway only ever
//!   sees ciphertext;
//! * [`PushService`] persists subscriptions (`<data>/push/subscriptions.json`)
//!   and runs the event → push loop, handing encrypted bodies to
//!   [`RemoteService::send_push`](super::RemoteService::send_push).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use orkestr_domain::{DeviceId, RiskLevel, TaskStatus};
use orkestr_protocol::event::{Event, EventKind};
use orkestr_protocol::methods::push::{PushLockScreen, PushPayload, PushTransport};
use orkestr_protocol::RpcError;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use parking_lot::Mutex;
use rand_core06::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{PushRequest, RemoteService};

/// Record size advertised in the `aes128gcm` header (single record).
const RECORD_SIZE: u32 = 4096;
/// Max plaintext so that one record fits: rs − 16 (tag) − 1 (delimiter).
pub const MAX_PLAINTEXT: usize = RECORD_SIZE as usize - 17;

/// Notification categories (also the `push.subscribe` filter values).
pub mod category {
    /// Approval needed.
    pub const APPROVAL: &str = "approval";
    /// Task completed.
    pub const TASK_COMPLETED: &str = "taskCompleted";
    /// Task blocked / waiting for the user.
    pub const TASK_BLOCKED: &str = "taskBlocked";
    /// Build failed.
    pub const BUILD_FAILED: &str = "buildFailed";
    /// Test run with failures.
    pub const TEST_FAILED: &str = "testFailed";
    /// Budget threshold crossed.
    pub const BUDGET: &str = "budget";
    /// `push.test`.
    pub const TEST: &str = "test";
}

/// A notification to send for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTrigger {
    /// Category (see [`category`]).
    pub category: &'static str,
    /// Lock-screen title (generic, no project secrets).
    pub title: String,
    /// Lock-screen body.
    pub body: String,
    /// `Urgency` header.
    pub urgency: &'static str,
    /// Collapse key (dedup) before hashing into a `Topic`.
    pub collapse_key: Option<String>,
}

/// Maps an event to a push trigger (`None` = no notification).
#[must_use]
pub fn trigger_for(event: &Event) -> Option<PushTrigger> {
    let t = |category, title: &str, body: String, urgency, key: Option<String>| PushTrigger {
        category,
        title: title.to_owned(),
        body,
        urgency,
        collapse_key: key,
    };
    match &event.kind {
        EventKind::ApprovalRequested(p) => {
            let a = &p.approval;
            let risk = match a.risk {
                RiskLevel::Low => "Low",
                RiskLevel::Medium => "Medium",
                RiskLevel::High => "High",
                RiskLevel::Critical => "Critical",
            };
            Some(t(
                category::APPROVAL,
                "Approval needed",
                format!("{risk}-risk {:?} action is waiting for you", a.capability),
                "high",
                Some(format!("approval:{}", a.id)),
            ))
        }
        EventKind::TaskStatusChanged(p) => {
            let key = Some(format!("task:{}", p.task_id));
            let reason = p.reason.clone().unwrap_or_default();
            match p.to {
                TaskStatus::Completed => Some(t(category::TASK_COMPLETED, "Task completed", reason, "normal", key)),
                TaskStatus::Blocked | TaskStatus::Waiting => {
                    Some(t(category::TASK_BLOCKED, "Task needs attention", reason, "normal", key))
                }
                _ => None,
            }
        }
        EventKind::BuildFailed(p) => Some(t(
            category::BUILD_FAILED,
            "Build failed",
            p.exit_code.map(|c| format!("exit code {c}")).unwrap_or_default(),
            "normal",
            Some(format!("build:{}", p.build_id)),
        )),
        EventKind::TestRunFinished(p) if p.failed > 0 => Some(t(
            category::TEST_FAILED,
            "Tests failed",
            format!("{} failed, {} passed", p.failed, p.passed),
            "normal",
            Some(format!("tests:{}", p.run_id)),
        )),
        EventKind::BudgetThreshold(p) => Some(t(
            category::BUDGET,
            "Budget threshold reached",
            format!("{}: {:.0}% of ${:.2}", p.scope, p.fraction * 100.0, p.limit_usd),
            "normal",
            Some(format!("budget:{}", p.scope)),
        )),
        _ => None,
    }
}

/// Builds the plaintext payload (remote-access.md §11).
#[must_use]
pub fn payload_for(event: &Event, trigger: &PushTrigger) -> PushPayload {
    PushPayload {
        v: 1,
        event_id: event.id.to_string(),
        category: trigger.category.to_owned(),
        project_id: event.project_id.map(|p| p.to_string()),
        ts: event.ts.to_rfc3339(),
        title: trigger.title.clone(),
        body: trigger.body.clone(),
    }
}

/// RFC 8030 `Topic` (≤ 32 base64url chars) derived from a collapse key.
#[must_use]
pub fn topic_for(collapse_key: &str) -> String {
    URL_SAFE_NO_PAD.encode(&Sha256::digest(collapse_key.as_bytes())[..24])
}

/// RFC 8291 errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PushCryptoError {
    /// `p256dh` is not a valid uncompressed P-256 point.
    #[error("invalid p256dh key")]
    BadKey,
    /// `auth` is not 16 bytes.
    #[error("invalid auth secret")]
    BadAuth,
    /// Payload does not fit one record.
    #[error("payload too large")]
    TooLarge,
    /// AEAD failure.
    #[error("encryption failed")]
    Aead,
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    // Output lengths here (12/16/32) are always valid for HKDF-SHA256.
    let _ = Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, out);
}

/// RFC 8291 `aes128gcm` encryption with explicit sender key and salt
/// (deterministic; used by the RFC test vector). Returns the full body
/// `salt ‖ rs ‖ idlen ‖ keyid(as_public) ‖ ciphertext`.
///
/// # Errors
/// Bad UA key/secret or oversize payload.
pub fn encrypt_with(
    plaintext: &[u8],
    ua_public: &[u8],
    auth_secret: &[u8],
    as_secret: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, PushCryptoError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(PushCryptoError::TooLarge);
    }
    if auth_secret.len() != 16 {
        return Err(PushCryptoError::BadAuth);
    }
    let ua = PublicKey::from_sec1_bytes(ua_public).map_err(|_| PushCryptoError::BadKey)?;
    let ua_raw = ua.to_encoded_point(false);
    let as_raw = as_secret.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua.as_affine());

    let mut key_info = Vec::with_capacity(14 + 130);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_raw.as_bytes());
    key_info.extend_from_slice(as_raw.as_bytes());
    let mut ikm = [0u8; 32];
    hkdf(auth_secret, shared.raw_secret_bytes(), &key_info, &mut ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek);
    hkdf(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce);

    let mut padded = Vec::with_capacity(plaintext.len() + 1);
    padded.extend_from_slice(plaintext);
    padded.push(2); // last-record delimiter, no padding
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| PushCryptoError::Aead)?;
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), padded.as_slice()).map_err(|_| PushCryptoError::Aead)?;

    let mut out = Vec::with_capacity(16 + 4 + 1 + 65 + ct.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(65);
    out.extend_from_slice(as_raw.as_bytes());
    out.extend_from_slice(&ct);
    Ok(out)
}

/// AAD of native (FCM/APNs) payloads (ADR-0037).
pub const NATIVE_AAD: &[u8] = b"orkestr-push-v1";

/// AES-256-GCM encryption of a native payload with an explicit nonce
/// (deterministic; used by the vector). Returns `{"n": b64u(nonce), "c": b64u(ct‖tag)}`.
///
/// # Errors
/// Key is not 32 bytes or oversize payload.
pub fn encrypt_native_with(
    key: &[u8],
    nonce: &[u8; 12],
    plaintext: &[u8],
) -> Result<serde_json::Value, PushCryptoError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(PushCryptoError::TooLarge);
    }
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(|_| PushCryptoError::BadKey)?;
    let ct = cipher
        .encrypt(Nonce::from_slice(nonce), aes_gcm::aead::Payload { msg: plaintext, aad: NATIVE_AAD })
        .map_err(|_| PushCryptoError::Aead)?;
    Ok(serde_json::json!({ "n": URL_SAFE_NO_PAD.encode(nonce), "c": URL_SAFE_NO_PAD.encode(ct) }))
}

/// AES-256-GCM encryption of a native payload with a random 12-byte nonce.
///
/// # Errors
/// See [`encrypt_native_with`].
pub fn encrypt_native(key: &[u8], plaintext: &[u8]) -> Result<serde_json::Value, PushCryptoError> {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    encrypt_native_with(key, &nonce, plaintext)
}

/// RFC 8291 encryption with a fresh sender key and salt.
///
/// # Errors
/// See [`encrypt_with`].
pub fn encrypt_aes128gcm(plaintext: &[u8], ua_public: &[u8], auth_secret: &[u8]) -> Result<Vec<u8>, PushCryptoError> {
    let as_secret = SecretKey::random(&mut OsRng);
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    encrypt_with(plaintext, ua_public, auth_secret, &as_secret, &salt)
}

// ---------------------------------------------------------------- subscriptions

/// A stored browser subscription or native (FCM/APNs) registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSubscription {
    /// Id.
    pub id: String,
    /// Owning device (`None` = subscribed from a local client).
    pub device_id: Option<DeviceId>,
    /// Push endpoint (FCM/APNs: the device token).
    pub endpoint: String,
    /// UA public key (base64url; empty for native).
    #[serde(default)]
    pub p256dh: String,
    /// UA auth secret (base64url; empty for native).
    #[serde(default)]
    pub auth: String,
    /// Category filter (`None` = all).
    pub categories: Option<Vec<String>>,
    /// Transport (`webPush` for Phase 4 records).
    #[serde(default)]
    pub transport: PushTransport,
    /// Native payload key (AES-256, base64url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Native lock-screen privacy (Phase 5 always shows the generic alert).
    #[serde(default)]
    pub lock_screen: PushLockScreen,
}

/// Wire name of a native transport (`ToCore::Welcome.nativePush`).
#[must_use]
pub const fn transport_name(t: PushTransport) -> &'static str {
    match t {
        PushTransport::WebPush => "webPush",
        PushTransport::Fcm => "fcm",
        PushTransport::Apns => "apns",
    }
}

impl StoredSubscription {
    fn wants(&self, category: &str) -> bool {
        category == category::TEST || self.categories.as_ref().is_none_or(|c| c.iter().any(|x| x == category))
    }
}

/// Subscription store + event → push loop.
#[derive(Debug)]
pub struct PushService {
    path: PathBuf,
    subs: Mutex<Vec<StoredSubscription>>,
    vapid_public_key: Mutex<Option<String>>,
}

impl PushService {
    /// Opens (or creates empty) the store under `data_dir/push`.
    #[must_use]
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("push").join("subscriptions.json");
        let subs = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let vapid = std::fs::read_to_string(data_dir.join("push").join("vapid_public_key"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        Self { path, subs: Mutex::new(subs), vapid_public_key: Mutex::new(vapid) }
    }

    fn persist(&self, subs: &[StoredSubscription]) -> Result<(), RpcError> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| RpcError::internal(format!("push store: {e}")))?;
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(subs).map_err(|e| RpcError::internal(e.to_string()))?;
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| RpcError::internal(format!("push store: {e}")))
    }

    /// The gateway's VAPID public key (set from the tunnel `welcome`, or the
    /// `push/vapid_public_key` file).
    #[must_use]
    pub fn vapid_public_key(&self) -> Option<String> {
        self.vapid_public_key.lock().clone()
    }

    /// Records the gateway's VAPID public key (called by the tunnel on welcome).
    pub fn set_vapid_public_key(&self, key: Option<String>) {
        *self.vapid_public_key.lock() = key;
    }

    /// Adds or replaces (same endpoint) a subscription.
    ///
    /// # Errors
    /// Invalid keys or storage failure.
    pub fn subscribe(&self, mut sub: StoredSubscription) -> Result<String, RpcError> {
        let p = URL_SAFE_NO_PAD.decode(&sub.p256dh).map_err(|_| RpcError::bad_request("p256dh is not base64url"))?;
        PublicKey::from_sec1_bytes(&p).map_err(|_| RpcError::bad_request("p256dh is not a P-256 point"))?;
        let a = URL_SAFE_NO_PAD.decode(&sub.auth).map_err(|_| RpcError::bad_request("auth is not base64url"))?;
        if a.len() != 16 {
            return Err(RpcError::bad_request("auth must be 16 bytes"));
        }
        if !(sub.endpoint.starts_with("https://")
            || sub.endpoint.starts_with("http://127.0.0.1")
            || sub.endpoint.starts_with("http://localhost"))
        {
            return Err(RpcError::bad_request("endpoint must be https"));
        }
        let mut subs = self.subs.lock();
        if let Some(old) = subs.iter().find(|s| s.endpoint == sub.endpoint) {
            sub.id.clone_from(&old.id);
        }
        subs.retain(|s| s.endpoint != sub.endpoint);
        let id = sub.id.clone();
        subs.push(sub);
        self.persist(&subs)?;
        Ok(id)
    }

    /// Adds or replaces a native (FCM/APNs) registration. The same token, or
    /// an older registration of the same device and transport, is replaced
    /// (keeping its id).
    ///
    /// # Errors
    /// Web Push transport, empty/oversize token, key not 32 bytes, storage failure.
    pub fn register_native(&self, mut sub: StoredSubscription) -> Result<String, RpcError> {
        if sub.transport == PushTransport::WebPush {
            return Err(RpcError::bad_request("transport must be fcm or apns (use push.subscribe for Web Push)"));
        }
        if sub.endpoint.is_empty() || sub.endpoint.len() > 4096 || sub.endpoint.chars().any(char::is_whitespace) {
            return Err(RpcError::bad_request("invalid device token"));
        }
        let key = sub.key.as_deref().map(|k| URL_SAFE_NO_PAD.decode(k.trim_end_matches('=')));
        if !matches!(key, Some(Ok(ref k)) if k.len() == 32) {
            return Err(RpcError::bad_request("key must be 32 bytes, base64url"));
        }
        sub.p256dh.clear();
        sub.auth.clear();
        let (t, ep, dev) = (sub.transport, sub.endpoint.clone(), sub.device_id);
        let same = move |s: &StoredSubscription| {
            s.transport == t && (s.endpoint == ep || (dev.is_some() && s.device_id == dev))
        };
        let mut subs = self.subs.lock();
        if let Some(old) = subs.iter().find(|s| same(s)) {
            sub.id.clone_from(&old.id);
        }
        subs.retain(|s| !same(s));
        let id = sub.id.clone();
        subs.push(sub);
        self.persist(&subs)?;
        Ok(id)
    }

    /// Removes the subscription for `endpoint` (only its owner, or a local caller).
    ///
    /// # Errors
    /// Owned by another device, or storage failure.
    pub fn unsubscribe(&self, endpoint: &str, caller: Option<DeviceId>) -> Result<(), RpcError> {
        let mut subs = self.subs.lock();
        if let (Some(c), Some(s)) = (caller, subs.iter().find(|s| s.endpoint == endpoint)) {
            if s.device_id != Some(c) {
                return Err(RpcError::forbidden("subscription belongs to another device"));
            }
        }
        subs.retain(|s| s.endpoint != endpoint);
        self.persist(&subs)
    }

    /// Drops every subscription of a revoked device.
    ///
    /// # Errors
    /// Storage failure.
    pub fn remove_device(&self, device: DeviceId) -> Result<(), RpcError> {
        let mut subs = self.subs.lock();
        subs.retain(|s| s.device_id != Some(device));
        self.persist(&subs)
    }

    /// Current subscriptions.
    #[must_use]
    pub fn list(&self) -> Vec<StoredSubscription> {
        self.subs.lock().clone()
    }

    /// Encrypts `payload` for every matching subscription (optionally only
    /// those of `only_device`) and hands them to the remote service.
    /// Returns how many messages the gateway accepted.
    ///
    /// # Errors
    /// The first `send_push` error when nothing could be sent.
    pub async fn dispatch(
        &self,
        remote: &RemoteService,
        payload: &PushPayload,
        urgency: &str,
        collapse_key: Option<&str>,
        only_device: Option<DeviceId>,
    ) -> Result<u32, RpcError> {
        let body = serde_json::to_vec(payload).map_err(|e| RpcError::internal(e.to_string()))?;
        let targets: Vec<_> = self
            .list()
            .into_iter()
            .filter(|s| s.wants(&payload.category) && only_device.is_none_or(|d| s.device_id == Some(d)))
            .collect();
        let mut sent = 0;
        let mut first_err = None;
        for s in targets {
            let enc = match encrypt_for(&s, &body, &payload.category) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "push encryption failed");
                    continue;
                }
            };
            let req = PushRequest {
                device_ids: s.device_id.into_iter().collect(),
                endpoint: s.endpoint.clone(),
                body: enc,
                collapse_key: collapse_key.map(topic_for),
                urgency: urgency.to_owned(),
                transport: s.transport,
            };
            match remote.send_push(req).await {
                Ok(()) => sent += 1,
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) if sent == 0 => Err(e),
            _ => Ok(sent),
        }
    }
}

/// Encrypts `body` for one subscription: RFC 8291 for Web Push; for FCM/APNs
/// the envelope `{"k": category, "p": {"n", "c"}}` the gateway maps onto the
/// provider message (`data.k`, `data.p`).
fn encrypt_for(s: &StoredSubscription, body: &[u8], category: &str) -> Result<Vec<u8>, PushCryptoError> {
    match s.transport {
        PushTransport::WebPush => {
            let ua = URL_SAFE_NO_PAD.decode(&s.p256dh).map_err(|_| PushCryptoError::BadKey)?;
            let auth = URL_SAFE_NO_PAD.decode(&s.auth).map_err(|_| PushCryptoError::BadAuth)?;
            encrypt_aes128gcm(body, &ua, &auth)
        }
        PushTransport::Fcm | PushTransport::Apns => {
            let key = s
                .key
                .as_deref()
                .and_then(|k| URL_SAFE_NO_PAD.decode(k.trim_end_matches('=')).ok())
                .ok_or(PushCryptoError::BadKey)?;
            let p = encrypt_native(&key, body)?;
            Ok(serde_json::json!({ "k": category, "p": p }).to_string().into_bytes())
        }
    }
}

/// Whether non-high-priority pushes are muted now: the global
/// `doNotDisturb` setting or the notification quiet hours (local time).
async fn muted_now(services: &crate::services::Services) -> bool {
    if services.settings.get().do_not_disturb {
        return true;
    }
    let raw: Option<String> = services
        .db
        .call(|c| {
            use orkestr_storage::rusqlite::OptionalExtension as _;
            Ok(c.query_row(
                "SELECT value FROM core_settings WHERE key = ?1",
                orkestr_storage::rusqlite::params![crate::rpc::voice::VOICE_KEY],
                |r| r.get(0),
            )
            .optional()?)
        })
        .await
        .ok()
        .flatten();
    let q = crate::rpc::voice::parse_stored(raw.as_deref()).notifications.quiet_hours;
    q.enabled && in_quiet_hours(&q.start, &q.end, chrono::Local::now().time())
}

/// `HH:MM` window check (wraps past midnight; `start == end` = never).
#[must_use]
pub fn in_quiet_hours(start: &str, end: &str, now: chrono::NaiveTime) -> bool {
    let parse = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok();
    let (Some(s), Some(e)) = (parse(start), parse(end)) else { return false };
    if s <= e {
        s <= now && now < e
    } else {
        now >= s || now < e
    }
}

static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<PushService>>>> = OnceLock::new();

/// The push service for these services (one per data dir).
#[must_use]
pub fn service(services: &Arc<crate::services::Services>) -> Arc<PushService> {
    let dir = services.paths.data_dir.clone();
    let mut map = REGISTRY.get_or_init(Default::default).lock();
    Arc::clone(map.entry(dir.clone()).or_insert_with(|| Arc::new(PushService::open(&dir))))
}

/// Starts the event → push loop (core startup). It stops on `shutdown`, so a
/// restarted core in the same process gets a fresh loop and the old
/// `Services` are released.
pub fn spawn_loop(services: &Arc<crate::services::Services>, shutdown: tokio_util::sync::CancellationToken) {
    let push = service(services);
    tokio::spawn(run_loop(Arc::clone(services), push, shutdown));
}

/// How often the push loop tails the event log.
const TAIL_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// Events read per page while tailing.
const TAIL_PAGE: u32 = 500;

/// Tails the persisted event log (like the architecture auto-refresh) instead
/// of holding a live bus subscription: a permanent subscriber would make the
/// core believe a client is listening (the node heartbeat then never idles).
/// Every trigger and `DeviceRevoked` event is persisted.
async fn run_loop(
    services: Arc<crate::services::Services>,
    push: Arc<PushService>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let bus = services.events.clone();
    let mut cursor = bus.last_seq();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(TAIL_POLL) => {}
        }
        let last = bus.last_seq();
        if last <= cursor {
            continue;
        }
        if push.list().is_empty() {
            // Nothing to deliver and nothing to drop.
            cursor = last;
            continue;
        }
        let mut from = cursor + 1;
        loop {
            let page = match bus
                .replay(&orkestr_events::ReplayQuery {
                    from_seq: from,
                    to_seq: Some(last),
                    limit: TAIL_PAGE,
                    topics: None,
                    newest_first: false,
                })
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "push: cannot read the event log");
                    break;
                }
            };
            for ev in &page.events {
                from = ev.seq + 1;
                handle_event(&services, &push, ev).await;
            }
            if !page.has_more || page.events.is_empty() {
                break;
            }
        }
        // Ephemeral events are never stored: everything up to `last` is done.
        cursor = last;
    }
}

async fn handle_event(services: &Arc<crate::services::Services>, push: &PushService, ev: &Event) {
    if let EventKind::DeviceRevoked(p) = &ev.kind {
        if let Err(e) = push.remove_device(p.device_id) {
            tracing::warn!(error = %e.message, "failed to drop push subscriptions of a revoked device");
        }
        return;
    }
    let Some(trigger) = trigger_for(ev) else { return };
    // DND / quiet hours: only high-priority pushes (approvals) still go.
    if trigger.urgency != "high" && muted_now(services).await {
        tracing::debug!(category = trigger.category, "push muted (do not disturb / quiet hours)");
        return;
    }
    let payload = payload_for(ev, &trigger);
    if let Err(e) =
        push.dispatch(&services.remote, &payload, trigger.urgency, trigger.collapse_key.as_deref(), None).await
    {
        tracing::debug!(error = %e.message, category = trigger.category, "push not delivered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(s).unwrap()
    }

    /// RFC 8291 §5 example.
    #[test]
    fn rfc8291_vector() {
        let as_secret = SecretKey::from_slice(&d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(as_secret.public_key().to_encoded_point(false).as_bytes()),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let ua_pub = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body =
            encrypt_with(b"When I grow up, I want to be a watermelon", &ua_pub, &auth, &as_secret, &salt).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn rejects_bad_inputs() {
        let ua = SecretKey::random(&mut OsRng).public_key().to_encoded_point(false);
        assert_eq!(encrypt_aes128gcm(b"x", b"nope", &[0; 16]), Err(PushCryptoError::BadKey));
        assert_eq!(encrypt_aes128gcm(b"x", ua.as_bytes(), &[0; 15]), Err(PushCryptoError::BadAuth));
        assert_eq!(
            encrypt_aes128gcm(&vec![0; MAX_PLAINTEXT + 1], ua.as_bytes(), &[0; 16]),
            Err(PushCryptoError::TooLarge)
        );
        let body = encrypt_aes128gcm(b"hi", ua.as_bytes(), &[1; 16]).unwrap();
        assert_eq!(body.len(), 16 + 4 + 1 + 65 + 2 + 1 + 16);
        assert!(topic_for("approval:0192f5d4-0000-7000-8000-000000000001").len() <= 32);
    }

    fn ev(kind: EventKind) -> Event {
        Event {
            seq: 1,
            id: orkestr_domain::EventId::new(),
            ts: orkestr_domain::now(),
            project_id: None,
            task_id: None,
            agent_id: None,
            node_id: None,
            correlation_id: None,
            kind,
        }
    }

    #[test]
    fn trigger_mapping() {
        use orkestr_protocol::event::{BudgetThresholdPayload, TaskStatusChangedPayload, TestRunFinishedPayload};
        let task = |to| {
            ev(EventKind::TaskStatusChanged(TaskStatusChangedPayload {
                task_id: orkestr_domain::TaskId::new(),
                from: TaskStatus::Running,
                to,
                reason: None,
            }))
        };
        assert_eq!(trigger_for(&task(TaskStatus::Completed)).unwrap().category, category::TASK_COMPLETED);
        assert_eq!(trigger_for(&task(TaskStatus::Blocked)).unwrap().category, category::TASK_BLOCKED);
        assert_eq!(trigger_for(&task(TaskStatus::Waiting)).unwrap().category, category::TASK_BLOCKED);
        assert!(trigger_for(&task(TaskStatus::Running)).is_none());
        let tests = |failed| {
            ev(EventKind::TestRunFinished(TestRunFinishedPayload {
                run_id: "r".into(),
                passed: 3,
                failed,
                skipped: 0,
                duration_ms: 1,
                summary: String::new(),
            }))
        };
        assert!(trigger_for(&tests(0)).is_none());
        assert_eq!(trigger_for(&tests(2)).unwrap().category, category::TEST_FAILED);
        let budget = ev(EventKind::BudgetThreshold(BudgetThresholdPayload {
            scope: "project".into(),
            spent_usd: 8.0,
            limit_usd: 10.0,
            fraction: 0.8,
        }));
        let b = trigger_for(&budget).unwrap();
        assert_eq!((b.category, b.body.as_str()), (category::BUDGET, "project: 80% of $10.00"));
        let p = payload_for(&budget, &b);
        assert_eq!(p.v, 1);
        assert_eq!(p.category, "budget");
    }

    #[test]
    fn subscription_store_round_trip_and_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let svc = PushService::open(dir.path());
        let ua = SecretKey::random(&mut OsRng).public_key().to_encoded_point(false);
        let dev = DeviceId::new();
        let sub = StoredSubscription {
            id: "s1".into(),
            device_id: Some(dev),
            endpoint: "https://push.example/abc".into(),
            p256dh: URL_SAFE_NO_PAD.encode(ua.as_bytes()),
            auth: URL_SAFE_NO_PAD.encode([7u8; 16]),
            categories: Some(vec!["approval".into()]),
            transport: PushTransport::WebPush,
            key: None,
            lock_screen: PushLockScreen::Generic,
        };
        assert_eq!(svc.subscribe(sub.clone()).unwrap(), "s1");
        let mut again = sub.clone();
        again.id = "s2".into();
        assert_eq!(svc.subscribe(again).unwrap(), "s1", "same endpoint keeps its id");
        assert!(sub.wants("approval") && sub.wants("test") && !sub.wants("budget"));
        let reopened = PushService::open(dir.path());
        assert_eq!(reopened.list(), vec![sub.clone()]);
        assert!(reopened.unsubscribe(&sub.endpoint, Some(DeviceId::new())).is_err());
        reopened.unsubscribe(&sub.endpoint, Some(dev)).unwrap();
        assert!(PushService::open(dir.path()).list().is_empty());
        let mut bad = sub;
        bad.endpoint = "http://evil.example/".into();
        assert!(svc.subscribe(bad).is_err());
    }

    const NATIVE_VECTOR: &str = include_str!("testdata/native-push-vector.json");
    const NATIVE_VECTOR_PATH: &str = "src/remote/testdata/native-push-vector.json";

    fn native_vector() -> serde_json::Value {
        let key: Vec<u8> = (0u8..32).collect();
        let nonce: [u8; 12] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab];
        let payload = PushPayload {
            v: 1,
            event_id: "0192f5d4-0000-7000-8000-0000000000e1".into(),
            category: category::APPROVAL.into(),
            project_id: Some("0192f5d4-0000-7000-8000-000000000001".into()),
            ts: "2026-10-22T12:00:00+00:00".into(),
            title: "Approval needed".into(),
            body: "High-risk Shell action is waiting for you".into(),
        };
        let plaintext = serde_json::to_string(&payload).unwrap();
        let body = encrypt_native_with(&key, &nonce, plaintext.as_bytes()).unwrap();
        serde_json::json!({
            "description": "Native push payload (ADR-0037): AES-256-GCM, 12-byte nonce, AAD \"orkestr-push-v1\", c = ciphertext || 16-byte tag; binary fields base64url without padding. FCM/APNs carry data.p = JSON text of `body` and data.k = category.",
            "key": URL_SAFE_NO_PAD.encode(&key),
            "nonce": URL_SAFE_NO_PAD.encode(nonce),
            "aad": std::str::from_utf8(NATIVE_AAD).unwrap(),
            "plaintext": plaintext,
            "payload": payload,
            "body": body,
        })
    }

    /// `ORKESTR_WRITE_VECTORS=1 cargo test -p orkestr-core native_push_vector` rewrites the file.
    #[test]
    fn native_push_vector() {
        let v = native_vector();
        if std::env::var_os("ORKESTR_WRITE_VECTORS").is_some() {
            std::fs::write(NATIVE_VECTOR_PATH, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        } else {
            let file: serde_json::Value = serde_json::from_str(NATIVE_VECTOR).unwrap();
            assert_eq!(file, v, "testdata/native-push-vector.json is stale");
        }
        // Decrypts back with the published values.
        let key = d(v["key"].as_str().unwrap());
        let ct = d(v["body"]["c"].as_str().unwrap());
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key).unwrap();
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&d(v["body"]["n"].as_str().unwrap())),
                aes_gcm::aead::Payload { msg: &ct, aad: NATIVE_AAD },
            )
            .unwrap();
        assert_eq!(std::str::from_utf8(&pt).unwrap(), v["plaintext"].as_str().unwrap());
        // Wrong AAD fails.
        assert!(cipher
            .decrypt(
                Nonce::from_slice(&d(v["body"]["n"].as_str().unwrap())),
                aes_gcm::aead::Payload { msg: &ct, aad: b"other" }
            )
            .is_err());
        // Random nonces differ.
        let a = encrypt_native(&key, b"x").unwrap();
        let b = encrypt_native(&key, b"x").unwrap();
        assert_ne!(a["n"], b["n"]);
        assert_eq!(encrypt_native(&[0; 16], b"x"), Err(PushCryptoError::BadKey));
    }

    fn native(dev: Option<DeviceId>, token: &str, transport: PushTransport) -> StoredSubscription {
        StoredSubscription {
            id: format!("n-{token}"),
            device_id: dev,
            endpoint: token.into(),
            p256dh: String::new(),
            auth: String::new(),
            categories: None,
            transport,
            key: Some(URL_SAFE_NO_PAD.encode([5u8; 32])),
            lock_screen: PushLockScreen::Generic,
        }
    }

    #[test]
    fn native_registration_and_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let svc = PushService::open(dir.path());
        let dev = DeviceId::new();
        assert_eq!(svc.register_native(native(Some(dev), "tok1", PushTransport::Fcm)).unwrap(), "n-tok1");
        // A new token of the same device + transport replaces the old one.
        assert_eq!(svc.register_native(native(Some(dev), "tok2", PushTransport::Fcm)).unwrap(), "n-tok1");
        svc.register_native(native(Some(dev), "apns1", PushTransport::Apns)).unwrap();
        let list = PushService::open(dir.path()).list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|s| s.endpoint == "tok2" && s.transport == PushTransport::Fcm));
        assert!(svc.register_native(native(Some(dev), "t", PushTransport::WebPush)).is_err());
        assert!(svc.register_native(native(Some(dev), "", PushTransport::Fcm)).is_err());
        let mut short = native(Some(dev), "t", PushTransport::Fcm);
        short.key = Some(URL_SAFE_NO_PAD.encode([1u8; 16]));
        assert!(svc.register_native(short).is_err());

        let env = encrypt_for(&list[0], b"{\"v\":1}", "approval").unwrap();
        let env: serde_json::Value = serde_json::from_slice(&env).unwrap();
        assert_eq!(env["k"], "approval");
        assert_eq!(d(env["p"]["n"].as_str().unwrap()).len(), 12);
        assert_eq!(d(env["p"]["c"].as_str().unwrap()).len(), 7 + 16);

        // Phase 4 records (no transport/key) still load as Web Push.
        let legacy: StoredSubscription = serde_json::from_value(serde_json::json!({
            "id": "x", "deviceId": null, "endpoint": "https://p/1", "p256dh": "a", "auth": "b", "categories": null
        }))
        .unwrap();
        assert_eq!(legacy.transport, PushTransport::WebPush);
        assert_eq!(transport_name(PushTransport::Apns), "apns");
    }

    #[test]
    fn quiet_hours_window() {
        let t = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        assert!(in_quiet_hours("22:00", "08:00", t("23:30")));
        assert!(in_quiet_hours("22:00", "08:00", t("07:59")));
        assert!(!in_quiet_hours("22:00", "08:00", t("08:00")));
        assert!(in_quiet_hours("12:00", "13:00", t("12:30")));
        assert!(!in_quiet_hours("12:00", "13:00", t("13:30")));
        assert!(!in_quiet_hours("bad", "13:00", t("12:30")));
    }
}
