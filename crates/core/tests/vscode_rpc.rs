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

/// `vscode.commands` / `vscode.execute` / `vscode.openView` against a mocked
/// bridge extension: manifest mapping, confirmation of dangerous commands,
/// round trip, audit, and `bridgeNotConnected`.
#[tokio::test]
async fn vscode_execute_round_trip_through_a_mock_bridge() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    // One installed extension (manifest on disk).
    let ext = core.services.vscode.layout().extensions_dir().join("mechatroner.rainbow-csv-3.6.0");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(
        ext.join("package.json"),
        r#"{"publisher":"mechatroner","name":"rainbow-csv","version":"3.6.0","displayName":"Rainbow CSV",
            "contributes":{"commands":[{"command":"rainbow-csv.Align","title":"Align columns"},
                                       {"command":"rainbow-csv.DeleteCache","title":"Delete cache"}],
                           "viewsContainers":{"activitybar":[{"id":"rainbow","title":"Rainbow"}]}}}"#,
    )
    .unwrap();

    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    assert_eq!(recv(&mut ws).await["ok"], true);

    let r = call(&mut ws, "1", "vscode.commands", json!({})).await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["bridgeConnected"], false);
    let e = &r["result"]["extensions"][0];
    assert_eq!(e["id"], "mechatroner.rainbow-csv");
    assert_eq!(e["commands"][0]["title"], "Align columns");
    assert_eq!(e["commands"][1]["dangerous"], true);
    assert_eq!(e["viewContainers"][0]["command"], "workbench.view.extension.rainbow");

    // No window connected → a clear, recognisable error.
    let r = call(&mut ws, "2", "vscode.execute", json!({"commandId": "rainbow-csv.Align"})).await;
    assert_eq!(r["error"]["code"], "conflict", "{r}");
    assert_eq!(r["error"]["data"]["reason"], "bridgeNotConnected");
    // Dangerous command without confirmation → approval_required (before any call).
    let r = call(&mut ws, "3", "vscode.execute", json!({"commandId": "rainbow-csv.DeleteCache"})).await;
    assert_eq!(r["error"]["code"], "approval_required", "{r}");
    let r = call(&mut ws, "3b", "vscode.runTask", json!({"name": "build"})).await;
    assert_eq!(r["error"]["code"], "approval_required", "{r}");

    // Mock bridge extension.
    let bridge = core.services.vscode.bridge().clone();
    bridge.set_token(Some("one-time".into()));
    let port = bridge.listen().await.unwrap();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (r_half, mut w_half) = stream.into_split();
    w_half.write_all(b"{\"type\":\"hello\",\"token\":\"one-time\",\"version\":\"0.1.0\"}\n").await.unwrap();
    let mut lines = BufReader::new(r_half).lines();
    assert!(lines.next_line().await.unwrap().unwrap().contains("welcome"));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let seen_mock = std::sync::Arc::clone(&seen);
    let mock = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let req: Value = serde_json::from_str(&line).unwrap();
            seen_mock.lock().unwrap().push(req.clone());
            let result = match req["method"].as_str().unwrap() {
                "listCommands" => json!(["rainbow-csv.Align", "rainbow-csv.hiddenThing"]),
                _ => json!({"ran": req["params"]["command"]}),
            };
            let reply = json!({"type": "res", "id": req["id"], "ok": true, "result": result});
            w_half.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
    });
    assert!(bridge.wait_connected(Duration::from_secs(5)).await);

    let r = call(&mut ws, "4", "vscode.execute", json!({"commandId": "rainbow-csv.Align", "args": [1]})).await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["result"]["ran"], "rainbow-csv.Align");
    let r =
        call(&mut ws, "5", "vscode.execute", json!({"commandId": "rainbow-csv.DeleteCache", "confirmed": true})).await;
    assert_eq!(r["ok"], true, "{r}");
    let r = call(&mut ws, "6", "vscode.openView", json!({"extensionId": "mechatroner.rainbow-csv"})).await;
    assert_eq!(r["result"]["result"]["ran"], "workbench.view.extension.rainbow", "{r}");
    // Runtime-only commands are merged once a window is connected.
    let r = call(&mut ws, "7", "vscode.commands", json!({"extensionId": "mechatroner.rainbow-csv"})).await;
    assert_eq!(r["result"]["bridgeConnected"], true);
    let cmds = r["result"]["extensions"][0]["commands"].as_array().unwrap();
    assert!(cmds.iter().any(|c| c["id"] == "rainbow-csv.hiddenThing" && c["runtimeOnly"] == true), "{cmds:?}");
    let s = call(&mut ws, "7b", "vscode.status", json!({})).await;
    assert_eq!(s["result"]["bridgeConnected"], true);

    mock.abort();
    let seen = seen.lock().unwrap().clone();
    // Order: execute, execute, openView→execute, listCommands.
    assert_eq!(seen[0]["params"]["args"], json!([1]));
    assert_eq!(seen.len(), 4, "{seen:?}");

    // Every execution (and the refused one) is audited.
    let r = call(&mut ws, "8", "audit.list", json!({"action": "vscode.execute", "limit": 50})).await;
    let text = r.to_string();
    assert!(text.contains("rainbow-csv.Align") && text.contains("rainbow-csv.DeleteCache"), "{text}");
    assert!(text.contains("denied") || text.contains("Denied"), "{text}");

    drop(ws);
    core.shutdown().await;
}
