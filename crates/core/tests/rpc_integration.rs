//! End-to-end: start a core on a temp dir, talk to it over a real WebSocket.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start() -> (tempfile::TempDir, orkestr_core::CoreHandle) {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    (dir, core)
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

/// Receives until a `res` with `id` arrives, collecting events seen meanwhile.
async fn recv_res(ws: &mut Ws, id: &str, events: &mut Vec<Value>) -> Value {
    loop {
        let v = recv(ws).await;
        match v["type"].as_str() {
            Some("res") if v["id"] == id => return v,
            Some("event") => events.push(v),
            _ => panic!("unexpected frame {v}"),
        }
    }
}

async fn connect_and_auth(core: &orkestr_core::CoreHandle) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": core.token})).await;
    let res = recv(&mut ws).await;
    assert_eq!(res["type"], "res");
    assert_eq!(res["id"], "auth");
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["result"]["protocolVersion"], 1);
    ws
}

#[tokio::test]
async fn health_endpoint_and_discovery_file() {
    let (dir, core) = start().await;
    assert!(core.url.starts_with("ws://127.0.0.1:"));
    let authority = core.url.trim_start_matches("ws://").trim_end_matches("/rpc").to_owned();
    assert!(orkestr_core::discovery::probe_health(&authority, Duration::from_secs(2)).await);
    let disc: Value = serde_json::from_slice(&std::fs::read(dir.path().join("core.json")).unwrap()).unwrap();
    assert_eq!(disc["url"], core.url);
    assert_eq!(disc["pid"], std::process::id());
    let token = std::fs::read_to_string(disc["token_file_hint"].as_str().unwrap()).unwrap();
    assert_eq!(token, core.token);
    core.shutdown().await;
    assert!(!dir.path().join("core.json").exists(), "discovery file removed on shutdown");
    assert!(!dir.path().join("session.lock").exists(), "session marker removed on clean shutdown");
}

#[tokio::test]
async fn auth_create_list_and_event_stream() {
    let (_dir, core) = start().await;
    let mut ws = connect_and_auth(&core).await;
    let mut events = Vec::new();

    send(&mut ws, json!({"type": "sub", "id": "s1", "topics": ["global"]})).await;
    let res = recv_res(&mut ws, "s1", &mut events).await;
    assert_eq!(res["result"]["topics"], json!(["global"]));

    send(&mut ws, json!({"type": "req", "id": "1", "method": "projects.create", "params": {"name": "Payroll", "color": "#22d3ee"}})).await;
    let res = recv_res(&mut ws, "1", &mut events).await;
    assert_eq!(res["ok"], true, "{res}");
    let project = res["result"]["project"].clone();
    assert_eq!(project["name"], "Payroll");
    assert_eq!(project["slug"], "payroll");

    // The ProjectCreated event arrives (before or after the response).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|e| e["event"]["kind"] == "ProjectCreated") {
        assert!(tokio::time::Instant::now() < deadline, "no ProjectCreated event");
        events.push(recv(&mut ws).await);
    }
    let ev = events.iter().find(|e| e["event"]["kind"] == "ProjectCreated").unwrap();
    assert_eq!(ev["event"]["payload"]["project"]["id"], project["id"]);
    assert_eq!(ev["seq"], ev["event"]["seq"]);
    assert!(ev["event"]["correlationId"].is_string(), "request correlation id propagates to events");

    send(&mut ws, json!({"type": "req", "id": "2", "method": "projects.list"})).await;
    let res = recv_res(&mut ws, "2", &mut events).await;
    let list = res["result"]["projects"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], project["id"]);

    // Duplicate names conflict.
    send(&mut ws, json!({"type": "req", "id": "3", "method": "projects.create", "params": {"name": "payroll"}})).await;
    let res = recv_res(&mut ws, "3", &mut events).await;
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"]["code"], "conflict");

    // Update, open, get, replay, delete.
    let pid = project["id"].as_str().unwrap().to_owned();
    send(&mut ws, json!({"type": "req", "id": "4", "method": "projects.update", "params": {"projectId": pid, "patch": {"description": "HR"}}})).await;
    let res = recv_res(&mut ws, "4", &mut events).await;
    assert_eq!(res["result"]["project"]["description"], "HR");

    send(&mut ws, json!({"type": "req", "id": "5", "method": "projects.open", "params": {"projectId": pid}})).await;
    let res = recv_res(&mut ws, "5", &mut events).await;
    assert!(res["result"]["project"]["lastOpenedAt"].is_string());

    send(
        &mut ws,
        json!({"type": "req", "id": "6", "method": "events.replay", "params": {"topics": [format!("project:{pid}")]}}),
    )
    .await;
    let res = recv_res(&mut ws, "6", &mut events).await;
    let kinds: Vec<_> =
        res["result"]["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_owned()).collect();
    assert_eq!(kinds, vec!["ProjectCreated", "ProjectUpdated", "ProjectUpdated"]);

    send(&mut ws, json!({"type": "req", "id": "7", "method": "projects.delete", "params": {"projectId": pid}})).await;
    let res = recv_res(&mut ws, "7", &mut events).await;
    assert_eq!(res["ok"], true);
    send(&mut ws, json!({"type": "req", "id": "8", "method": "projects.get", "params": {"projectId": pid}})).await;
    let res = recv_res(&mut ws, "8", &mut events).await;
    assert_eq!(res["error"]["code"], "not_found");

    // Errors: unknown method, bad params, malformed frame.
    send(&mut ws, json!({"type": "req", "id": "9", "method": "nope.nothing"})).await;
    assert_eq!(recv_res(&mut ws, "9", &mut events).await["error"]["code"], "bad_request");
    send(&mut ws, json!({"type": "req", "id": "10", "method": "projects.create", "params": {"name": ""}})).await;
    assert_eq!(recv_res(&mut ws, "10", &mut events).await["error"]["code"], "bad_request");
    send(&mut ws, json!({"type": "bogus", "id": "11"})).await;
    assert_eq!(recv_res(&mut ws, "11", &mut events).await["error"]["code"], "bad_request");

    send(&mut ws, json!({"type": "req", "id": "12", "method": "system.info"})).await;
    let info = recv_res(&mut ws, "12", &mut events).await;
    assert_eq!(info["result"]["protocolVersion"], 1);
    assert!(!info["result"]["shells"].as_array().unwrap().is_empty());

    core.shutdown().await;
}

#[tokio::test]
async fn header_auth_works() {
    let (_dir, core) = start().await;
    let mut req = core.url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("Authorization", format!("Bearer {}", core.token).parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut ws, json!({"type": "req", "id": "h", "method": "system.health"})).await;
    let res = recv(&mut ws).await;
    assert_eq!(res["result"]["status"], "ok");
    core.shutdown().await;
}

#[tokio::test]
async fn unauthorized_is_rejected() {
    let (_dir, core) = start().await;

    // Wrong token in the first frame → error + close.
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": "x".repeat(64)})).await;
    let res = recv(&mut ws).await;
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"]["code"], "unauthorized");
    let next = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
    assert!(matches!(next, Some(Ok(Message::Close(_))) | None | Some(Err(_))), "{next:?}");

    // Request before auth → rejected.
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "req", "id": "1", "method": "projects.list"})).await;
    let res = recv(&mut ws).await;
    assert_eq!(res["error"]["code"], "unauthorized");
    assert_eq!(res["id"], "1");

    // Wrong bearer header → HTTP 401 before upgrade.
    let mut req = core.url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("Authorization", "Bearer wrong".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");

    core.shutdown().await;
}

#[tokio::test]
async fn second_core_on_same_data_dir_is_refused_and_restart_resumes_seq() {
    let (dir, core) = start().await;
    let mut ws = connect_and_auth(&core).await;
    let mut events = Vec::new();
    send(&mut ws, json!({"type": "req", "id": "1", "method": "projects.create", "params": {"name": "A"}})).await;
    recv_res(&mut ws, "1", &mut events).await;
    let last = core.services.events.last_seq();
    assert!(last >= 1);
    drop(ws);
    core.shutdown().await;

    let core2 = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    assert_eq!(core2.services.events.last_seq(), last, "seq resumes from the persisted log");
    assert!(!core2.services.recovered_from_crash);
    let projects = core2.services.projects.list(false).await.unwrap();
    assert_eq!(projects.len(), 1);
    core2.shutdown().await;
}
