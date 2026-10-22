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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use orkestr_domain::{DeviceId, RiskLevel, TaskStatus};
use orkestr_protocol::event::{Event, EventKind};
use orkestr_protocol::methods::push::PushPayload;
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

/// A stored browser subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSubscription {
    /// Id.
    pub id: String,
    /// Owning device (`None` = subscribed from a local client).
    pub device_id: Option<DeviceId>,
    /// Push endpoint.
    pub endpoint: String,
    /// UA public key (base64url).
    pub p256dh: String,
    /// UA auth secret (base64url).
    pub auth: String,
    /// Category filter (`None` = all).
    pub categories: Option<Vec<String>>,
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
    started: AtomicBool,
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
        Self { path, subs: Mutex::new(subs), vapid_public_key: Mutex::new(vapid), started: AtomicBool::new(false) }
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
            let (Ok(ua), Ok(auth)) = (URL_SAFE_NO_PAD.decode(&s.p256dh), URL_SAFE_NO_PAD.decode(&s.auth)) else {
                continue;
            };
            let enc = match encrypt_aes128gcm(&body, &ua, &auth) {
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

static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<PushService>>>> = OnceLock::new();

/// The push service for these services (one per data dir), starting the
/// event → push loop on first use. Integration: call once at core startup so
/// triggers fire before the first `push.*` call.
#[must_use]
pub fn service(services: &Arc<crate::services::Services>) -> Arc<PushService> {
    let dir = services.paths.data_dir.clone();
    let svc = {
        let mut map = REGISTRY.get_or_init(Default::default).lock();
        Arc::clone(map.entry(dir.clone()).or_insert_with(|| Arc::new(PushService::open(&dir))))
    };
    if !svc.started.swap(true, Ordering::SeqCst) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let services = Arc::clone(services);
            let push = Arc::clone(&svc);
            handle.spawn(run_loop(services, push));
        } else {
            svc.started.store(false, Ordering::SeqCst);
        }
    }
    svc
}

async fn run_loop(services: Arc<crate::services::Services>, push: Arc<PushService>) {
    let mut sub = services.events.subscribe_all();
    while let Some(rx) = sub.recv().await {
        let orkestr_events::Received::Event(ev) = rx else { continue };
        if let EventKind::DeviceRevoked(p) = &ev.kind {
            if let Err(e) = push.remove_device(p.device_id) {
                tracing::warn!(error = %e.message, "failed to drop push subscriptions of a revoked device");
            }
            continue;
        }
        let Some(trigger) = trigger_for(&ev) else { continue };
        if push.list().is_empty() {
            continue;
        }
        let payload = payload_for(&ev, &trigger);
        if let Err(e) =
            push.dispatch(&services.remote, &payload, trigger.urgency, trigger.collapse_key.as_deref(), None).await
        {
            tracing::debug!(error = %e.message, category = trigger.category, "push not delivered");
        }
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
}
