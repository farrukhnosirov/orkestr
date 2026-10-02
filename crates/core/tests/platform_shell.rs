//! End-to-end (real WebSocket) tests of the platform-shell area:
//! `settings.*`, `nodes.*`, `system.checks`, `system.resumeSummary`,
//! `SettingsChanged` / `NodeStatus` events and seq continuity across restarts.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::{AutostartConfig, CoreConfig};
use orkestr_platform::{Autostart, MemoryAutostart};
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
async fn settings_roundtrip_autostart_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let autostart = Arc::new(MemoryAutostart::default());
    let core = orkestr_core::start(config(dir.path(), Arc::clone(&autostart))).await.unwrap();
    let mut ws = connect(&core).await;
    let mut events = Vec::new();
    subscribe(&mut ws, "sub1", &["global"]).await;

    let got = ok(&mut ws, "g1", "settings.get", json!({}), &mut events).await;
    assert_eq!(got["settings"]["closeToTray"], true);
    assert_eq!(got["settings"]["eventRetentionDays"], 180);
    assert_eq!(got["settings"]["onboardingCompleted"], false);
    assert_eq!(got["autostartSupported"], true);

    let upd = ok(
        &mut ws,
        "u1",
        "settings.update",
        json!({"patch": {"closeToTray": false, "autostart": true, "profileName": "Ada", "language": "ru"}}),
        &mut events,
    )
    .await;
    assert_eq!(upd["settings"]["closeToTray"], false);
    assert_eq!(upd["settings"]["autostart"], true);
    let entry = autostart.entry("dev.orkestr.test").expect("autostart registered");
    assert_eq!(entry.args, vec!["--autostart".to_owned()]);

    // Invalid patches are rejected and change nothing.
    let bad = call(&mut ws, "u2", "settings.update", json!({"patch": {"storageMode": "Cloud"}}), &mut events).await;
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"]["code"], "bad_request");

    // SettingsChanged arrives on `global`.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|e| e["event"]["kind"] == "SettingsChanged") {
        assert!(tokio::time::Instant::now() < deadline, "no SettingsChanged event: {events:?}");
        let v = recv(&mut ws, Duration::from_secs(5)).await;
        if v["type"] == "event" {
            events.push(v);
        }
    }
    let ev = events.iter().find(|e| e["event"]["kind"] == "SettingsChanged").unwrap();
    let changed = ev["event"]["payload"]["changed"].as_array().unwrap();
    assert!(changed.iter().any(|c| c == "autostart") && changed.iter().any(|c| c == "closeToTray"));

    // Removing the OS registration by hand is reflected by settings.get.
    autostart.disable("dev.orkestr.test").unwrap();
    let got = ok(&mut ws, "g2", "settings.get", json!({}), &mut events).await;
    assert_eq!(got["settings"]["autostart"], false);
    assert_eq!(got["settings"]["profileName"], "Ada");
    core.shutdown().await;

    // Persisted across restarts.
    let core = orkestr_core::start(config(dir.path(), Arc::new(MemoryAutostart::default()))).await.unwrap();
    let mut ws = connect(&core).await;
    let got = ok(&mut ws, "g3", "settings.get", json!({}), &mut events).await;
    assert_eq!(got["settings"]["closeToTray"], false);
    assert_eq!(got["settings"]["language"], "ru");
    core.shutdown().await;
}

#[tokio::test]
async fn nodes_and_system_checks() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(config(dir.path(), Arc::new(MemoryAutostart::default()))).await.unwrap();
    let node_id = core.services.node_id.to_string();
    let mut ws = connect(&core).await;
    let mut events = Vec::new();

    let list = ok(&mut ws, "n1", "nodes.list", json!({}), &mut events).await;
    assert_eq!(list["nodes"][0]["id"], node_id.as_str());
    assert_eq!(list["nodes"][0]["isLocal"], true);

    let res = ok(&mut ws, "n2", "nodes.resources", json!({}), &mut events).await;
    assert_eq!(res["intervalMs"], 300);
    assert!(res["sample"]["memoryTotalBytes"].as_u64().unwrap() > 0);
    assert!(!res["sample"]["perCorePercent"].as_array().unwrap().is_empty());

    // Heartbeats stream on node:<id> (not on global).
    subscribe(&mut ws, "sub-node", &[&format!("node:{node_id}")]).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let heartbeat = loop {
        assert!(tokio::time::Instant::now() < deadline, "no NodeStatus heartbeat");
        let v = recv(&mut ws, Duration::from_secs(10)).await;
        if v["type"] == "event" && v["event"]["kind"] == "NodeStatus" {
            break v;
        }
    };
    assert_eq!(heartbeat["event"]["nodeId"], node_id.as_str());
    assert!(heartbeat["event"]["payload"]["sample"]["coreProcess"]["pid"].as_u64().is_some());

    let info = ok(&mut ws, "n3", "nodes.info", json!({}), &mut events).await;
    assert!(info["cpu"]["logicalCores"].as_u64().unwrap() >= 1);
    assert!(info["runtimes"].as_array().unwrap().iter().any(|r| r["id"] == "cargo" && r["available"] == true));
    assert!(info["disks"].as_array().unwrap().iter().any(|d| d["isDataVolume"] == true));
    let detected_at = info["detectedAt"].clone();
    let cached = ok(&mut ws, "n4", "nodes.info", json!({}), &mut events).await;
    assert_eq!(cached["detectedAt"], detected_at, "second call is served from cache");

    let checks = ok(&mut ws, "c1", "system.checks", json!({}), &mut events).await;
    let ids: Vec<&str> = checks["checks"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["git", "shell", "dataDir", "webview", "keyring"]);
    let data_dir = checks["checks"].as_array().unwrap().iter().find(|c| c["id"] == "dataDir").unwrap();
    assert_ne!(data_dir["status"], "error");
    core.shutdown().await;
}

#[tokio::test]
async fn resume_summary_and_seq_continuity() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(config(dir.path(), Arc::new(MemoryAutostart::default()))).await.unwrap();
    let mut ws = connect(&core).await;
    let mut events = Vec::new();
    let created = ok(&mut ws, "p1", "projects.create", json!({"name": "Resume me"}), &mut events).await;
    let project_id = created["project"]["id"].as_str().unwrap().to_owned();
    let summary = ok(&mut ws, "r1", "system.resumeSummary", json!({"projectId": project_id}), &mut events).await;
    assert_eq!(summary["previousSessionStartedAt"], Value::Null, "first session ever");
    assert!(summary["counts"].as_array().unwrap().iter().any(|c| c["kind"] == "ProjectCreated"));
    let last_seq_before = core.services.events.last_seq();
    core.shutdown().await;

    let core = orkestr_core::start(config(dir.path(), Arc::new(MemoryAutostart::default()))).await.unwrap();
    assert_eq!(core.services.events.last_seq(), last_seq_before, "clean restart continues exactly");
    let mut ws = connect(&core).await;
    let summary = ok(&mut ws, "r2", "system.resumeSummary", json!({"projectId": project_id}), &mut events).await;
    assert!(summary["previousSessionStartedAt"].is_string());
    let summary = ok(
        &mut ws,
        "r3",
        "system.resumeSummary",
        json!({"projectId": project_id, "since": "2000-01-01T00:00:00Z"}),
        &mut events,
    )
    .await;
    assert!(summary["totalEvents"].as_u64().unwrap() >= 1);
    let bad = call(&mut ws, "r4", "system.resumeSummary", json!({"since": "yesterday"}), &mut events).await;
    assert_eq!(bad["error"]["code"], "bad_request");
    core.shutdown().await;
}
