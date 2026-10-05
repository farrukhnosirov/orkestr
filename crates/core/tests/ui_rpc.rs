//! End-to-end (real WebSocket) tests of the theme engine area (`ui.*`):
//! config get/set with per-device overrides and optimistic concurrency,
//! server-side validation (unsafe tokens, WCAG hard invariants), the theme
//! library (built-ins read-only, save/overwrite/delete) and events.

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

async fn err(ws: &mut Ws, id: &str, method: &str, params: Value, events: &mut Vec<Value>) -> String {
    let res = call(ws, id, method, params, events).await;
    assert_eq!(res["ok"], false, "{method} should fail: {res}");
    res["error"]["code"].as_str().unwrap().to_owned()
}

async fn wait_kind(ws: &mut Ws, events: &mut Vec<Value>, kind: &str, n: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while events.iter().filter(|e| e["event"]["kind"] == kind).count() < n {
        assert!(tokio::time::Instant::now() < deadline, "missing {kind} events: {events:?}");
        let v = recv(ws, Duration::from_secs(5)).await;
        if v["type"] == "event" {
            events.push(v);
        }
    }
}

#[tokio::test]
async fn ui_config_and_theme_library() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    let mut ev = Vec::new();
    send(&mut ws, json!({"type": "sub", "id": "s", "topics": ["global"]})).await;

    // Defaults: Nebula, never stored, contrast report included.
    let got = ok(&mut ws, "g1", "ui.getConfig", json!({}), &mut ev).await;
    assert_eq!(got["config"]["theme"]["id"], "nebula");
    assert_eq!(got["version"], 0);
    assert_eq!(got["scope"], "Global");
    assert!(got["contrast"].as_array().unwrap().iter().all(|c| c["pass"] == true));

    // Library: six built-ins, Nebula first.
    let lib = ok(&mut ws, "t1", "ui.themes", json!({}), &mut ev).await;
    let themes = lib["themes"].as_array().unwrap();
    assert_eq!(themes.len(), 6);
    assert_eq!(themes[0]["theme"]["id"], "nebula");
    let solar = themes.iter().find(|t| t["theme"]["id"] == "solar-light").unwrap()["theme"].clone();

    // Global set with optimistic concurrency.
    let mut cfg = got["config"].clone();
    cfg["theme"] = solar.clone();
    let set = ok(&mut ws, "s1", "ui.setConfig", json!({"config": cfg, "baseVersion": 0}), &mut ev).await;
    assert_eq!(set["version"], 1);
    assert_eq!(set["config"]["theme"]["id"], "solar-light");
    assert_eq!(err(&mut ws, "s2", "ui.setConfig", json!({"config": cfg, "baseVersion": 0}), &mut ev).await, "conflict");

    // Unsafe and illegible themes are rejected by the core.
    let mut bad = cfg.clone();
    bad["theme"]["tokens"]["color-bg"] = json!("url(javascript:alert(1))");
    assert_eq!(err(&mut ws, "s3", "ui.setConfig", json!({"config": bad}), &mut ev).await, "bad_request");
    let mut illegible = cfg.clone();
    illegible["theme"]["tokens"]["color-text"] = json!("#f0f0f0");
    let res = call(&mut ws, "s4", "ui.setConfig", json!({"config": illegible}), &mut ev).await;
    assert_eq!(res["error"]["code"], "bad_request");
    assert!(res["error"]["data"].as_array().unwrap().iter().any(|i| i["path"] == "theme.contrast.text-bg"), "{res}");

    // Per-device override, then removal.
    let mut dev = cfg.clone();
    dev["theme"] = themes.iter().find(|t| t["theme"]["id"] == "carbon").unwrap()["theme"].clone();
    ok(&mut ws, "d1", "ui.setConfig", json!({"config": dev, "deviceId": "laptop-1"}), &mut ev).await;
    let mine = ok(&mut ws, "d2", "ui.getConfig", json!({"deviceId": "laptop-1"}), &mut ev).await;
    assert_eq!((mine["scope"].as_str(), mine["config"]["theme"]["id"].as_str()), (Some("Device"), Some("carbon")));
    let other = ok(&mut ws, "d3", "ui.getConfig", json!({"deviceId": "desk-2"}), &mut ev).await;
    assert_eq!(other["config"]["theme"]["id"], "solar-light");
    assert_eq!(err(&mut ws, "d4", "ui.getConfig", json!({"deviceId": "../x"}), &mut ev).await, "bad_request");
    ok(&mut ws, "d5", "ui.setConfig", json!({"config": null, "deviceId": "laptop-1"}), &mut ev).await;
    let back = ok(&mut ws, "d6", "ui.getConfig", json!({"deviceId": "laptop-1"}), &mut ev).await;
    assert_eq!(back["scope"], "Global");
    assert_eq!(err(&mut ws, "d7", "ui.setConfig", json!({"config": null}), &mut ev).await, "bad_request");

    // Built-ins are read-only.
    assert_eq!(err(&mut ws, "b1", "ui.saveTheme", json!({"theme": solar}), &mut ev).await, "bad_request");
    assert_eq!(err(&mut ws, "b2", "ui.deleteTheme", json!({"id": "nebula"}), &mut ev).await, "forbidden");

    // User theme: save, conflict without overwrite, overwrite updates configs using it.
    let mut mine_theme = solar.clone();
    mine_theme["id"] = json!("my-solar");
    mine_theme["name"] = json!("My Solar");
    let saved = ok(&mut ws, "u1", "ui.saveTheme", json!({"theme": mine_theme}), &mut ev).await;
    assert_eq!(saved["entry"]["builtin"], false);
    assert_eq!(err(&mut ws, "u2", "ui.saveTheme", json!({"theme": mine_theme}), &mut ev).await, "conflict");
    let mut cfg2 = cfg.clone();
    cfg2["theme"] = mine_theme.clone();
    ok(&mut ws, "u3", "ui.setConfig", json!({"config": cfg2}), &mut ev).await;
    mine_theme["tokens"]["color-accent"] = json!("#92400e");
    let over = ok(&mut ws, "u4", "ui.saveTheme", json!({"theme": mine_theme, "overwrite": true}), &mut ev).await;
    assert_eq!(over["configsUpdated"], 1);
    let now = ok(&mut ws, "u5", "ui.getConfig", json!({}), &mut ev).await;
    assert_eq!(now["config"]["theme"]["tokens"]["color-accent"], "#92400e");
    let lib = ok(&mut ws, "u6", "ui.themes", json!({}), &mut ev).await;
    assert_eq!(lib["themes"].as_array().unwrap().len(), 7);

    // Delete: configs using it fall back to Nebula.
    let del = ok(&mut ws, "x1", "ui.deleteTheme", json!({"id": "my-solar"}), &mut ev).await;
    assert_eq!((del["deleted"].as_bool(), del["configsReset"].as_u64()), (Some(true), Some(1)));
    let after = ok(&mut ws, "x2", "ui.getConfig", json!({}), &mut ev).await;
    assert_eq!(after["config"]["theme"]["id"], "nebula");
    let again = ok(&mut ws, "x3", "ui.deleteTheme", json!({"id": "my-solar"}), &mut ev).await;
    assert_eq!(again["deleted"], false);

    // Performance mode is forwarded to settings (single source of truth).
    let mut perf = after["config"].clone();
    perf["performanceMode"] = json!("BatterySaver");
    ok(&mut ws, "p1", "ui.setConfig", json!({"config": perf}), &mut ev).await;
    let s = ok(&mut ws, "p2", "settings.get", json!({}), &mut ev).await;
    assert_eq!(s["settings"]["performanceMode"], "BatterySaver");

    wait_kind(&mut ws, &mut ev, "UiConfigChanged", 5).await;
    wait_kind(&mut ws, &mut ev, "UiThemesChanged", 3).await;
    wait_kind(&mut ws, &mut ev, "SettingsChanged", 1).await;
    core.shutdown().await;
}

#[tokio::test]
async fn legacy_theme_setting_seeds_the_global_config() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    let mut ev = Vec::new();
    ok(&mut ws, "a", "settings.update", json!({"patch": {"themeId": "aurora"}}), &mut ev).await;
    let got = ok(&mut ws, "b", "ui.getConfig", json!({}), &mut ev).await;
    assert_eq!(got["config"]["theme"]["id"], "aurora");
    core.shutdown().await;
}
