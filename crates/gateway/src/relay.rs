//! Core tunnel (`/v1/tunnel`), client relay (`/v1/connect`) and pairing relay
//! (`/v1/pairings/*`). The gateway never sees E2E plaintext: client messages are
//! forwarded as `sid_be32 ‖ bytes` and back unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{SinkExt, StreamExt};
use orkestr_protocol::methods::remote::tunnel::{ToCore, ToGateway, HELLO_CONTEXT};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::{auth, ApiError, ClientIp, GatewayState};

/// Max relayed message (1 MiB record + framing).
const MAX_MESSAGE: usize = (1 << 20) + 64;

struct CoreConn {
    conn_id: u64,
    account_id: String,
    tx: mpsc::Sender<Message>,
    streams: HashMap<u32, mpsc::Sender<Message>>,
}

/// Live tunnels.
#[derive(Default)]
pub struct Relay {
    cores: Mutex<HashMap<String, CoreConn>>,
    acks: Mutex<HashMap<String, oneshot::Sender<ToGateway>>>,
    next_sid: AtomicU32,
    next_conn: AtomicU64,
    /// Every relayed payload (tests: "relay sees no plaintext").
    pub capture: Option<Arc<Mutex<Vec<Vec<u8>>>>>,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay").field("cores", &self.cores.lock().len()).finish_non_exhaustive()
    }
}

impl Relay {
    /// Relay that records every relayed payload.
    #[must_use]
    pub fn with_capture() -> Self {
        Self { capture: Some(Arc::new(Mutex::new(Vec::new()))), ..Self::default() }
    }

    /// Whether a core's tunnel is connected.
    #[must_use]
    pub fn is_online(&self, core_id: &str) -> bool {
        self.cores.lock().contains_key(core_id)
    }

    fn send_ctl(&self, core_id: &str, frame: &ToCore) -> bool {
        let Ok(text) = serde_json::to_string(frame) else { return false };
        self.cores.lock().get(core_id).is_some_and(|c| c.tx.try_send(Message::Text(text.into())).is_ok())
    }

    /// Drops every tunnel and stream of an account ("panic").
    pub fn drop_account(&self, account: &str) {
        self.cores.lock().retain(|_, c| c.account_id != account);
    }
}

fn verify_hello(cik_b64: &str, nonce: &[u8], sig_b64: &str) -> bool {
    let (Ok(pk), Ok(sig)) = (orkestr_security::webauthn::unb64(cik_b64), orkestr_security::webauthn::unb64(sig_b64))
    else {
        return false;
    };
    let (Ok(vk), Ok(sig)) = (VerifyingKey::from_sec1_bytes(&pk), Signature::from_slice(&sig)) else { return false };
    let mut msg = HELLO_CONTEXT.as_bytes().to_vec();
    msg.extend_from_slice(nonce);
    vk.verify(&msg, &sig).is_ok()
}

/// `GET /v1/tunnel` (core).
pub async fn tunnel(State(st): State<GatewayState>, ClientIp(ip): ClientIp, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_MESSAGE * 2).on_upgrade(move |socket| run_tunnel(st, socket, ip))
}

async fn send_err(socket: &mut WebSocket, code: &str, message: &str) {
    let f = ToCore::Error { code: code.into(), message: message.into() };
    if let Ok(t) = serde_json::to_string(&f) {
        let _ = socket.send(Message::Text(t.into())).await;
    }
    let _ = socket.close().await;
}

async fn run_tunnel(st: GatewayState, mut socket: WebSocket, ip: String) {
    let nonce = crate::random_bytes(32);
    let challenge = ToCore::Challenge { nonce: orkestr_security::webauthn::b64(&nonce) };
    let Ok(text) = serde_json::to_string(&challenge) else { return };
    if socket.send(Message::Text(text.into())).await.is_err() {
        return;
    }
    let hello = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(t))) => return serde_json::from_str::<ToGateway>(t.as_str()).ok(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    let Some(ToGateway::Hello { core_id, cik_public, enrollment_token, sig, .. }) = hello else {
        return send_err(&mut socket, "bad_hello", "expected hello").await;
    };
    if !verify_hello(&cik_public, &nonce, &sig) {
        st.store.audit(None, "tunnel.rejected", Some(&ip), &json!({ "reason": "bad_signature" }));
        return send_err(&mut socket, "bad_signature", "CIK signature did not verify").await;
    }
    let (core_id, account_id, core_token) = match core_id {
        Some(id) => match st.store.core(&id) {
            Some(row) if row.cik_public == cik_public => (id, row.account_id, None),
            _ => {
                st.store.audit(None, "tunnel.rejected", Some(&ip), &json!({ "reason": "unknown_core", "coreId": id }));
                return send_err(&mut socket, "unknown_core", "core not enrolled or key mismatch").await;
            }
        },
        None => {
            let Some(token) = enrollment_token else {
                return send_err(&mut socket, "bad_enrollment_token", "enrollment token required").await;
            };
            match st.store.enroll(&token, &cik_public, "core") {
                Some((c, a, t)) => {
                    st.store.audit(Some(&a), "cores.enrolled", Some(&ip), &json!({ "coreId": c }));
                    (c, a, Some(t))
                }
                None => {
                    st.store.audit(None, "tunnel.rejected", Some(&ip), &json!({ "reason": "bad_enrollment_token" }));
                    return send_err(&mut socket, "bad_enrollment_token", "invalid, used or expired enrollment token")
                        .await;
                }
            }
        }
    };
    let welcome = ToCore::Welcome {
        core_id: core_id.clone(),
        account_id: account_id.clone(),
        core_token,
        rp_id: st.rp.rp_id.clone(),
        origins: st.rp.origins.clone(),
    };
    let Ok(text) = serde_json::to_string(&welcome) else { return };
    if socket.send(Message::Text(text.into())).await.is_err() {
        return;
    }
    st.store.touch_core(&core_id);
    st.store.audit(Some(&account_id), "tunnel.connected", Some(&ip), &json!({ "coreId": core_id }));

    let (tx, mut rx) = mpsc::channel::<Message>(1024);
    let conn_id = st.relay.next_conn.fetch_add(1, Ordering::Relaxed);
    if let Some(old) = st
        .relay
        .cores
        .lock()
        .insert(core_id.clone(), CoreConn { conn_id, account_id: account_id.clone(), tx, streams: HashMap::new() })
    {
        drop(old); // superseded: its writer ends when the sender drops
    }
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(60), stream.next()).await;
        let Ok(Some(Ok(msg))) = msg else { break }; // 60 s without traffic (pings every 20 s) ⇒ offline
        match msg {
            Message::Text(t) => match serde_json::from_str::<ToGateway>(t.as_str()) {
                Ok(f) => on_core_frame(&st, &core_id, &account_id, f).await,
                Err(e) => tracing::warn!(error = %e, "bad frame from core"),
            },
            Message::Binary(b) => {
                if b.len() < 4 {
                    continue;
                }
                let sid = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                if let Some(cap) = &st.relay.capture {
                    cap.lock().push(b[4..].to_vec());
                }
                let tx = st.relay.cores.lock().get(&core_id).and_then(|c| c.streams.get(&sid).cloned());
                if let Some(tx) = tx {
                    let _ = tx.try_send(Message::Binary(b.slice(4..)));
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    let mut cores = st.relay.cores.lock();
    if cores.get(&core_id).is_some_and(|c| c.conn_id == conn_id) {
        cores.remove(&core_id); // dropping stream senders closes client sockets
    }
    drop(cores);
    st.store.audit(Some(&account_id), "tunnel.disconnected", Some(&ip), &json!({ "coreId": core_id }));
}

async fn on_core_frame(st: &GatewayState, core_id: &str, account: &str, f: ToGateway) {
    match f {
        ToGateway::Close { sid, reason } => {
            let tx = st.relay.cores.lock().get_mut(core_id).and_then(|c| c.streams.remove(&sid));
            if let Some(tx) = tx {
                let _ = tx.try_send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4403,
                    reason: reason.into(),
                })));
            }
        }
        ToGateway::PairingAck { ref pairing_id, .. } => {
            if let Some(tx) = st.relay.acks.lock().remove(pairing_id) {
                let _ = tx.send(f);
            }
        }
        ToGateway::PairingResult { pairing_id, status, device_id, role, sig } => {
            let result = json!({ "status": status, "deviceId": device_id, "role": role, "sig": sig });
            st.store.put_pairing(&pairing_id, core_id, account, &status, Some(&device_id), &result);
            st.store.audit(
                Some(account),
                "pairing.result",
                None,
                &json!({ "pairingId": pairing_id, "status": status }),
            );
        }
        ToGateway::DeviceAuthorized { device_id, name, public_key, role } => {
            let role = serde_json::to_value(role).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            st.store.upsert_device(core_id, &device_id, &name, &public_key, &role, "active");
            st.store.audit(Some(account), "devices.authorized", None, &json!({ "deviceId": device_id, "role": role }));
        }
        ToGateway::RevokeDevice { device_id } => {
            st.store.revoke_device(&device_id);
            st.store.audit(Some(account), "devices.revoked", None, &json!({ "deviceId": device_id }));
        }
        ToGateway::Push { device_ids, endpoint, body, collapse_key, urgency } => {
            let Ok(body) = orkestr_security::webauthn::unb64(&body) else { return };
            let msg = crate::push::PushDelivery { endpoint, body, topic: collapse_key, urgency, ttl_secs: 24 * 3600 };
            let store = Arc::clone(&st.store);
            let account = account.to_owned();
            tokio::spawn(async move {
                let outcome = crate::push::deliver(&msg).await;
                store.audit(
                    Some(&account),
                    "push.deliver",
                    None,
                    &json!({ "devices": device_ids, "ok": outcome.is_ok(), "error": outcome.err().map(|e| e.to_string()) }),
                );
            });
        }
        ToGateway::Hello { .. } => {}
    }
}

/// Query of `/v1/connect`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectQuery {
    core_id: String,
    #[serde(default)]
    token: Option<String>,
}

/// `GET /v1/connect?coreId=…[&token=…]` (client WS; session cookie / bearer / token).
pub async fn connect(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(q): Query<ConnectQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let account = auth::bearer(&headers, q.token.as_deref())
        .and_then(|t| st.store.session(&t))
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "sign in first"))?;
    match st.store.core(&q.core_id) {
        Some(c) if c.account_id == account => {}
        _ => return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown core")),
    }
    if !st.relay.is_online(&q.core_id) {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "core offline"));
    }
    st.windows.hit(&format!("connect:{account}"), 30, Duration::from_secs(60)).map_err(ApiError::too_many)?;
    let core_id = q.core_id;
    Ok(ws
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run_client(st, socket, core_id, account, ip))
        .into_response())
}

async fn run_client(st: GatewayState, socket: WebSocket, core_id: String, account: String, ip: String) {
    let sid = st.relay.next_sid.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let (tx, mut rx) = mpsc::channel::<Message>(512);
    let core_tx = {
        let mut cores = st.relay.cores.lock();
        let Some(c) = cores.get_mut(&core_id) else { return };
        c.streams.insert(sid, tx);
        c.tx.clone()
    };
    if !st.relay.send_ctl(&core_id, &ToCore::Open { sid, client_ip: Some(ip.clone()) }) {
        return;
    }
    st.store.audit(Some(&account), "relay.open", Some(&ip), &json!({ "coreId": core_id, "sid": sid }));
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = sink.close().await;
    });
    while let Some(Ok(msg)) = stream.next().await {
        let bytes: Vec<u8> = match msg {
            Message::Binary(b) => b.to_vec(),
            Message::Text(t) => t.as_str().as_bytes().to_vec(),
            Message::Close(_) => break,
            _ => continue,
        };
        if let Some(cap) = &st.relay.capture {
            cap.lock().push(bytes.clone());
        }
        let mut frame = Vec::with_capacity(4 + bytes.len());
        frame.extend_from_slice(&sid.to_be_bytes());
        frame.extend_from_slice(&bytes);
        if core_tx.send(Message::Binary(frame.into())).await.is_err() {
            break;
        }
    }
    let still_open = st.relay.cores.lock().get_mut(&core_id).and_then(|c| c.streams.remove(&sid)).is_some();
    if still_open {
        st.relay.send_ctl(&core_id, &ToCore::Close { sid });
    }
    writer.abort();
}

/// Body of a pairing claim.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimBody {
    core_id: String,
    device_pub_key: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    platform: String,
    code_mac: String,
}

/// `POST /v1/pairings/{pairingId}/claim` (session).
pub async fn claim(
    State(st): State<GatewayState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(pairing_id): Path<String>,
    Json(b): Json<ClaimBody>,
) -> Result<Json<Value>, ApiError> {
    let account = auth::require(&st, &headers)?;
    st.windows.hit(&format!("claim-pairing:{pairing_id}"), 5, Duration::from_secs(600)).map_err(ApiError::too_many)?;
    st.windows.hit(&format!("claim-acct:{account}"), 20, Duration::from_secs(3600)).map_err(ApiError::too_many)?;
    let core = match st.store.core(&b.core_id) {
        Some(c) if c.account_id == account => c,
        _ => return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown core")),
    };
    let (tx, rx) = oneshot::channel();
    st.relay.acks.lock().insert(pairing_id.clone(), tx);
    let sent = st.relay.send_ctl(
        &core.core_id,
        &ToCore::PairingClaim {
            pairing_id: pairing_id.clone(),
            device_pub_key: b.device_pub_key,
            device_name: b.device_name.clone(),
            platform: b.platform,
            code_mac: b.code_mac,
        },
    );
    if !sent {
        st.relay.acks.lock().remove(&pairing_id);
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "core offline"));
    }
    let ack = tokio::time::timeout(Duration::from_secs(10), rx).await;
    st.relay.acks.lock().remove(&pairing_id);
    let Ok(Ok(ToGateway::PairingAck { ok, device_id, error, .. })) = ack else {
        return Err(ApiError::new(StatusCode::GATEWAY_TIMEOUT, "core did not answer"));
    };
    st.store.audit(
        Some(&account),
        "pairing.claim",
        Some(&ip),
        &json!({ "pairingId": pairing_id, "ok": ok, "error": error }),
    );
    if !ok {
        return Err(ApiError::new(StatusCode::FORBIDDEN, error.unwrap_or_else(|| "rejected".into())));
    }
    st.store.put_pairing(&pairing_id, &core.core_id, &account, "pending", device_id.as_deref(), &Value::Null);
    Ok(Json(
        json!({ "status": "pending", "deviceId": device_id, "coreId": core.core_id, "corePublicKey": core.cik_public }),
    ))
}

/// `GET /v1/pairings/{pairingId}` (session).
pub async fn pairing_status(
    State(st): State<GatewayState>,
    headers: HeaderMap,
    Path(pairing_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account = auth::require(&st, &headers)?;
    let Some((acc, core_id, status, result)) = st.store.pairing(&pairing_id) else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown pairing"));
    };
    if acc != account {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown pairing"));
    }
    let core_pub = st.store.core(&core_id).map(|c| c.cik_public);
    Ok(Json(json!({ "status": status, "coreId": core_id, "corePublicKey": core_pub, "result": result })))
}
