//! Outbound tunnel to the gateway (remote-access.md §5) and the per-stream
//! E2E endpoint (§6). The core never listens: it dials `ws(s)://<gw>/v1/tunnel`,
//! proves its identity by signing the gateway's challenge with the CIK, then
//! multiplexes client streams (`sid_be32 ‖ bytes`).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_domain::DeviceId;
use orkestr_protocol::methods::remote::tunnel::{ToCore, ToGateway, HELLO_CONTEXT};
use orkestr_security::webauthn::RelyingParty;
use p256::ecdsa::signature::Signer;
use rand::Rng;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::e2e::{self, ClientHelloData, CoreHandshake, HandshakeFrame};
use super::{RemoteService, SessionCtl};
use crate::rpc::{Principal, RemotePrincipal};

/// Max concurrent client streams per core.
const MAX_STREAMS: usize = 256;
/// Keepalive ping interval.
const PING_EVERY: Duration = Duration::from_secs(20);
/// Handshake must complete within this time.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

fn tunnel_url(gateway: &str) -> String {
    let base = gateway.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    format!("{ws}/v1/tunnel")
}

/// Dial loop with exponential backoff (1 s → 60 s, jitter). Exits when
/// cancelled (kill switch / shutdown) or when remote access is disabled.
pub(crate) async fn run(svc: Arc<RemoteService>, cancel: CancellationToken) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if cancel.is_cancelled() || !svc.state_snapshot().enabled {
            break;
        }
        match connect_once(&svc, &cancel).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                tracing::warn!(error = %e, "remote tunnel failed");
                svc.set_error(Some(e));
            }
        }
        svc.set_connected(false);
        svc.set_tunnel_tx(&cancel, None);
        svc.close_all_sessions("tunnel_lost");
        if cancel.is_cancelled() {
            break;
        }
        let jitter = rand::rng().random_range(0..=backoff.as_millis() as u64 / 4);
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(backoff + Duration::from_millis(jitter)) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
    svc.set_connected(false);
}

/// One tunnel connection; `Ok` when it ran and closed normally.
async fn connect_once(svc: &Arc<RemoteService>, cancel: &CancellationToken) -> Result<(), String> {
    let st = svc.state_snapshot();
    let gateway = st.gateway_url.clone().ok_or("no gateway configured")?;
    let cik = svc.cik().ok_or("not attached")?.clone();
    let (ws, _) = tokio_tungstenite::connect_async(tunnel_url(&gateway)).await.map_err(|e| format!("connect: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // challenge → hello → welcome
    let nonce = match next_ctl(&mut stream).await? {
        ToCore::Challenge { nonce } => e2e::unb64(&nonce).map_err(|e| e.to_string())?,
        other => return Err(format!("unexpected first frame {other:?}")),
    };
    let mut msg = HELLO_CONTEXT.as_bytes().to_vec();
    msg.extend_from_slice(&nonce);
    let sig: p256::ecdsa::Signature = cik.sign(&msg);
    let hello = ToGateway::Hello {
        core_id: st.core_id.clone(),
        cik_public: e2e::verifying_b64(cik.verifying_key()),
        enrollment_token: if st.core_id.is_none() { st.enrollment_token.clone() } else { None },
        version: svc.version(),
        capabilities: vec!["e2e-v1".into(), "push-v1".into()],
        sig: e2e::b64(&sig.to_bytes()),
    };
    sink.send(Message::Text(serde_json::to_string(&hello).map_err(|e| e.to_string())?.into()))
        .await
        .map_err(|e| e.to_string())?;
    match next_ctl(&mut stream).await? {
        ToCore::Welcome { core_id, account_id, core_token, rp_id, origins, vapid_public_key, native_push: _ } => {
            svc.update_state(|s| {
                if vapid_public_key.is_some() {
                    s.vapid_public_key = vapid_public_key;
                }
                s.core_id = Some(core_id);
                s.account_id = Some(account_id);
                if core_token.is_some() {
                    s.core_token = core_token;
                }
                s.enrollment_token = None;
                s.rp = Some(RelyingParty { rp_id, origins });
            });
        }
        ToCore::Error { code, message } => {
            if code == "bad_enrollment_token" {
                svc.update_state(|s| s.enrollment_token = None);
            }
            return Err(format!("gateway refused the tunnel: {code}: {message}"));
        }
        other => return Err(format!("unexpected frame {other:?}")),
    }

    let (tx, mut rx) = mpsc::channel::<Message>(1024);
    svc.set_tunnel_tx(cancel, Some(tx));
    svc.set_connected(true);
    svc.set_error(None);
    tracing::info!(gateway = %gateway, "remote tunnel connected");

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some(m) => if sink.send(m).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break },
            }
        }
        let _ = sink.close().await;
    });

    let result = loop {
        tokio::select! {
            () = cancel.cancelled() => break Ok(()),
            m = stream.next() => match m {
                None => break Err("gateway closed the tunnel".to_owned()),
                Some(Err(e)) => break Err(format!("tunnel read: {e}")),
                Some(Ok(Message::Close(_))) => break Err("gateway closed the tunnel".to_owned()),
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<ToCore>(t.as_str()) {
                    Ok(frame) => on_ctl(svc, frame).await,
                    Err(e) => tracing::warn!(error = %e, "bad tunnel control frame"),
                },
                Some(Ok(Message::Binary(b))) => on_data(svc, &b),
                Some(Ok(_)) => {}
            }
        }
    };
    writer.abort();
    result
}

async fn next_ctl<S>(stream: &mut S) -> Result<ToCore, String>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let fut = async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => {
                    return serde_json::from_str::<ToCore>(t.as_str()).map_err(|e| e.to_string())
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(other)) => return Err(format!("unexpected message {other:?}")),
                Some(Err(e)) => return Err(e.to_string()),
                None => return Err("tunnel closed during hello".into()),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), fut).await.map_err(|_| "tunnel hello timed out".to_owned())?
}

async fn on_ctl(svc: &Arc<RemoteService>, frame: ToCore) {
    match frame {
        ToCore::Open { sid, .. } => {
            let mut sessions = svc.sessions().lock();
            if sessions.len() >= MAX_STREAMS || sessions.contains_key(&sid) {
                drop(sessions);
                svc.send_ctl(&ToGateway::Close { sid, reason: "too_many_streams".into() });
                return;
            }
            let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
            let cancel = CancellationToken::new();
            sessions.insert(sid, SessionCtl { device_id: None, inbound: tx, cancel: cancel.clone() });
            drop(sessions);
            tokio::spawn(run_stream(Arc::clone(svc), sid, rx, cancel));
        }
        ToCore::Close { sid } => {
            if let Some(s) = svc.sessions().lock().remove(&sid) {
                s.cancel.cancel();
            }
        }
        ToCore::PairingClaim { pairing_id, device_pub_key, device_name, platform, code_mac } => {
            let ack = svc.handle_claim(&pairing_id, &device_pub_key, &device_name, &platform, &code_mac).await;
            svc.send_ctl(&ack);
        }
        ToCore::Challenge { .. } | ToCore::Welcome { .. } => {}
        ToCore::Error { code, message } => tracing::warn!(%code, %message, "gateway error"),
    }
}

fn on_data(svc: &Arc<RemoteService>, b: &[u8]) {
    if b.len() < 4 {
        return;
    }
    let sid = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let tx = svc.sessions().lock().get(&sid).map(|s| s.inbound.clone());
    if let Some(tx) = tx {
        if tx.try_send(b[4..].to_vec()).is_err() {
            tracing::warn!(sid, "remote stream inbound buffer full; closing stream");
            if let Some(s) = svc.sessions().lock().remove(&sid) {
                s.cancel.cancel();
            }
            svc.send_ctl(&ToGateway::Close { sid, reason: "overflow".into() });
        }
    }
}

fn refuse(svc: &RemoteService, sid: u32, code: &str) {
    svc.send_data(sid, &HandshakeFrame::Error { code: code.into() }.to_bytes());
    svc.send_ctl(&ToGateway::Close { sid, reason: code.into() });
}

/// One client stream: E2E handshake, then decrypt → RPC session → encrypt.
async fn run_stream(
    svc: Arc<RemoteService>,
    sid: u32,
    mut inbound: mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
) {
    let established = tokio::select! {
        () = cancel.cancelled() => None,
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&svc, sid, &mut inbound)) => match r {
            Ok(Ok(v)) => Some(v),
            Ok(Err((device, code))) => {
                svc.handshake_refused(device, sid, &code).await;
                refuse(&svc, sid, &code);
                None
            }
            Err(_) => { refuse(&svc, sid, "timeout"); None }
        },
    };
    let Some((session, principal)) = established else {
        svc.sessions().lock().remove(&sid);
        return;
    };
    let Principal::Remote(rp) = &principal else { return };
    let device = rp.device_id;
    if let Some(s) = svc.sessions().lock().get_mut(&sid) {
        s.device_id = Some(device);
    }
    svc.session_opened(device, sid).await;
    let e2e::Session { mut sealer, mut opener, .. } = session;
    let (in_tx, in_rx) = mpsc::channel::<String>(256);
    let (out_tx, mut out_rx) = mpsc::channel::<String>(512);
    let Some(runner) = svc.runner() else { return };
    let rpc = tokio::spawn(runner(principal, in_rx, out_tx));
    let reason = loop {
        tokio::select! {
            () = cancel.cancelled() => break "closed",
            m = inbound.recv() => {
                let Some(rec) = m else { break "closed" };
                match opener.open(&rec).map(String::from_utf8) {
                    Ok(Ok(text)) => if in_tx.send(text).await.is_err() { break "session_ended" },
                    Ok(Err(_)) => break "malformed",
                    Err(e) => { tracing::warn!(sid, error = %e, "E2E record rejected"); break "e2e_error" }
                }
            }
            out = out_rx.recv() => {
                let Some(text) = out else { break "session_ended" };
                match sealer.seal(text.as_bytes()) {
                    Ok(rec) => svc.send_data(sid, &rec),
                    Err(_) => break "rekey_required",
                }
            }
        }
    };
    drop(in_tx);
    rpc.abort();
    if svc.sessions().lock().remove(&sid).is_some() {
        svc.send_ctl(&ToGateway::Close { sid, reason: reason.into() });
    }
    tracing::debug!(sid, %device, reason, "remote session closed");
}

type HandshakeResult = Result<(e2e::Session, Principal), (Option<DeviceId>, String)>;

async fn handshake(svc: &RemoteService, sid: u32, inbound: &mut mpsc::Receiver<Vec<u8>>) -> HandshakeResult {
    let err = |d: Option<DeviceId>, c: &str| (d, c.to_owned());
    let first = inbound.recv().await.ok_or_else(|| err(None, "closed"))?;
    let frame = HandshakeFrame::from_bytes(&first).map_err(|_| err(None, "malformed"))?;
    let hello = ClientHelloData::parse(&frame, RemoteService::now_ms()).map_err(|e| match e {
        e2e::E2eError::Stale => err(None, "stale"),
        _ => err(None, "malformed"),
    })?;
    let device_id = DeviceId::parse(&hello.device_id).map_err(|_| err(None, "unknown_device"))?;
    let d = Some(device_id);
    if svc.core_id().as_deref() != Some(hello.core_id.as_str()) {
        return Err(err(d, "wrong_core"));
    }
    if !svc.state_snapshot().enabled {
        return Err(err(d, "disabled"));
    }
    let record = svc.device(device_id).ok_or_else(|| err(d, "unknown_device"))?;
    if record.revoked {
        return Err(err(d, "revoked"));
    }
    if !svc.check_replay(&hello.device_id, &hello.nonce) {
        return Err(err(d, "replay"));
    }
    let device_key = e2e::parse_verifying_b64(&record.public_key).map_err(|_| err(d, "unknown_device"))?;
    let cik = svc.cik().ok_or_else(|| err(d, "disabled"))?;
    let (pending, server_hello) = CoreHandshake::respond(&hello, cik).map_err(|_| err(d, "malformed"))?;
    svc.send_data(sid, &server_hello.to_bytes());
    let fin = inbound.recv().await.ok_or_else(|| err(d, "closed"))?;
    let fin = HandshakeFrame::from_bytes(&fin).map_err(|_| err(d, "malformed"))?;
    let session = pending.complete(&fin, &device_key).map_err(|_| err(d, "bad_signature"))?;
    // re-check: the device may have been revoked during the handshake
    if svc.device(device_id).is_none_or(|r| r.revoked) {
        return Err(err(d, "revoked"));
    }
    let principal = Principal::Remote(RemotePrincipal {
        device_id,
        role: record.role,
        remote_terminal: record.remote_terminal,
        project_ids: record.project_ids.unwrap_or_default(),
    });
    Ok((session, principal))
}
