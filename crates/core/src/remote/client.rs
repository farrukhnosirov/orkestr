//! Device-side pairing + E2E client in Rust (extracted from
//! `crates/gateway/tests/remote_e2e.rs`): used by a coordinator core to pair
//! with a worker core (`nodes.enroll`) and to open E2E sessions (Phase 6), and
//! by the remote-access integration tests as the "phone".
//!
//! * [`GatewayHttp`] — gateway HTTP APIs a device uses: pairing claim
//!   (`codeMac`), pairing status (completion signature verified against the
//!   pinned core key).
//! * [`E2eClient`] — `/v1/connect` + `e2e-v1` handshake, then the ordinary
//!   RPC frames (`auth`, `req`, `sub`) sealed into records; responses are
//!   matched by id, events are fanned out on a broadcast channel.
//!
//! The bearer token is a user session (devices) or a core token (core-to-core,
//! same account; ADR-0033). It travels in the `Authorization` header.
//! **Owner: WP-A.**

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_protocol::frames::{ResFrame, ServerFrame};
use orkestr_protocol::methods::Method;
use orkestr_protocol::policy::Role;
use orkestr_protocol::{Event, RpcError};
use p256::ecdsa::{SigningKey, VerifyingKey};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::{e2e, pairing};

/// Default per-call timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Handshake / auth timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Capacity of the event fan-out.
const EVENT_CAPACITY: usize = 1024;

/// Client errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    /// The gateway answered an HTTP error (`401`, `404 unknown core`, `503 core offline`, …).
    #[error("gateway {status}: {message}")]
    Gateway {
        /// HTTP status.
        status: u16,
        /// Message.
        message: String,
    },
    /// The core refused the E2E handshake (`revoked`, `unknown_device`, `wrong_core`, …).
    #[error("handshake refused: {0}")]
    Refused(String),
    /// Network / protocol failure.
    #[error("transport: {0}")]
    Transport(String),
    /// The session is closed.
    #[error("session closed")]
    Closed,
    /// No answer in time.
    #[error("timed out")]
    Timeout,
    /// The core answered with an RPC error.
    #[error(transparent)]
    Rpc(RpcError),
}

/// `ws(s)://…` from an `http(s)://` gateway base.
fn ws_base(gateway: &str) -> String {
    let base = gateway.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base.to_owned()
    } else {
        format!("ws://{base}")
    }
}

fn http_base(gateway: &str) -> String {
    let base = gateway.trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        base.to_owned()
    } else {
        format!("http://{base}")
    }
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// SEC1 public key of a device key.
#[must_use]
pub fn device_public(key: &SigningKey) -> Vec<u8> {
    e2e::public_raw(&p256::PublicKey::from(key.verifying_key()))
}

// ---------------------------------------------------------------- pairing (HTTP)

/// A claimed pairing (waiting for approval on the core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedPairing {
    /// Device id the core assigned.
    pub device_id: String,
    /// The core's identity key (SEC1), as reported by the gateway.
    pub core_public_key: Vec<u8>,
    /// Short authentication string (compare with the core's screen).
    pub sas: String,
}

/// Gateway HTTP client of a device.
#[derive(Debug, Clone)]
pub struct GatewayHttp {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl GatewayHttp {
    /// Client for `gateway` (`http://host:port`) with a bearer `token`.
    #[must_use]
    pub fn new(gateway: &str, token: &str) -> Self {
        let http = reqwest::Client::builder().timeout(CALL_TIMEOUT).build().unwrap_or_default();
        Self { http, base: http_base(gateway), token: token.to_owned() }
    }

    async fn json(res: reqwest::Response) -> Result<Value, ClientError> {
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            let message = body["error"]["message"]
                .as_str()
                .or_else(|| body["message"].as_str())
                .or_else(|| body["error"].as_str())
                .unwrap_or("request failed")
                .to_owned();
            Err(ClientError::Gateway { status: status.as_u16(), message })
        }
    }

    /// `POST /v1/pairings/{id}/claim` with `codeMac = HMAC(code, devicePub ‖ pairingId)`.
    ///
    /// # Errors
    /// Gateway refusal (`403 bad_code`, `404`, `503`) or transport failure.
    pub async fn claim(
        &self,
        core_id: &str,
        pairing_id: &str,
        code: &str,
        device_key: &SigningKey,
        device_name: &str,
        platform: &str,
    ) -> Result<ClaimedPairing, ClientError> {
        let device_pub = device_public(device_key);
        let mac = pairing::code_mac(code.trim(), &device_pub, pairing_id);
        let res = self
            .http
            .post(format!("{}/v1/pairings/{pairing_id}/claim", self.base))
            .bearer_auth(&self.token)
            .json(&json!({
                "coreId": core_id, "devicePubKey": e2e::b64(&device_pub), "deviceName": device_name,
                "platform": platform, "codeMac": e2e::b64(&mac),
            }))
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let v = Self::json(res).await?;
        let device_id = v["deviceId"].as_str().ok_or_else(|| ClientError::Transport("no deviceId".into()))?.to_owned();
        let core_public_key = v["corePublicKey"]
            .as_str()
            .and_then(|k| e2e::unb64(k).ok())
            .ok_or_else(|| ClientError::Transport("no corePublicKey".into()))?;
        let sas = pairing::sas(&core_public_key, &device_pub, pairing_id);
        Ok(ClaimedPairing { device_id, core_public_key, sas })
    }

    /// `GET /v1/pairings/{id}` (raw JSON: `status`, `coreId`, `corePublicKey`, `result`).
    ///
    /// # Errors
    /// Gateway refusal or transport failure.
    pub async fn pairing_status(&self, pairing_id: &str) -> Result<Value, ClientError> {
        let res = self
            .http
            .get(format!("{}/v1/pairings/{pairing_id}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Self::json(res).await
    }

    /// Checks once whether the pairing was approved: `Ok(Some(role))` when the
    /// completion signature verifies against the pinned core key, `Ok(None)`
    /// while pending.
    ///
    /// # Errors
    /// Rejected pairing, bad completion signature, gateway / transport failure.
    pub async fn approval(
        &self,
        pairing_id: &str,
        claimed: &ClaimedPairing,
        device_key: &SigningKey,
    ) -> Result<Option<Role>, ClientError> {
        let st = self.pairing_status(pairing_id).await?;
        match st["status"].as_str() {
            Some("approved") => {}
            Some("pending") | None => return Ok(None),
            Some(other) => return Err(ClientError::Refused(other.to_owned())),
        }
        let role: Role = serde_json::from_value(st["result"]["role"].clone())
            .map_err(|_| ClientError::Transport("approved pairing without role".into()))?;
        let sig = st["result"]["sig"].as_str().unwrap_or_default();
        let msg = pairing::completion_message(pairing_id, &claimed.device_id, &device_public(device_key), role);
        let core_key = VerifyingKey::from_sec1_bytes(&claimed.core_public_key)
            .map_err(|_| ClientError::Transport("bad core key".into()))?;
        e2e::verify(&core_key, &msg, sig).map_err(|_| ClientError::Refused("bad_completion_signature".into()))?;
        Ok(Some(role))
    }
}

// ---------------------------------------------------------------- E2E session

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<ResFrame>>>>;

/// An authenticated E2E RPC session to one core through the gateway.
#[derive(Debug)]
pub struct E2eClient {
    out: mpsc::Sender<Value>,
    pending: Pending,
    events: broadcast::Sender<Event>,
    closed: CancellationToken,
    next: AtomicU64,
    timeout: Duration,
}

impl Drop for E2eClient {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv_frame(ws: &mut Ws) -> Result<Vec<u8>, ClientError> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(b))) => return Ok(b.to_vec()),
            Some(Ok(Message::Text(t))) => return Ok(t.as_str().as_bytes().to_vec()),
            Some(Ok(Message::Close(f))) => {
                return Err(f.map_or(ClientError::Closed, |f| ClientError::Refused(f.reason.to_string())))
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(ClientError::Transport(e.to_string())),
            None => return Err(ClientError::Closed),
        }
    }
}

impl E2eClient {
    /// Connects through `gateway` to `core_id` as `device_id`, signing with
    /// `device_key` and pinning `core_public_key` (SEC1); authenticates the
    /// RPC session.
    ///
    /// # Errors
    /// `Gateway` (HTTP refusal of `/v1/connect`), `Refused(code)` (the core
    /// refused the handshake), `Transport`, `Timeout`.
    pub async fn connect(
        gateway: &str,
        token: &str,
        core_id: &str,
        device_id: &str,
        device_key: &SigningKey,
        core_public_key: &[u8],
    ) -> Result<Self, ClientError> {
        let core_key = VerifyingKey::from_sec1_bytes(core_public_key)
            .map_err(|_| ClientError::Transport("bad core public key".into()))?;
        let url = format!("{}/v1/connect?coreId={core_id}", ws_base(gateway));
        let mut req = url.into_client_request().map_err(|e| ClientError::Transport(e.to_string()))?;
        let auth = format!("Bearer {token}")
            .parse()
            .map_err(|_| ClientError::Transport("token is not a valid header value".into()))?;
        req.headers_mut().insert("authorization", auth);
        let handshake = async {
            let (mut ws, _) = tokio_tungstenite::connect_async(req).await.map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(res) => {
                    let message = res
                        .body()
                        .as_ref()
                        .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    ClientError::Gateway { status: res.status().as_u16(), message }
                }
                other => ClientError::Transport(other.to_string()),
            })?;
            let (hs, hello) = e2e::DeviceHandshake::start(core_id, device_id, now_ms());
            let hello = String::from_utf8(hello.to_bytes()).map_err(|e| ClientError::Transport(e.to_string()))?;
            ws.send(Message::Text(hello.into())).await.map_err(|e| ClientError::Transport(e.to_string()))?;
            let server_hello = recv_frame(&mut ws).await?;
            let frame =
                e2e::HandshakeFrame::from_bytes(&server_hello).map_err(|e| ClientError::Transport(e.to_string()))?;
            if let e2e::HandshakeFrame::Error { code } = &frame {
                return Err(ClientError::Refused(code.clone()));
            }
            let (finish, session) =
                hs.finish(&frame, device_key, &core_key).map_err(|e| ClientError::Refused(e.to_string()))?;
            ws.send(Message::Binary(finish.to_bytes().into())).await.map_err(|e| ClientError::Transport(e.to_string()))?;
            Ok::<_, ClientError>((ws, session))
        };
        let (ws, session) = tokio::time::timeout(CONNECT_TIMEOUT, handshake).await.map_err(|_| ClientError::Timeout)??;
        let client = Self::spawn(ws, session);
        // `auth` is answered like any request (the token is ignored on remote sessions).
        let res = client.round_trip("auth", |id| json!({"type": "auth", "id": id, "token": "e2e"}), CONNECT_TIMEOUT).await?;
        if !res.ok {
            return Err(res.error.map_or(ClientError::Closed, ClientError::Rpc));
        }
        Ok(client)
    }

    fn spawn(ws: Ws, session: e2e::Session) -> Self {
        let e2e::Session { mut sealer, mut opener, .. } = session;
        let (mut sink, mut stream) = ws.split();
        let (out, mut out_rx) = mpsc::channel::<Value>(256);
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let closed = CancellationToken::new();
        // writer: plaintext frame → record
        {
            let closed = closed.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = closed.cancelled() => break,
                        m = out_rx.recv() => {
                            let Some(v) = m else { break };
                            let Ok(rec) = sealer.seal(v.to_string().as_bytes()) else { break };
                            if sink.send(Message::Binary(rec.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                closed.cancel();
                let _ = sink.close().await;
            });
        }
        // reader: record → res / event
        {
            let closed = closed.clone();
            let pending = Arc::clone(&pending);
            let events = events.clone();
            tokio::spawn(async move {
                loop {
                    let msg = tokio::select! {
                        () = closed.cancelled() => break,
                        m = stream.next() => m,
                    };
                    let rec = match msg {
                        Some(Ok(Message::Binary(b))) => b.to_vec(),
                        Some(Ok(Message::Text(t))) => t.as_str().as_bytes().to_vec(),
                        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                        Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    };
                    let Ok(pt) = opener.open(&rec) else {
                        tracing::warn!("E2E record rejected; closing the session");
                        break;
                    };
                    match serde_json::from_slice::<ServerFrame>(&pt) {
                        Ok(ServerFrame::Res(res)) => {
                            if let Some(tx) = pending.lock().remove(&res.id) {
                                let _ = tx.send(res);
                            }
                        }
                        Ok(ServerFrame::Event { event, .. }) => {
                            let _ = events.send(*event);
                        }
                        Err(e) => tracing::debug!(error = %e, "unparsed frame on E2E session"),
                    }
                }
                closed.cancel();
                pending.lock().clear(); // callers see `Closed`
            });
        }
        Self { out, pending, events, closed, next: AtomicU64::new(0), timeout: CALL_TIMEOUT }
    }

    async fn round_trip(
        &self,
        prefix: &str,
        frame: impl FnOnce(&str) -> Value,
        timeout: Duration,
    ) -> Result<ResFrame, ClientError> {
        if self.closed.is_cancelled() {
            return Err(ClientError::Closed);
        }
        let id = format!("{prefix}{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.clone(), tx);
        if self.out.send(frame(&id)).await.is_err() {
            self.pending.lock().remove(&id);
            return Err(ClientError::Closed);
        }
        tokio::select! {
            r = tokio::time::timeout(timeout, rx) => match r {
                Ok(Ok(res)) => Ok(res),
                Ok(Err(_)) => Err(ClientError::Closed),
                Err(_) => {
                    self.pending.lock().remove(&id);
                    Err(ClientError::Timeout)
                }
            },
            () = self.closed.cancelled() => {
                self.pending.lock().remove(&id);
                Err(ClientError::Closed)
            }
        }
    }

    /// Calls a method; `Err(Rpc)` carries the core's error.
    ///
    /// # Errors
    /// RPC error, closed session, timeout.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let res = self.call_raw(method, params).await?;
        if res.ok {
            Ok(res.result.unwrap_or(Value::Null))
        } else {
            Err(res.error.map_or(ClientError::Closed, ClientError::Rpc))
        }
    }

    /// Calls a method and returns the raw response frame.
    ///
    /// # Errors
    /// Closed session, timeout.
    pub async fn call_raw(&self, method: &str, params: Value) -> Result<ResFrame, ClientError> {
        self.round_trip(
            "c",
            |id| json!({"type": "req", "id": id, "method": method, "params": params}),
            self.timeout,
        )
        .await
    }

    /// Typed call.
    ///
    /// # Errors
    /// As [`Self::call`]; `Transport` when (de)serialization fails.
    pub async fn call_typed<M: Method>(&self, params: &M::Params) -> Result<M::Result, ClientError> {
        let v = serde_json::to_value(params).map_err(|e| ClientError::Transport(e.to_string()))?;
        let out = self.call(M::NAME, v).await?;
        serde_json::from_value(out).map_err(|e| ClientError::Transport(format!("bad result of {}: {e}", M::NAME)))
    }

    /// Subscribes to topics; events arrive on [`Self::events`].
    ///
    /// # Errors
    /// The core refused a topic, closed session, timeout.
    pub async fn subscribe(&self, topics: &[&str]) -> Result<(), ClientError> {
        let res = self
            .round_trip("s", |id| json!({"type": "sub", "id": id, "topics": topics}), self.timeout)
            .await?;
        if res.ok {
            Ok(())
        } else {
            Err(res.error.map_or(ClientError::Closed, ClientError::Rpc))
        }
    }

    /// Events of the subscribed topics.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Whether the session is closed (revoked, gateway gone, …).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Resolves when the session closes.
    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }

    /// Closes the session.
    pub fn close(&self) {
        self.closed.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(ws_base("http://127.0.0.1:8080/"), "ws://127.0.0.1:8080");
        assert_eq!(ws_base("https://gw.example"), "wss://gw.example");
        assert_eq!(ws_base("127.0.0.1:1"), "ws://127.0.0.1:1");
        assert_eq!(http_base("127.0.0.1:1"), "http://127.0.0.1:1");
        assert_eq!(http_base("https://gw/"), "https://gw");
    }
}
