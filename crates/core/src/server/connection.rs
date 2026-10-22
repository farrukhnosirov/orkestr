//! One RPC session: auth handshake, request dispatch, subscriptions.
//!
//! The same session logic serves the local WebSocket ([`run`]) and remote E2E
//! streams ([`run_channel`], frames already decrypted by `remote::tunnel`).

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures::{SinkExt, StreamExt};
use orkestr_domain::{CorrelationId, EventId, SessionId};
use orkestr_events::{EventBus, Received, Subscription};
use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
use orkestr_protocol::frames::{AuthResult, ClientFrame, ResFrame, ServerFrame, SubResult};
use orkestr_protocol::{Event, RpcError, PROTOCOL_VERSION};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot, Semaphore};

use super::AppState;
use crate::auth::token_matches;
use crate::rpc::{Principal, RemotePrincipal, RpcContext};

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INFLIGHT_REQUESTS: usize = 64;
const OUTBOUND_BUFFER: usize = 512;
const MAX_ID_LEN: usize = 128;
const MAX_TOPICS: usize = 256;

/// Close codes (4000–4999 are application defined).
const CLOSE_UNAUTHORIZED: u16 = 4401;
const CLOSE_GOING_AWAY: u16 = 1001;

enum Outgoing {
    Frame(Box<ServerFrame>),
    Close(u16, &'static str),
}

enum SubCommand {
    Add(Vec<String>, oneshot::Sender<Result<Vec<String>, String>>),
    Remove(Vec<String>, oneshot::Sender<Result<Vec<String>, String>>),
}

fn res(frame: ResFrame) -> Outgoing {
    Outgoing::Frame(Box::new(ServerFrame::Res(frame)))
}

/// Extracts `"id"` from a frame that failed to parse, for a helpful error.
fn salvage_id(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(|s| s.chars().take(MAX_ID_LEN).collect()))
        .unwrap_or_default()
}

/// Request ids currently being handled on one connection.
type InflightIds = Arc<Mutex<HashSet<String>>>;

pub(super) async fn run(socket: WebSocket, state: AppState, header_authed: bool, peer: Option<IpAddr>) {
    let session_id = SessionId::new();
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Outgoing>(OUTBOUND_BUFFER);

    let writer = tokio::spawn(async move {
        while let Some(out) = out_rx.recv().await {
            match out {
                Outgoing::Frame(frame) => {
                    let Ok(text) = serde_json::to_string(&frame) else {
                        tracing::error!("failed to serialize server frame");
                        continue;
                    };
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Outgoing::Close(code, reason) => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                    break;
                }
            }
        }
        let _ = sink.close().await;
    });

    let authed = header_authed || authenticate(&mut stream, &state, &out_tx, session_id, peer).await;
    if !authed {
        drop(out_tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
        return;
    }
    let incoming = stream
        .take_while(|m| futures::future::ready(matches!(m, Ok(msg) if !matches!(msg, Message::Close(_)))))
        .filter_map(|m| {
            futures::future::ready(match m {
                Ok(Message::Text(t)) => Some(Inbound::Text(t.to_string())),
                Ok(Message::Binary(_)) => Some(Inbound::Binary),
                _ => None, // ping/pong handled by axum
            })
        });
    serve(Box::pin(incoming), &state, out_tx, Principal::Local, session_id, peer).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

/// Runs an RPC session over text channels for `principal` (remote E2E
/// streams). The client's `auth` frame is accepted and its token ignored —
/// the principal was established by the E2E handshake (ADR-0030).
pub(crate) async fn run_channel(
    state: AppState,
    principal: Principal,
    mut incoming: mpsc::Receiver<String>,
    outgoing: mpsc::Sender<String>,
) {
    let session_id = SessionId::new();
    let (out_tx, mut out_rx) = mpsc::channel::<Outgoing>(OUTBOUND_BUFFER);
    let writer = tokio::spawn(async move {
        while let Some(out) = out_rx.recv().await {
            match out {
                Outgoing::Frame(frame) => {
                    let Ok(text) = serde_json::to_string(&frame) else { continue };
                    if outgoing.send(text).await.is_err() {
                        break;
                    }
                }
                Outgoing::Close(..) => break,
            }
        }
    });
    let stream = futures::stream::poll_fn(move |cx| incoming.poll_recv(cx).map(|o| o.map(Inbound::Text)));
    serve(Box::pin(stream), &state, out_tx, principal, session_id, None).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

/// An inbound transport message.
enum Inbound {
    Text(String),
    Binary,
}

async fn serve(
    mut incoming: std::pin::Pin<Box<dyn futures::Stream<Item = Inbound> + Send + '_>>,
    state: &AppState,
    out_tx: mpsc::Sender<Outgoing>,
    principal: Principal,
    session_id: SessionId,
    peer: Option<IpAddr>,
) {
    tracing::debug!(%session_id, remote = !principal.is_local(), "rpc session opened");
    let remote = match &principal {
        Principal::Remote(rp) => Some(rp.clone()),
        Principal::Local => None,
    };
    let (sub_tx, sub_rx) = mpsc::channel::<SubCommand>(32);
    let forwarder = tokio::spawn(forward_events(state.services.events.clone(), sub_rx, out_tx.clone(), remote));
    let inflight = Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS));
    let inflight_ids: InflightIds = Arc::new(Mutex::new(HashSet::new()));
    let conn = Conn {
        state,
        out: &out_tx,
        subs: &sub_tx,
        inflight: (&inflight, &inflight_ids),
        session_id,
        peer,
        principal: &principal,
    };

    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => {
                let _ = out_tx.send(Outgoing::Close(CLOSE_GOING_AWAY, "core shutting down")).await;
                break;
            }
            msg = incoming.next() => match msg {
                None => break,
                Some(Inbound::Text(text)) => {
                    if !handle_text(text.as_str(), &conn).await {
                        break;
                    }
                }
                Some(Inbound::Binary) => {
                    let _ = out_tx.send(res(ResFrame::err("", RpcError::bad_request("binary frames are not supported")))).await;
                }
            }
        }
    }

    forwarder.abort();
    state.services.files.unwatch_all(&session_id.to_string());
    drop(sub_tx);
    drop(out_tx);
    tracing::debug!(%session_id, "rpc session closed");
}

/// Per-connection handles for [`handle_text`].
struct Conn<'a> {
    state: &'a AppState,
    out: &'a mpsc::Sender<Outgoing>,
    subs: &'a mpsc::Sender<SubCommand>,
    inflight: (&'a Arc<Semaphore>, &'a InflightIds),
    session_id: SessionId,
    peer: Option<IpAddr>,
    principal: &'a Principal,
}

fn auth_result(state: &AppState, session_id: SessionId) -> serde_json::Value {
    serde_json::to_value(AuthResult {
        session_id,
        server_version: state.services.version.clone(),
        protocol_version: PROTOCOL_VERSION,
        last_seq: state.services.events.last_seq(),
    })
    .unwrap_or(serde_json::Value::Null)
}

/// Waits for the first frame and validates it as `auth`. Sends the response.
async fn authenticate(
    stream: &mut futures::stream::SplitStream<WebSocket>,
    state: &AppState,
    out: &mpsc::Sender<Outgoing>,
    session_id: SessionId,
    peer: Option<IpAddr>,
) -> bool {
    let first = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => return Some(t.to_string()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                _ => return None,
            }
        }
    })
    .await;
    let (id, ok) = match first {
        Ok(Some(text)) => match serde_json::from_str::<ClientFrame>(&text) {
            Ok(ClientFrame::Auth { token, id, .. }) => (id, token_matches(&state.token, &token)),
            Ok(_) | Err(_) => (salvage_id(&text), false),
        },
        _ => ("auth".to_owned(), false),
    };
    if ok {
        if let Some(ip) = peer {
            state.limiter.record_success(ip);
        }
        let _ = out.send(res(ResFrame::ok(id, auth_result(state, session_id)))).await;
        true
    } else {
        tracing::warn!("rejected rpc session: missing or invalid auth frame");
        if let Some(ip) = peer {
            state.limiter.record_failure(ip);
        }
        let _ = out
            .send(res(ResFrame::err(
                if id.is_empty() { "auth".to_owned() } else { id },
                RpcError::unauthorized("authenticate first: {\"type\":\"auth\",\"token\":\"…\"}"),
            )))
            .await;
        let _ = out.send(Outgoing::Close(CLOSE_UNAUTHORIZED, "unauthorized")).await;
        false
    }
}

/// Handles one text frame. Returns `false` when the connection must close.
async fn handle_text(text: &str, conn: &Conn<'_>) -> bool {
    let Conn { state, out, subs, inflight: (inflight, inflight_ids), session_id, peer, principal } = *conn;
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(f) => f,
        Err(e) => {
            let _ = out
                .send(res(ResFrame::err(salvage_id(text), RpcError::bad_request(format!("invalid frame: {e}")))))
                .await;
            return true;
        }
    };
    match frame {
        ClientFrame::Auth { token, id, .. } => {
            // Remote sessions: the E2E handshake authenticated the device; the token is ignored.
            if !principal.is_local() || token_matches(&state.token, &token) {
                let _ = out.send(res(ResFrame::ok(id, auth_result(state, session_id)))).await;
                true
            } else {
                if let Some(ip) = peer {
                    state.limiter.record_failure(ip);
                }
                let _ = out.send(res(ResFrame::err(id, RpcError::unauthorized("invalid token")))).await;
                let _ = out.send(Outgoing::Close(CLOSE_UNAUTHORIZED, "unauthorized")).await;
                false
            }
        }
        ClientFrame::Req { id, method, params } => {
            if id.is_empty() || id.len() > MAX_ID_LEN {
                let _ =
                    out.send(res(ResFrame::err(id, RpcError::bad_request("request id must be 1..=128 chars")))).await;
                return true;
            }
            if !inflight_ids.lock().insert(id.clone()) {
                let _ = out
                    .send(res(ResFrame::err(id, RpcError::conflict("a request with this id is already in flight"))))
                    .await;
                return true;
            }
            let Ok(permit) = Arc::clone(inflight).acquire_owned().await else { return false };
            let ids = Arc::clone(inflight_ids);
            let ctx = RpcContext {
                services: Arc::clone(&state.services),
                session_id,
                correlation_id: CorrelationId::new(),
                principal: principal.clone(),
            };
            let router = Arc::clone(&state.router);
            let out = out.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let started = std::time::Instant::now();
                let task = tokio::spawn({
                    let method = method.clone();
                    async move { router.dispatch(ctx, &method, params).await }
                });
                let frame = match task.await {
                    Ok(Ok(result)) => ResFrame::ok(id, result),
                    Ok(Err(err)) => ResFrame::err(id, err),
                    Err(join) => {
                        tracing::error!(%method, error = %join, "rpc handler panicked");
                        ResFrame::err(id, RpcError::internal("handler failed"))
                    }
                };
                tracing::debug!(%method, ok = frame.ok, elapsed_ms = started.elapsed().as_millis() as u64, "rpc");
                ids.lock().remove(&frame.id);
                let _ = out.send(res(frame)).await;
            });
            true
        }
        ClientFrame::Sub { id, topics } | ClientFrame::Unsub { id, topics } if topics.len() > MAX_TOPICS => {
            let _ = out.send(res(ResFrame::err(id, RpcError::bad_request("too many topics")))).await;
            true
        }
        ClientFrame::Sub { id, topics } => {
            if let Principal::Remote(rp) = principal {
                if let Some(bad) = topics.iter().find(|t| !crate::remote::authz::topic_allowed(rp, t)) {
                    let err = RpcError::forbidden(format!("topic `{bad}` is not allowed for this device"));
                    let _ = out.send(res(ResFrame::err(id, err))).await;
                    return true;
                }
            }
            sub_command(out, subs, id, |tx| SubCommand::Add(topics, tx)).await;
            true
        }
        ClientFrame::Unsub { id, topics } => {
            sub_command(out, subs, id, |tx| SubCommand::Remove(topics, tx)).await;
            true
        }
    }
}

async fn sub_command(
    out: &mpsc::Sender<Outgoing>,
    subs: &mpsc::Sender<SubCommand>,
    id: String,
    make: impl FnOnce(oneshot::Sender<Result<Vec<String>, String>>) -> SubCommand,
) {
    let (tx, rx) = oneshot::channel();
    let frame = if subs.send(make(tx)).await.is_err() {
        ResFrame::err(id, RpcError::internal("subscription task stopped"))
    } else {
        match rx.await {
            Ok(Ok(topics)) => ResFrame::ok(id, serde_json::to_value(SubResult { topics }).unwrap_or_default()),
            Ok(Err(msg)) => ResFrame::err(id, RpcError::bad_request(msg)),
            Err(_) => ResFrame::err(id, RpcError::internal("subscription task stopped")),
        }
    };
    let _ = out.send(res(frame)).await;
}

fn lag_notice(dropped: u64) -> ServerFrame {
    let event = Event {
        seq: 0,
        id: EventId::new(),
        ts: chrono::Utc::now(),
        project_id: None,
        task_id: None,
        agent_id: None,
        node_id: None,
        correlation_id: None,
        kind: EventKind::Log(LogPayload {
            level: LogLevel::Warn,
            target: "orkestr_core::rpc".into(),
            message: format!("event stream lagged: {dropped} events were dropped; call events.replay to resync"),
            fields: serde_json::json!({ "dropped": dropped, "local": true }),
        }),
    };
    ServerFrame::Event { seq: 0, event: Box::new(event) }
}

/// Owns the connection's subscription; applies topic changes and forwards events.
async fn forward_events(
    bus: EventBus,
    mut cmds: mpsc::Receiver<SubCommand>,
    out: mpsc::Sender<Outgoing>,
    remote: Option<RemotePrincipal>,
) {
    let mut sub: Option<Subscription> = None;
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    SubCommand::Add(topics, reply) => {
                        let result = match sub.as_mut() {
                            Some(s) => s.add_topics(topics).map(|()| s.topics()),
                            None => bus.subscribe(topics).map(|s| {
                                let t = s.topics();
                                sub = Some(s);
                                t
                            }),
                        };
                        let _ = reply.send(result.map_err(|e| e.to_string()));
                    }
                    SubCommand::Remove(topics, reply) => {
                        let remaining = match sub.as_mut() {
                            Some(s) => {
                                s.remove_topics(&topics);
                                s.topics()
                            }
                            None => Vec::new(),
                        };
                        if remaining.is_empty() {
                            sub = None;
                        }
                        let _ = reply.send(Ok(remaining));
                    }
                }
            }
            received = async { sub.as_mut().expect("guarded by precondition").recv().await }, if sub.is_some() => {
                match received {
                    None => break,
                    Some(Received::Event(event)) => {
                        if remote.as_ref().is_some_and(|rp| !crate::remote::authz::event_visible(rp, &event)) {
                            continue;
                        }
                        let frame = ServerFrame::Event { seq: event.seq, event: Box::new((*event).clone()) };
                        if out.send(Outgoing::Frame(Box::new(frame))).await.is_err() {
                            break;
                        }
                    }
                    Some(Received::Lagged(n)) => {
                        tracing::warn!(dropped = n, "rpc subscriber lagged");
                        if out.send(Outgoing::Frame(Box::new(lag_notice(n)))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Session runner for `remote::RemoteService` (holds the services weakly so
/// the service graph has no reference cycle).
pub(crate) fn remote_runner(state: &AppState) -> crate::remote::SessionRunner {
    let services = Arc::downgrade(&state.services);
    let token = Arc::clone(&state.token);
    let router = Arc::clone(&state.router);
    let shutdown = state.shutdown.clone();
    let limiter = Arc::clone(&state.limiter);
    let port = state.port;
    Arc::new(move |principal, incoming, outgoing| {
        let services = services.upgrade();
        let token = Arc::clone(&token);
        let router = Arc::clone(&router);
        let shutdown = shutdown.clone();
        let limiter = Arc::clone(&limiter);
        Box::pin(async move {
            let Some(services) = services else { return };
            let state = AppState { services, token, router, shutdown, port, limiter };
            run_channel(state, principal, incoming, outgoing).await;
        })
    })
}
