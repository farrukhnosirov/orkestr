//! End-to-end (real WebSocket) tests of `ui.device` / `ui.layouts*`
//! (layout-multimonitor, §29, §34): per-device persistence, activation,
//! rename/delete, validation and survival across core restarts.

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
        let msg = tokio::time::timeout(Duration::from_secs(30), ws.next()).await.expect("timeout").expect("closed").unwrap();
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

fn layout(id: &str, name: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "id": id,
        "name": name,
        "basedOn": "coding",
        "dock": {
            "grid": {
                "root": {"type": "branch", "data": [{"type": "leaf", "data": {"id": "1", "views": ["code"], "activeView": "code"}, "size": 800}], "size": 600},
                "width": 800, "height": 600, "orientation": "HORIZONTAL"
            },
            "panels": {"code": {"id": "code", "contentComponent": "code", "title": "Code"}}
        },
        "panels": ["code"],
        "locked": false,
        "windows": {
            "mainMonitor": "\\\\.\\DISPLAY1",
            "mainMaximized": true,
            "popouts": [{"panelId": "logs", "monitor": "\\\\.\\DISPLAY2", "bounds": {"x": 0, "y": 0, "width": 900, "height": 700}, "maximized": false}]
        }
    })
}

#[tokio::test]
async fn layouts_roundtrip_per_device_and_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;

    // A device id is created on first use and is stable.
    let device = ok(&mut ws, "d1", "ui.device", json!({})).await["deviceId"].as_str().unwrap().to_owned();
    assert_eq!(ok(&mut ws, "d2", "ui.device", json!({})).await["deviceId"], device.as_str());

    // Fresh device: no stored layouts, Coding active.
    let s = ok(&mut ws, "l1", "ui.layoutsList", json!({})).await;
    assert_eq!(s["deviceId"], device.as_str());
    assert_eq!(s["activeId"], "coding");
    assert_eq!(s["layouts"].as_array().unwrap().len(), 0);

    // Save + activate a user layout, customise a built-in without activating it.
    let s = ok(&mut ws, "s1", "ui.layoutsSave", json!({"layout": layout("u-deep-work", "Deep work"), "activate": true})).await;
    assert_eq!(s["activeId"], "u-deep-work");
    let s = ok(&mut ws, "s2", "ui.layoutsSave", json!({"layout": layout("focus", "Focus"), "activate": false})).await;
    assert_eq!(s["activeId"], "u-deep-work");
    assert_eq!(s["layouts"].as_array().unwrap().len(), 2);
    let saved = s["layouts"].as_array().unwrap().iter().find(|l| l["id"] == "u-deep-work").unwrap().clone();
    assert!(saved["updatedAt"].is_string());
    assert_eq!(saved["windows"]["popouts"][0]["monitor"], "\\\\.\\DISPLAY2");

    // Rename; invalid names and documents are rejected and change nothing.
    let s = ok(&mut ws, "r1", "ui.layoutsRename", json!({"id": "u-deep-work", "name": "Глубокая работа"})).await;
    assert!(s["layouts"].as_array().unwrap().iter().any(|l| l["name"] == "Глубокая работа"));
    let bad = call(&mut ws, "r2", "ui.layoutsRename", json!({"id": "u-deep-work", "name": ""})).await;
    assert_eq!(bad["ok"], false, "{bad}");
    let mut broken = layout("u-broken", "Broken");
    broken["panels"] = json!([]);
    broken["windows"]["popouts"] = json!([]);
    let bad = call(&mut ws, "s3", "ui.layoutsSave", json!({"layout": broken, "activate": true})).await;
    assert_eq!(bad["ok"], false, "{bad}");
    let bad = call(&mut ws, "a0", "ui.layoutsSetActive", json!({"id": "u-missing"})).await;
    assert_eq!(bad["ok"], false, "{bad}");

    // Another device sees nothing of this one.
    let other = "0190a000-0000-7000-8000-0000000000ff";
    let s = ok(&mut ws, "o1", "ui.layoutsList", json!({"deviceId": other})).await;
    assert_eq!(s["layouts"].as_array().unwrap().len(), 0);
    assert_eq!(s["activeId"], "coding");

    // Switch to a built-in (no stored document needed).
    let s = ok(&mut ws, "a1", "ui.layoutsSetActive", json!({"id": "review"})).await;
    assert_eq!(s["activeId"], "review");
    core.shutdown().await;

    // Restart: same device, same layouts, same active one.
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    let s = ok(&mut ws, "l2", "ui.layoutsList", json!({})).await;
    assert_eq!(s["deviceId"], device.as_str());
    assert_eq!(s["activeId"], "review");
    assert_eq!(s["layouts"].as_array().unwrap().len(), 2);

    // Deleting the built-in customisation resets it; deleting the active user layout falls back.
    ok(&mut ws, "a2", "ui.layoutsSetActive", json!({"id": "u-deep-work"})).await;
    let s = ok(&mut ws, "x1", "ui.layoutsDelete", json!({"id": "focus"})).await;
    assert_eq!(s["layouts"].as_array().unwrap().len(), 1);
    let s = ok(&mut ws, "x2", "ui.layoutsDelete", json!({"id": "u-deep-work"})).await;
    assert_eq!(s["layouts"].as_array().unwrap().len(), 0);
    assert_eq!(s["activeId"], "coding");
    core.shutdown().await;
}
