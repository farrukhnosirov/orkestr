//! One WebSocket connection: auth handshake, request dispatch, subscriptions.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures::{SinkExt, StreamExt};
use orkestr_domain::{CorrelationId, EventId, SessionId};
use orkestr_events::{EventBus, Received, Subscription};
use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
use orkestr_protocol::frames::{AuthResult, ClientFrame, ResFrame, ServerFrame, SubResult};
use orkestr_protocol::{Event, RpcError, PROTOCOL_VERSION};
use tokio::sync::{mpsc, oneshot, Semaphore};

use super::AppState;
use crate::auth::token_matches;
use crate::rpc::RpcContext;

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

pub(super) async fn run(socket: WebSocket, state: AppState, header_authed: bool) {
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

    let authed = header_authed || authenticate(&mut stream, &state, &out_tx, session_id).await;
    if !authed {
        drop(out_tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
        return;
    }
    tracing::debug!(%session_id, "rpc session opened");

    let (sub_tx, sub_rx) = mpsc::channel::<SubCommand>(32);
    let forwarder = tokio::spawn(forward_events(state.services.events.clone(), sub_rx, out_tx.clone()));
    let inflight = Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS));

    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => {
                let _ = out_tx.send(Outgoing::Close(CLOSE_GOING_AWAY, "core shutting down")).await;
                break;
            }
            msg = stream.next() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    let keep_open = handle_text(text.as_str(), &state, &out_tx, &sub_tx, &inflight, session_id).await;
                    if !keep_open {
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) => {
                    let _ = out_tx.send(res(ResFrame::err("", RpcError::bad_request("binary frames are not supported")))).await;
                }
                Some(Ok(_)) => {} // ping/pong handled by axum
            }
        }
    }

    forwarder.abort();
    state.services.files.unwatch_all(&session_id.to_string());
    drop(sub_tx);
    drop(out_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    tracing::debug!(%session_id, "rpc session closed");
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
        let _ = out.send(res(ResFrame::ok(id, auth_result(state, session_id)))).await;
        true
    } else {
        tracing::warn!("rejected rpc session: missing or invalid auth frame");
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
async fn handle_text(
    text: &str,
    state: &AppState,
    out: &mpsc::Sender<Outgoing>,
    subs: &mpsc::Sender<SubCommand>,
    inflight: &Arc<Semaphore>,
    session_id: SessionId,
) -> bool {
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
            if token_matches(&state.token, &token) {
                let _ = out.send(res(ResFrame::ok(id, auth_result(state, session_id)))).await;
                true
            } else {
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
            let Ok(permit) = Arc::clone(inflight).acquire_owned().await else { return false };
            let ctx =
                RpcContext { services: Arc::clone(&state.services), session_id, correlation_id: CorrelationId::new() };
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
                let _ = out.send(res(frame)).await;
            });
            true
        }
        ClientFrame::Sub { id, topics } | ClientFrame::Unsub { id, topics } if topics.len() > MAX_TOPICS => {
            let _ = out.send(res(ResFrame::err(id, RpcError::bad_request("too many topics")))).await;
            true
        }
        ClientFrame::Sub { id, topics } => {
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
async fn forward_events(bus: EventBus, mut cmds: mpsc::Receiver<SubCommand>, out: mpsc::Sender<Outgoing>) {
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
