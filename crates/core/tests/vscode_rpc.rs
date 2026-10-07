//! `vscode.*` over a real WebSocket: status, consent, validation, and that
//! nothing is downloaded or started until asked. (The real download + server
//! run is exercised manually / by `orkestr-vscode-host`'s lifecycle tests.)

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(20), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

async fn call(ws: &mut Ws, id: &str, method: &str, params: Value) -> Value {
    let frame = json!({"type": "req", "id": id, "method": method, "params": params});
    ws.send(Message::Text(frame.to_string().into())).await.unwrap();
    loop {
        let v = recv(ws).await;
        if v["type"] == "res" && v["id"] == id {
            return v;
        }
    }
}

#[tokio::test]
async fn vscode_status_consent_and_config() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    assert_eq!(recv(&mut ws).await["ok"], true);

    let s = call(&mut ws, "1", "vscode.status", json!({})).await;
    assert_eq!(s["ok"], true, "{s}");
    let status = &s["result"];
    assert_eq!(status["state"], "notInstalled");
    assert_eq!(status["config"]["autoStop"], true);
    assert!(status["dataDir"].as_str().unwrap().ends_with("vscode-server"));
    assert!(status["running"].is_null());

    // No consent → refused; missing field → invalid params.
    let r = call(&mut ws, "2", "vscode.install", json!({"accept": false})).await;
    assert_eq!(r["ok"], false);
    assert_eq!(r["error"]["code"], "bad_request");
    assert!(r["error"]["message"].as_str().unwrap().contains("consent"));
    let r = call(&mut ws, "3", "vscode.install", json!({})).await;
    assert_eq!(r["error"]["code"], "bad_request");

    // Not installed → start is a clear error, not a fake success.
    let r = call(&mut ws, "4", "vscode.start", json!({})).await;
    assert_eq!(r["ok"], false);
    assert_eq!(r["error"]["code"], "not_found");

    // Config validation + persistence.
    let r = call(&mut ws, "5", "vscode.setConfig", json!({"idleStopSecs": 1})).await;
    assert_eq!(r["error"]["code"], "bad_request");
    let r = call(&mut ws, "6", "vscode.setConfig", json!({"autoRestart": false})).await;
    assert_eq!(r["result"]["config"]["autoRestart"], false);

    // Stop when nothing runs is a no-op; Stop All AI includes the VS Code server.
    let r = call(&mut ws, "7", "vscode.stop", json!({})).await;
    assert_eq!(r["result"]["state"], "notInstalled");
    let r = call(&mut ws, "8", "emergency.stopAll", json!({})).await;
    assert_eq!(r["ok"], true, "{r}");
    let r = call(&mut ws, "9", "vscode.logs", json!({})).await;
    assert!(r["result"]["lines"].is_array());
    let r = call(&mut ws, "10", "vscode.keepAlive", json!({})).await;
    assert_eq!(r["ok"], true);

    // Extensions: ids are validated before anything runs; nothing installed → clear error.
    let r = call(&mut ws, "11", "vscode.installExtension", json!({"id": "--force"})).await;
    assert_eq!(r["error"]["code"], "bad_request", "{r}");
    let r = call(&mut ws, "12", "vscode.listExtensions", json!({})).await;
    assert_eq!(r["ok"], false, "{r}");

    drop(ws);
    core.shutdown().await;
}
