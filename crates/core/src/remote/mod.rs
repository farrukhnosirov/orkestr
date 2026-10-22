//! Remote access (Phase 4, ADR-0030): gateway tunnel, E2E sessions, device
//! registry (authoritative), pairing, step-up, kill switch, audit, Web Push
//! hand-off to the gateway.
//!
//! * [`e2e`] — `e2e-v1` handshake and records (frozen wire spec + test vector).
//! * [`authz`] — `MethodPolicy` enforcement for `Principal::Remote`.
//! * [`tunnel`] — outbound tunnel dialer and per-stream E2E endpoints.
//! * [`pairing`] — `codeMac`, SAS, completion signature.
//! * [`store`] — `remote/state.json` + `remote/cik.key`.

pub mod authz;
pub mod e2e;
pub mod native_stepup;
pub mod pairing;
pub mod push;
pub mod store;
pub mod tunnel;

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use orkestr_domain::{Actor, AuditOutcome, DeviceId, Timestamp};
use orkestr_events::EventBus;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::remote::{
    tunnel::ToGateway, DeviceApproveParams, DeviceAuthorizedPayload, DeviceInfo, DeviceList, DevicePairingStart,
    DeviceRevokedPayload, PendingPairing, RemoteEnableParams, RemoteSessionPayload, RemoteStatus,
};
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_security::audit::{AuditLog, NewAudit};
use orkestr_security::webauthn::{self, AssertionJson, RegistrationJson};
use p256::ecdsa::SigningKey;
use parking_lot::Mutex;
use rand_core06::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::rpc::Principal;
use authz::RateLimiter;
pub use store::StepUpKind;
use store::{DeviceRecord, RemoteState, Store};

/// A Web Push message, already encrypted in the core (RFC 8291 `aes128gcm`),
/// handed to the gateway for VAPID signing + delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    /// Target devices (subscriptions owned by these devices).
    pub device_ids: Vec<DeviceId>,
    /// Push service endpoint (FCM/APNs: the device token).
    pub endpoint: String,
    /// Web Push: RFC 8291 `aes128gcm` body. FCM/APNs: the native envelope
    /// JSON `{"k": category, "p": {"n", "c"}}` (ADR-0037).
    pub body: Vec<u8>,
    /// Delivery transport.
    pub transport: orkestr_protocol::methods::push::PushTransport,
    /// `Topic` header (collapse key).
    pub collapse_key: Option<String>,
    /// `Urgency` header (`very-low` | `low` | `normal` | `high`).
    pub urgency: String,
}

/// Runs one RPC session over text channels (decrypted frames in, frames out)
/// with the given principal — the same connection logic as the local WS.
pub type SessionRunner = Arc<
    dyn Fn(Principal, mpsc::Receiver<String>, mpsc::Sender<String>) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Pairing offer lifetime.
const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// Wrong `codeMac` attempts before an offer is burned.
const PAIRING_MAX_ATTEMPTS: u32 = 5;
/// Step-up challenge lifetime.
const STEP_UP_TTL: Duration = Duration::from_secs(5 * 60);

struct Pairing {
    code: String,
    created: Instant,
    attempts: u32,
    claim: Option<Claim>,
}

#[derive(Clone)]
struct Claim {
    device_id: DeviceId,
    device_pub: Vec<u8>,
    name: String,
    platform: String,
    sas: String,
    at: Timestamp,
}

struct PendingStepUp {
    device: DeviceId,
    method: String,
    params_hash: [u8; 32],
    created: Instant,
}

pub(crate) struct SessionCtl {
    pub(crate) device_id: Option<DeviceId>,
    pub(crate) inbound: mpsc::Sender<Vec<u8>>,
    pub(crate) cancel: CancellationToken,
}

struct Attached {
    store: Store,
    cik: SigningKey,
    events: EventBus,
    audit: Arc<AuditLog>,
    runner: SessionRunner,
    shutdown: CancellationToken,
    me: Weak<RemoteService>,
    version: String,
}

pub(crate) struct TunnelCtl {
    pub(crate) cancel: CancellationToken,
    pub(crate) tx: Option<mpsc::Sender<tokio_tungstenite::tungstenite::Message>>,
}

/// Remote-access service (`Services.remote`).
pub struct RemoteService {
    att: OnceLock<Attached>,
    state: Mutex<RemoteState>,
    limiter: RateLimiter,
    connected: AtomicBool,
    last_error: Mutex<Option<String>>,
    tunnel: Mutex<Option<TunnelCtl>>,
    sessions: Mutex<HashMap<u32, SessionCtl>>,
    pairings: Mutex<HashMap<String, Pairing>>,
    step_ups: Mutex<HashMap<String, PendingStepUp>>,
    replay: Mutex<HashMap<(String, Vec<u8>), Instant>>,
}

impl std::fmt::Debug for RemoteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteService")
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Default for RemoteService {
    fn default() -> Self {
        Self::new()
    }
}

fn not_attached() -> RpcError {
    RpcError::conflict("remote access is not initialised")
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

impl RemoteService {
    /// Detached service (no persistence until [`Self::attach`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            att: OnceLock::new(),
            state: Mutex::new(RemoteState::default()),
            limiter: RateLimiter::default(),
            connected: AtomicBool::new(false),
            last_error: Mutex::new(None),
            tunnel: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            pairings: Mutex::new(HashMap::new()),
            step_ups: Mutex::new(HashMap::new()),
            replay: Mutex::new(HashMap::new()),
        }
    }

    /// Loads state + CIK from `<data_dir>/remote`, wires the session runner and
    /// re-dials the tunnel when remote access was enabled.
    ///
    /// # Errors
    /// The CIK cannot be created.
    pub fn attach(
        self: &Arc<Self>,
        data_dir: &Path,
        events: EventBus,
        audit: Arc<AuditLog>,
        runner: SessionRunner,
        shutdown: CancellationToken,
        version: String,
    ) -> std::io::Result<()> {
        let store = Store::new(data_dir.join("remote"));
        let cik = store.load_or_create_cik()?;
        *self.state.lock() = store.load();
        let enabled = self.state.lock().enabled;
        let _ =
            self.att.set(Attached { store, cik, events, audit, runner, shutdown, me: Arc::downgrade(self), version });
        if enabled {
            self.start_tunnel();
        }
        Ok(())
    }

    fn att(&self) -> Result<&Attached, RpcError> {
        self.att.get().ok_or_else(not_attached)
    }

    /// Per-device rate limiter.
    #[must_use]
    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    /// CIK public key (SEC1).
    #[must_use]
    pub fn cik_public(&self) -> Option<Vec<u8>> {
        self.att.get().map(|a| e2e::public_raw(&p256::PublicKey::from(a.cik.verifying_key())))
    }

    /// Gateway VAPID public key from the last tunnel `welcome`.
    #[must_use]
    pub fn vapid_public_key(&self) -> Option<String> {
        self.state.lock().vapid_public_key.clone()
    }

    /// `(gateway_url, core_token)` for gateway HTTP APIs (e.g. `/v1/sync`)
    /// once enrolled.
    #[must_use]
    pub fn sync_credentials(&self) -> Option<(String, String)> {
        let s = self.state.lock();
        Some((s.gateway_url.clone()?, s.core_token.clone()?))
    }

    fn persist(&self) {
        if let Some(a) = self.att.get() {
            let snapshot = self.state.lock().clone();
            if let Err(e) = a.store.save(&snapshot) {
                tracing::error!(error = %e, "failed to persist remote state");
            }
        }
    }

    async fn audit(
        &self,
        actor: Actor,
        action: &str,
        resource: String,
        outcome: AuditOutcome,
        details: serde_json::Value,
    ) {
        if let Some(a) = self.att.get() {
            let mut entry = NewAudit::new(actor, action, resource, outcome);
            entry.details = details;
            a.audit.record(entry).await;
        }
    }

    async fn emit(&self, kind: EventKind) {
        if let Some(a) = self.att.get() {
            if let Err(e) = a.events.publish(NewEvent::new(kind)).await {
                tracing::warn!(error = %e, "failed to publish remote event");
            }
        }
    }

    /// Records the decision on a remote RPC call (method, params hash, decision).
    pub async fn audit_call(
        &self,
        device: DeviceId,
        method: &str,
        params: &serde_json::Value,
        denied: Option<&RpcError>,
    ) {
        let hash = e2e::b64(&authz::params_hash(params));
        let (outcome, decision) = match denied {
            None => (AuditOutcome::Allowed, "allowed".to_owned()),
            Some(e) => (AuditOutcome::Denied, e.code.as_str().to_owned()),
        };
        self.audit(
            Actor::RemoteDevice(device),
            "remote.call",
            method.to_owned(),
            outcome,
            serde_json::json!({ "method": method, "paramsHash": hash, "decision": decision }),
        )
        .await;
    }

    // ------------------------------------------------------------------ status / kill switch

    /// Current status.
    #[must_use]
    pub fn status(&self) -> RemoteStatus {
        let s = self.state.lock();
        RemoteStatus {
            enabled: s.enabled,
            connected: self.connected.load(Ordering::SeqCst),
            gateway_url: s.gateway_url.clone(),
            core_id: s.core_id.clone(),
            sessions: u32::try_from(self.sessions.lock().len()).unwrap_or(u32::MAX),
            core_key_fingerprint: self.cik_public().map(|k| pairing::fingerprint(&k)),
            last_error: self.last_error.lock().clone(),
        }
    }

    /// `remote.enable`: persists the gateway URL (+ enrollment token) and dials.
    /// Waits up to 5 s for the first tunnel result.
    ///
    /// # Errors
    /// Invalid URL or not attached.
    pub async fn enable(&self, p: RemoteEnableParams) -> Result<RemoteStatus, RpcError> {
        self.att()?;
        let url = p.gateway_url.trim().trim_end_matches('/').to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(RpcError::bad_request("gatewayUrl must start with http:// or https://"));
        }
        {
            let mut s = self.state.lock();
            if s.gateway_url.as_deref() != Some(url.as_str()) {
                // a different gateway: re-enroll
                s.core_id = None;
                s.account_id = None;
                s.core_token = None;
            }
            if p.enrollment_token.is_some() {
                s.enrollment_token = p.enrollment_token.clone();
            }
            if s.core_id.is_none() && s.enrollment_token.is_none() {
                return Err(RpcError::bad_request("enrollmentToken is required for the first connection to a gateway"));
            }
            s.gateway_url = Some(url.clone());
            s.enabled = true;
        }
        self.persist();
        *self.last_error.lock() = None;
        self.stop_tunnel();
        self.start_tunnel();
        self.audit(Actor::System, "remote.enable", url, AuditOutcome::Allowed, serde_json::Value::Null).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !self.connected.load(Ordering::SeqCst) && self.last_error.lock().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(self.status())
    }

    /// `remote.disable` (kill switch): closes the tunnel and every session;
    /// the core does not re-dial until a **local** `remote.enable`.
    pub async fn disable(&self, actor: Actor) -> RemoteStatus {
        self.state.lock().enabled = false;
        self.persist();
        let closed = self.close_all_sessions("remote_disabled");
        self.stop_tunnel();
        self.audit(
            actor,
            "remote.disable",
            "tunnel".into(),
            AuditOutcome::Allowed,
            serde_json::json!({ "sessionsClosed": closed }),
        )
        .await;
        self.status()
    }

    /// `remote.revokeAllSessions`.
    pub async fn revoke_all_sessions(&self, actor: Actor) -> u32 {
        let n = self.close_all_sessions("sessions_revoked");
        self.step_ups.lock().clear();
        self.audit(
            actor,
            "remote.revokeAllSessions",
            "sessions".into(),
            AuditOutcome::Allowed,
            serde_json::json!({ "revoked": n }),
        )
        .await;
        n
    }

    pub(crate) fn close_all_sessions(&self, reason: &str) -> u32 {
        let drained: Vec<(u32, SessionCtl)> = self.sessions.lock().drain().collect();
        for (sid, s) in &drained {
            s.cancel.cancel();
            self.send_ctl(&ToGateway::Close { sid: *sid, reason: reason.into() });
        }
        u32::try_from(drained.len()).unwrap_or(u32::MAX)
    }

    fn start_tunnel(&self) {
        let Some(a) = self.att.get() else { return };
        let Some(me) = a.me.upgrade() else { return };
        let cancel = a.shutdown.child_token();
        *self.tunnel.lock() = Some(TunnelCtl { cancel: cancel.clone(), tx: None });
        tokio::spawn(tunnel::run(me, cancel));
    }

    fn stop_tunnel(&self) {
        if let Some(t) = self.tunnel.lock().take() {
            t.cancel.cancel();
        }
        self.connected.store(false, Ordering::SeqCst);
    }

    pub(crate) fn send_ctl(&self, frame: &ToGateway) {
        let Ok(text) = serde_json::to_string(frame) else { return };
        self.send_raw(tokio_tungstenite::tungstenite::Message::Text(text.into()));
    }

    pub(crate) fn send_data(&self, sid: u32, bytes: &[u8]) {
        let mut buf = Vec::with_capacity(4 + bytes.len());
        buf.extend_from_slice(&sid.to_be_bytes());
        buf.extend_from_slice(bytes);
        self.send_raw(tokio_tungstenite::tungstenite::Message::Binary(buf.into()));
    }

    fn send_raw(&self, msg: tokio_tungstenite::tungstenite::Message) {
        let tx = self.tunnel.lock().as_ref().and_then(|t| t.tx.clone());
        if let Some(tx) = tx {
            if tx.try_send(msg).is_err() {
                tracing::warn!("tunnel send buffer full or closed; frame dropped");
            }
        }
    }

    // ------------------------------------------------------------------ pairing

    /// `devices.startPairing`.
    ///
    /// # Errors
    /// Remote access is not enrolled at a gateway.
    pub fn start_pairing(&self) -> Result<DevicePairingStart, RpcError> {
        self.att()?;
        let (core_id, gateway_url) = {
            let s = self.state.lock();
            match (s.core_id.clone(), s.gateway_url.clone()) {
                (Some(c), Some(g)) if s.enabled => (c, g),
                _ => return Err(RpcError::conflict("enable remote access (remote.enable) before pairing a device")),
            }
        };
        let mut id = [0u8; 16];
        rand_core06::OsRng.fill_bytes(&mut id);
        let pairing_id = hex::encode(id);
        let code = pairing::generate_code();
        let expires_at =
            chrono::Utc::now() + chrono::Duration::seconds(i64::try_from(PAIRING_TTL.as_secs()).unwrap_or(600));
        let fp = self.cik_public().map(|k| pairing::fingerprint(&k)).unwrap_or_default();
        let qr = format!(
            "orkestr-pair:{}",
            serde_json::json!({ "gatewayUrl": gateway_url, "coreId": core_id, "pairingId": pairing_id, "code": code, "coreKeyFingerprint": fp })
        );
        let mut pairings = self.pairings.lock();
        pairings.retain(|_, p| p.created.elapsed() < PAIRING_TTL);
        pairings.insert(
            pairing_id.clone(),
            Pairing { code: code.clone(), created: Instant::now(), attempts: 0, claim: None },
        );
        Ok(DevicePairingStart { pairing_id, code, expires_at, core_id, gateway_url, core_key_fingerprint: fp, qr })
    }

    /// Handles a `pairingClaim` relayed by the gateway; returns the ack frame.
    pub(crate) async fn handle_claim(
        &self,
        pairing_id: &str,
        device_pub_b64: &str,
        name: &str,
        platform: &str,
        code_mac_b64: &str,
    ) -> ToGateway {
        let nack = |e: &str| ToGateway::PairingAck {
            pairing_id: pairing_id.to_owned(),
            ok: false,
            device_id: None,
            error: Some(e.into()),
        };
        let Ok(device_pub) = e2e::unb64(device_pub_b64) else { return nack("malformed") };
        if e2e::parse_public(&device_pub).is_err() {
            return nack("malformed");
        }
        let mac = e2e::unb64(code_mac_b64).unwrap_or_default();
        let core_pub = self.cik_public().unwrap_or_default();
        let result = {
            let mut pairings = self.pairings.lock();
            let Some(p) = pairings.get_mut(pairing_id) else { return nack("unknown_pairing") };
            if p.created.elapsed() >= PAIRING_TTL {
                pairings.remove(pairing_id);
                return nack("expired");
            }
            if p.claim.is_some() {
                return nack("already_claimed");
            }
            if pairing::verify_code_mac(&p.code, &device_pub, pairing_id, &mac) {
                let claim = Claim {
                    device_id: DeviceId::new(),
                    sas: pairing::sas(&core_pub, &device_pub, pairing_id),
                    device_pub,
                    name: name.chars().take(64).collect(),
                    platform: platform.chars().take(32).collect(),
                    at: chrono::Utc::now(),
                };
                p.claim = Some(claim.clone());
                Ok(claim)
            } else {
                p.attempts += 1;
                if p.attempts >= PAIRING_MAX_ATTEMPTS {
                    pairings.remove(pairing_id);
                }
                Err(())
            }
        };
        match result {
            Ok(claim) => {
                self.audit(
                    Actor::System,
                    "devices.pairingClaimed",
                    pairing_id.to_owned(),
                    AuditOutcome::Allowed,
                    serde_json::json!({ "deviceId": claim.device_id, "name": claim.name, "platform": claim.platform }),
                )
                .await;
                ToGateway::PairingAck {
                    pairing_id: pairing_id.to_owned(),
                    ok: true,
                    device_id: Some(claim.device_id.to_string()),
                    error: None,
                }
            }
            Err(()) => {
                self.audit(
                    Actor::System,
                    "devices.pairingClaimed",
                    pairing_id.to_owned(),
                    AuditOutcome::Denied,
                    serde_json::json!({ "reason": "bad codeMac" }),
                )
                .await;
                nack("bad_code")
            }
        }
    }

    /// `devices.approve` (local only).
    ///
    /// # Errors
    /// Unknown/unclaimed pairing or SAS not confirmed.
    pub async fn approve(&self, p: DeviceApproveParams) -> Result<DeviceInfo, RpcError> {
        let a = self.att()?;
        if !p.sas_confirmed {
            return Err(RpcError::bad_request("confirm that the SAS matches on both devices (sasConfirmed)"));
        }
        let claim = {
            let mut pairings = self.pairings.lock();
            let Some(pairing) = pairings.get(&p.pairing_id) else {
                return Err(RpcError::not_found("pairing not found or expired"));
            };
            let Some(claim) = pairing.claim.clone() else {
                return Err(RpcError::conflict("no device has claimed this pairing yet"));
            };
            pairings.remove(&p.pairing_id);
            claim
        };
        let record = DeviceRecord {
            device_id: claim.device_id,
            name: claim.name,
            platform: claim.platform,
            public_key: e2e::b64(&claim.device_pub),
            role: p.role,
            remote_terminal: p.remote_terminal,
            project_ids: p.project_ids.filter(|v| !v.is_empty()),
            created_at: chrono::Utc::now(),
            last_seen_at: None,
            revoked: false,
            step_up: None,
            step_up_kind: StepUpKind::default(),
        };
        self.state.lock().devices.push(record.clone());
        self.persist();
        let device_id = record.device_id.to_string();
        let msg = pairing::completion_message(&p.pairing_id, &device_id, &claim.device_pub, p.role);
        let sig: p256::ecdsa::Signature = p256::ecdsa::signature::Signer::sign(&a.cik, &msg);
        self.send_ctl(&ToGateway::PairingResult {
            pairing_id: p.pairing_id.clone(),
            status: "approved".into(),
            device_id: device_id.clone(),
            role: Some(p.role),
            sig: Some(e2e::b64(&sig.to_bytes())),
        });
        self.send_ctl(&ToGateway::DeviceAuthorized {
            device_id,
            name: record.name.clone(),
            public_key: record.public_key.clone(),
            role: record.role,
        });
        let info = record.info();
        self.audit(
            Actor::System,
            "devices.approve",
            record.device_id.to_string(),
            AuditOutcome::Approved,
            serde_json::json!({ "role": p.role, "remoteTerminal": p.remote_terminal }),
        )
        .await;
        self.emit(EventKind::DeviceAuthorized(DeviceAuthorizedPayload { device: info.clone() })).await;
        Ok(info)
    }

    /// `devices.list` (+ claimed pairings awaiting approval).
    #[must_use]
    pub fn list_devices(&self) -> DeviceList {
        let devices = self.state.lock().devices.iter().map(DeviceRecord::info).collect();
        let pending = self
            .pairings
            .lock()
            .iter()
            .filter(|(_, p)| p.created.elapsed() < PAIRING_TTL)
            .filter_map(|(id, p)| {
                p.claim.as_ref().map(|c| PendingPairing {
                    pairing_id: id.clone(),
                    device_id: c.device_id,
                    device_name: c.name.clone(),
                    platform: c.platform.clone(),
                    sas: c.sas.clone(),
                    claimed_at: c.at,
                })
            })
            .collect();
        DeviceList { devices, pending }
    }

    /// `devices.revoke`: authoritative revocation, closes the device's streams,
    /// tells the gateway, emits `DeviceRevoked`. Works with the gateway down.
    ///
    /// # Errors
    /// Unknown device.
    pub async fn revoke_device(&self, device_id: DeviceId, actor: Actor) -> Result<(), RpcError> {
        {
            let mut s = self.state.lock();
            let Some(d) = s.devices.iter_mut().find(|d| d.device_id == device_id) else {
                return Err(RpcError::not_found("device not found"));
            };
            d.revoked = true;
            d.step_up = None;
        }
        self.persist();
        self.limiter.forget(device_id);
        let closed: Vec<u32> = {
            let mut sessions = self.sessions.lock();
            let sids: Vec<u32> =
                sessions.iter().filter(|(_, s)| s.device_id == Some(device_id)).map(|(k, _)| *k).collect();
            for sid in &sids {
                if let Some(s) = sessions.remove(sid) {
                    s.cancel.cancel();
                }
            }
            sids
        };
        for sid in &closed {
            self.send_ctl(&ToGateway::Close { sid: *sid, reason: "device_revoked".into() });
            self.emit(EventKind::RemoteSessionRevoked(RemoteSessionPayload {
                device_id,
                stream_id: *sid,
                reason: Some("device_revoked".into()),
            }))
            .await;
        }
        self.send_ctl(&ToGateway::RevokeDevice { device_id: device_id.to_string() });
        self.audit(
            actor,
            "devices.revoke",
            device_id.to_string(),
            AuditOutcome::Allowed,
            serde_json::json!({ "sessionsClosed": closed.len() }),
        )
        .await;
        self.emit(EventKind::DeviceRevoked(DeviceRevokedPayload { device_id })).await;
        Ok(())
    }

    // ------------------------------------------------------------------ step-up

    /// `devices.registerStepUp`: stores the WebAuthn credential (registered
    /// inside the E2E channel) in the core-held device record.
    ///
    /// # Errors
    /// Invalid registration or unknown device.
    pub async fn register_step_up(&self, device_id: DeviceId, credential: &serde_json::Value) -> Result<(), RpcError> {
        let (cred, kind, details) = if native_stepup::is_native(credential) {
            let reg = native_stepup::parse_registration(credential)?;
            let details = serde_json::json!({ "kind": "native", "keyStorage": reg.key_storage });
            (reg.credential, StepUpKind::Native, details)
        } else {
            let rp =
                self.state.lock().rp.clone().ok_or_else(|| RpcError::conflict("gateway relying party unknown"))?;
            let reg: RegistrationJson = serde_json::from_value(credential.clone())
                .map_err(|e| RpcError::bad_request(format!("credential: {e}")))?;
            let cd =
                webauthn::unb64(&reg.response.client_data_json).map_err(|e| RpcError::bad_request(e.to_string()))?;
            let ao =
                webauthn::unb64(&reg.response.attestation_object).map_err(|e| RpcError::bad_request(e.to_string()))?;
            let cred =
                webauthn::verify_registration(&rp, None, &cd, &ao).map_err(|e| RpcError::bad_request(e.to_string()))?;
            (cred, StepUpKind::WebAuthn, serde_json::json!({ "kind": "webauthn" }))
        };
        {
            let mut s = self.state.lock();
            let d = s
                .devices
                .iter_mut()
                .find(|d| d.device_id == device_id && !d.revoked)
                .ok_or_else(|| RpcError::not_found("device not found"))?;
            d.step_up = Some(cred);
            d.step_up_kind = kind;
        }
        self.persist();
        self.audit(
            Actor::RemoteDevice(device_id),
            "devices.registerStepUp",
            device_id.to_string(),
            AuditOutcome::Allowed,
            details,
        )
        .await;
        Ok(())
    }

    /// Issues a single-use step-up challenge bound to device + method + params;
    /// returns it base64url (the WebAuthn `challenge`).
    pub fn issue_step_up(&self, device: DeviceId, method: &str, params_hash: &[u8; 32]) -> String {
        let nonce = e2e::random_nonce();
        let mut h = Sha256::new();
        h.update(b"orkestr-approve");
        h.update(method.as_bytes());
        h.update(params_hash);
        h.update(nonce);
        let challenge = e2e::b64(&h.finalize());
        let mut map = self.step_ups.lock();
        map.retain(|_, p| p.created.elapsed() < STEP_UP_TTL);
        map.insert(
            challenge.clone(),
            PendingStepUp { device, method: method.to_owned(), params_hash: *params_hash, created: Instant::now() },
        );
        challenge
    }

    /// Verifies a step-up assertion (consumes the challenge: replay is rejected).
    ///
    /// # Errors
    /// Reason string.
    pub fn verify_step_up(
        &self,
        device: DeviceId,
        method: &str,
        params_hash: &[u8; 32],
        assertion: &serde_json::Value,
    ) -> Result<(), String> {
        let a = AssertionJson::from_value(assertion).map_err(|e| e.to_string())?;
        let cd = webauthn::unb64(&a.client_data_json).map_err(|e| e.to_string())?;
        let challenge = webauthn::client_data_challenge(&cd).map_err(|e| e.to_string())?;
        let key = e2e::b64(&challenge);
        let pending = self.step_ups.lock().remove(&key).ok_or("unknown or already used challenge")?;
        if pending.device != device
            || pending.method != method
            || &pending.params_hash != params_hash
            || pending.created.elapsed() >= STEP_UP_TTL
        {
            return Err("challenge does not match this call".into());
        }
        let mut s = self.state.lock();
        let gateway_rp = s.rp.clone();
        let d = s.devices.iter_mut().find(|d| d.device_id == device && !d.revoked).ok_or("device not found")?;
        let rp = match d.step_up_kind {
            StepUpKind::WebAuthn => gateway_rp.ok_or("gateway relying party unknown")?,
            StepUpKind::Native => native_stepup::relying_party(device),
        };
        let kind = d.step_up_kind;
        let cred = d.step_up.as_mut().ok_or("no step-up credential registered (devices.registerStepUp)")?;
        if kind == StepUpKind::Native {
            native_stepup::check_counter(&a, cred.sign_count)?;
        }
        let count = webauthn::verify_assertion_json(&rp, &challenge, cred, &a).map_err(|e| e.to_string())?;
        cred.sign_count = count;
        drop(s);
        self.persist();
        Ok(())
    }

    // ------------------------------------------------------------------ push

    /// Sends an encrypted push message through the gateway.
    ///
    /// # Errors
    /// `unavailable` when the tunnel is not connected.
    pub async fn send_push(&self, req: PushRequest) -> Result<(), RpcError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(RpcError::conflict("remote tunnel is not connected"));
        }
        self.send_ctl(&ToGateway::Push {
            device_ids: req.device_ids.iter().map(ToString::to_string).collect(),
            endpoint: req.endpoint,
            body: e2e::b64(&req.body),
            collapse_key: req.collapse_key,
            urgency: req.urgency,
            transport: req.transport,
        });
        Ok(())
    }

    /// Native push transports announced by the gateway (`fcm`, `apns`).
    #[must_use]
    pub fn native_push_transports(&self) -> Vec<String> {
        self.state.lock().native_push.clone()
    }

    // ------------------------------------------------------------------ helpers for tunnel.rs

    pub(crate) fn device(&self, id: DeviceId) -> Option<DeviceRecord> {
        self.state.lock().devices.iter().find(|d| d.device_id == id).cloned()
    }

    pub(crate) fn check_replay(&self, device: &str, nonce: &[u8]) -> bool {
        let mut map = self.replay.lock();
        map.retain(|_, t| t.elapsed() < Duration::from_secs(600));
        map.insert((device.to_owned(), nonce.to_vec()), Instant::now()).is_none()
    }

    pub(crate) async fn session_opened(&self, device: DeviceId, sid: u32) {
        {
            let mut s = self.state.lock();
            if let Some(d) = s.devices.iter_mut().find(|d| d.device_id == device) {
                d.last_seen_at = Some(chrono::Utc::now());
            }
        }
        self.persist();
        self.audit(
            Actor::RemoteDevice(device),
            "remote.handshake",
            format!("sid:{sid}"),
            AuditOutcome::Allowed,
            serde_json::Value::Null,
        )
        .await;
        self.emit(EventKind::RemoteSessionOpened(RemoteSessionPayload {
            device_id: device,
            stream_id: sid,
            reason: None,
        }))
        .await;
    }

    pub(crate) async fn handshake_refused(&self, device: Option<DeviceId>, sid: u32, code: &str) {
        let actor = device.map_or(Actor::System, Actor::RemoteDevice);
        self.audit(
            actor,
            "remote.handshake",
            format!("sid:{sid}"),
            AuditOutcome::Denied,
            serde_json::json!({ "reason": code }),
        )
        .await;
    }

    pub(crate) fn cik(&self) -> Option<&SigningKey> {
        self.att.get().map(|a| &a.cik)
    }

    pub(crate) fn runner(&self) -> Option<SessionRunner> {
        self.att.get().map(|a| Arc::clone(&a.runner))
    }

    pub(crate) fn version(&self) -> String {
        self.att.get().map(|a| a.version.clone()).unwrap_or_default()
    }

    pub(crate) fn core_id(&self) -> Option<String> {
        self.state.lock().core_id.clone()
    }

    pub(crate) fn state_snapshot(&self) -> RemoteState {
        self.state.lock().clone()
    }

    pub(crate) fn update_state(&self, f: impl FnOnce(&mut RemoteState)) {
        f(&mut self.state.lock());
        self.persist();
    }

    pub(crate) fn set_connected(&self, v: bool) {
        self.connected.store(v, Ordering::SeqCst);
    }

    pub(crate) fn set_error(&self, e: Option<String>) {
        *self.last_error.lock() = e;
    }

    pub(crate) fn set_tunnel_tx(
        &self,
        cancel: &CancellationToken,
        tx: Option<mpsc::Sender<tokio_tungstenite::tungstenite::Message>>,
    ) {
        if cancel.is_cancelled() {
            return;
        }
        if let Some(t) = self.tunnel.lock().as_mut() {
            t.tx = tx;
        }
    }

    pub(crate) fn sessions(&self) -> &Mutex<HashMap<u32, SessionCtl>> {
        &self.sessions
    }

    pub(crate) fn now_ms() -> u64 {
        now_ms()
    }
}
