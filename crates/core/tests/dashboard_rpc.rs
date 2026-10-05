//! End-to-end (real WebSocket) tests of `dashboard.*` (Mission Control, Monitoring).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::{AutostartConfig, CoreConfig};
use orkestr_platform::MemoryAutostart;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn config(dir: &std::path::Path, autostart: Arc<MemoryAutostart>) -> CoreConfig {
    CoreConfig {
        autostart: AutostartConfig {
            id: "dev.orkestr.test".into(),
            args: vec!["--autostart".into()],
            backend: Some(autostart),
            ..AutostartConfig::default()
        },
        node_sample_interval: Duration::from_millis(300),
        node_info_prefetch_after: None,
        ..CoreConfig::with_data_dir(dir)
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws, timeout: Duration) -> Value {
    loop {
        let msg = tokio::time::timeout(timeout, ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

async fn connect(core: &orkestr_core::CoreHandle) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": core.token})).await;
    let res = recv(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(res["ok"], true, "{res}");
    ws
}

/// Sends a request and waits for its response, collecting events meanwhile.
async fn call(ws: &mut Ws, id: &str, method: &str, params: Value, events: &mut Vec<Value>) -> Value {
    send(ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
    loop {
        let v = recv(ws, Duration::from_secs(60)).await;
        match v["type"].as_str() {
            Some("res") if v["id"] == id => return v,
            Some("event") => events.push(v),
            _ => {}
        }
    }
}

#[allow(dead_code)]
async fn subscribe(ws: &mut Ws, id: &str, topics: &[&str]) {
    send(ws, json!({"type": "sub", "id": id, "topics": topics})).await;
    loop {
        let v = recv(ws, Duration::from_secs(5)).await;
        if v["type"] == "res" && v["id"] == id {
            assert_eq!(v["ok"], true, "{v}");
            return;
        }
    }
}

async fn ok(ws: &mut Ws, id: &str, method: &str, params: Value, events: &mut Vec<Value>) -> Value {
    let res = call(ws, id, method, params, events).await;
    assert_eq!(res["ok"], true, "{method}: {res}");
    res["result"].clone()
}

#[tokio::test]
async fn dashboard_snapshot_and_health() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(config(dir.path(), Arc::new(MemoryAutostart::default()))).await.unwrap();
    let mut ws = connect(&core).await;
    let mut events = Vec::new();

    let empty = ok(&mut ws, "d0", "dashboard.snapshot", json!({}), &mut events).await;
    assert_eq!(empty["totals"]["projects"], 0);
    assert_eq!(empty["emergencyStopped"], false);
    assert!(empty["providers"].is_array());

    let created = ok(&mut ws, "p1", "projects.create", json!({"name": "Schoolify"}), &mut events).await;
    let pid = created["project"]["id"].as_str().unwrap().to_owned();
    ok(&mut ws, "t1", "tasks.create", json!({"projectId": pid, "title": "Build API"}), &mut events).await;

    let snap = ok(&mut ws, "d1", "dashboard.snapshot", json!({}), &mut events).await;
    assert_eq!(snap["totals"]["projects"], 1);
    let row = &snap["projects"][0];
    assert_eq!(row["projectId"], pid.as_str());
    assert_eq!(row["name"], "Schoolify");
    assert_eq!(row["openTasks"], 1);
    assert_eq!(row["activity"], "Idle");
    assert!(snap["spend"]["todayUsd"].as_f64().is_some());

    let health = ok(&mut ws, "h1", "dashboard.health", json!({}), &mut events).await;
    assert_eq!(health["pid"], std::process::id());
    assert!(health["storage"]["dbBytes"].as_u64().unwrap() > 0);
    assert!(health["rpcMethods"].as_u64().unwrap() > 100);
    assert!(health["lastEventSeq"].as_u64().unwrap() >= 2);
    assert!(health["coreProcess"]["memoryBytes"].as_u64().unwrap() > 0);
    assert!(health["processes"].is_array());
    core.shutdown().await;
}
