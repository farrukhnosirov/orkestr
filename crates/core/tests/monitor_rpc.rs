//! End-to-end (real WebSocket) tests of `monitor.*` (ADR-0042, WP5).

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(30), ws.next()).await.expect("timeout").expect("closed").unwrap();
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
    let res = recv(&mut ws).await;
    assert_eq!(res["ok"], true, "{res}");
    ws
}

async fn call(ws: &mut Ws, id: &str, method: &str, params: Value) -> Value {
    send(ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
    loop {
        let v = recv(ws).await;
        if v["type"] == "res" && v["id"] == id {
            return v;
        }
    }
}

async fn ok(ws: &mut Ws, id: &str, method: &str, params: Value) -> Value {
    let res = call(ws, id, method, params).await;
    assert_eq!(res["ok"], true, "{method}: {res}");
    res["result"].clone()
}

#[tokio::test]
async fn monitor_rpc_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;

    // Overview: the local node appears once the sampler has run.
    let mut nodes = 0;
    for i in 0..100 {
        let ov = ok(&mut ws, &format!("ov{i}"), "monitor.overview", json!({})).await;
        nodes = ov["nodes"].as_array().unwrap().len();
        if nodes > 0 {
            assert_eq!(ov["nodes"][0]["local"], true);
            assert!(ov["nodes"][0]["memTotalBytes"].as_u64().unwrap() > 0);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(nodes > 0, "local node never sampled");
    let m = ok(&mut ws, "m", "monitor.metrics", json!({"metrics": ["cpu"]})).await;
    assert_eq!(m["series"][0]["metric"], "cpu");

    // Ingest is redacted before storage.
    let r = ok(
        &mut ws,
        "in",
        "monitor.ingest",
        json!({"stream": "web", "lines": [
            {"message": "boot password=hunter2hunter2", "level": "error"},
            {"message": "user login", "attrs": {"token": "abc123abc123", "user": "bob"}}
        ]}),
    )
    .await;
    assert_eq!(r["accepted"], 2);
    let logs = ok(&mut ws, "lg", "monitor.logs", json!({"sourceId": "ingest:web"})).await;
    let text = logs["lines"].to_string();
    assert!(!text.contains("hunter2hunter2") && !text.contains("abc123abc123"), "{text}");
    assert!(text.contains("user=bob"));
    let errs = ok(&mut ws, "le", "monitor.logs", json!({"minLevel": "error"})).await;
    assert_eq!(errs["lines"].as_array().unwrap().len(), 1);
    let sources = ok(&mut ws, "ls", "monitor.logSources", json!({})).await;
    assert_eq!(sources["sources"][0]["id"], "ingest:web");
    let bad = call(&mut ws, "inb", "monitor.ingest", json!({"stream": "no spaces", "lines": []})).await;
    assert_eq!(bad["ok"], false);

    // Checks: TCP probe to a closed port goes Down after the threshold.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let c = ok(
        &mut ws,
        "cu",
        "monitor.checkUpsert",
        json!({"name": "db", "probe": {"type": "Tcp", "host": "127.0.0.1", "port": port}, "failureThreshold": 1, "enabled": false}),
    )
    .await;
    let id = c["check"]["check"]["id"].clone();
    let run = ok(&mut ws, "cr", "monitor.checkRun", json!({"checkId": id})).await;
    assert_eq!(run["check"]["state"], "Down");
    let list = ok(&mut ws, "cl", "monitor.checksList", json!({})).await;
    assert_eq!(list["checks"].as_array().unwrap().len(), 1);
    let alerts = ok(&mut ws, "al", "monitor.alerts", json!({})).await;
    let alert = alerts["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["source"]["type"] == "Check")
        .cloned()
        .expect("check alert");
    let acked = ok(&mut ws, "ack", "monitor.ackAlert", json!({"alertId": alert["id"]})).await;
    assert_eq!(acked["alert"]["state"], "Acknowledged");
    ok(&mut ws, "cd", "monitor.checkDelete", json!({"checkId": id})).await;
    let alerts = ok(&mut ws, "al2", "monitor.alerts", json!({"includeResolved": true})).await;
    let a = alerts["alerts"].as_array().unwrap().iter().find(|a| a["id"] == alert["id"]).cloned().unwrap();
    assert_eq!(a["state"], "Resolved");
    let invalid =
        call(&mut ws, "ci", "monitor.checkUpsert", json!({"name": "x", "probe": {"type": "Http", "url": "ftp://x"}}))
            .await;
    assert_eq!(invalid["ok"], false);
    core.shutdown().await;
}
