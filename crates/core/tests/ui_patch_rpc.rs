//! End-to-end (real WebSocket) tests of the Phase 3 hud-perf-aiui area:
//! `ui.previewPatch`, `ui.applyPatch` (validation, versions, conflicts,
//! undo through the inverse patch, performance-mode mirroring, events) and
//! `nodes.power`.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

async fn call(ws: &mut Ws, id: &str, method: &str, params: Value, events: &mut Vec<Value>) -> Value {
    send(ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
    loop {
        let v = recv(ws, Duration::from_secs(30)).await;
        match v["type"].as_str() {
            Some("res") if v["id"] == id => return v,
            Some("event") => events.push(v),
            _ => {}
        }
    }
}

async fn ok(ws: &mut Ws, id: &str, method: &str, params: Value, events: &mut Vec<Value>) -> Value {
    let res = call(ws, id, method, params, events).await;
    assert_eq!(res["ok"], true, "{method}: {res}");
    res["result"].clone()
}

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

#[tokio::test]
async fn ui_patches_preview_apply_conflict_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    subscribe(&mut ws, "s", &["global"]).await;
    let mut events = Vec::new();

    // Empty preview = current document (defaults, version 0).
    let cur = ok(&mut ws, "1", "ui.previewPatch", json!({"patch": []}), &mut events).await;
    assert_eq!(cur["version"], 0);
    assert_eq!(cur["base"]["schemaVersion"], 1);

    let patch = json!([
        {"op": "replace", "path": "/prefs/density", "value": "Compact"},
        {"op": "add", "path": "/prefs/panels/missionControl.claude", "value": {"side": "Right", "hidden": false}},
        {"op": "add", "path": "/prefs/hiddenWidgets/-", "value": "missionControl.tokenChart"},
        {"op": "replace", "path": "/prefs/backgroundDim", "value": 0.55},
        {"op": "replace", "path": "/performanceMode", "value": "Performance"}
    ]);
    let pv = ok(&mut ws, "2", "ui.previewPatch", json!({"patch": patch, "author": "Manager"}), &mut events).await;
    assert_eq!(pv["config"]["prefs"]["density"], "Compact");
    assert_eq!(pv["changes"].as_array().unwrap().len(), 5);
    // Preview persists nothing.
    let again = ok(&mut ws, "3", "ui.previewPatch", json!({"patch": []}), &mut events).await;
    assert_eq!(again["version"], 0);

    // Rejections: forbidden path, code injection, unknown member.
    for (i, bad) in [
        json!([{"op": "replace", "path": "/schemaVersion", "value": 2}]),
        json!([{"op": "add", "path": "/theme/tokens/color-bg", "value": "url(https://evil.example)"}]),
        json!([{"op": "add", "path": "/prefs/onload", "value": "alert(1)"}]),
        json!([{"op": "add", "path": "/layout/docking", "value": {}}]),
    ]
    .into_iter()
    .enumerate()
    {
        let res =
            call(&mut ws, &format!("bad{i}"), "ui.applyPatch", json!({"patch": bad, "author": "Manager"}), &mut events)
                .await;
        assert_eq!(res["ok"], false, "{res}");
        assert_eq!(res["error"]["code"], "bad_request", "{res}");
        assert!(res["error"]["data"].is_array(), "issues are attached: {res}");
    }

    // Apply against the previewed version.
    let applied = ok(
        &mut ws,
        "4",
        "ui.applyPatch",
        json!({"patch": patch, "author": "Manager", "baseVersion": 0, "summary": "Compact MC", "proposalId": "p1"}),
        &mut events,
    )
    .await;
    assert_eq!(applied["version"], 1);
    let inverse = applied["inverse"].clone();
    assert_eq!(inverse.as_array().unwrap().len(), 5);

    // Stale base version → conflict.
    let res = call(&mut ws, "5", "ui.applyPatch", json!({"patch": patch, "baseVersion": 0}), &mut events).await;
    assert_eq!(res["error"]["code"], "conflict", "{res}");

    // The performance mode was mirrored into the settings.
    let settings = ok(&mut ws, "6", "settings.get", json!({}), &mut events).await;
    assert_eq!(settings["settings"]["performanceMode"], "Performance");

    // Undo = apply the inverse.
    let undone =
        ok(&mut ws, "7", "ui.applyPatch", json!({"patch": inverse, "baseVersion": 1, "summary": "Undo"}), &mut events)
            .await;
    assert_eq!(undone["version"], 2);
    assert_eq!(undone["config"]["prefs"]["density"], "Comfortable");
    assert_eq!(undone["config"]["prefs"]["hiddenWidgets"], json!([]));
    assert_eq!(undone["config"]["performanceMode"], "Balanced");

    // Events: UiConfigPatched (x2) and SettingsChanged.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline
        && events.iter().filter(|e| e["event"]["kind"] == "UiConfigPatched").count() < 2
    {
        if let Ok(Some(Ok(Message::Text(t)))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["type"] == "event" {
                events.push(v);
            }
        }
    }
    let patched: Vec<&Value> = events.iter().filter(|e| e["event"]["kind"] == "UiConfigPatched").collect();
    assert_eq!(patched.len(), 2, "{events:?}");
    assert_eq!(patched[0]["event"]["payload"]["proposalId"], "p1");
    assert_eq!(patched[0]["event"]["payload"]["author"], "Manager");
    assert!(events.iter().any(|e| e["event"]["kind"] == "SettingsChanged"));

    // Survives a restart.
    drop(ws);
    core.shutdown().await;
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    let cur = ok(&mut ws, "8", "ui.previewPatch", json!({"patch": []}), &mut events).await;
    assert_eq!(cur["version"], 2);

    // nodes.power answers on every host (supported or not).
    let power = ok(&mut ws, "9", "nodes.power", json!({}), &mut events).await;
    assert!(power["supported"].is_boolean(), "{power}");
    assert!(power["hasBattery"].is_boolean(), "{power}");
    core.shutdown().await;
}
