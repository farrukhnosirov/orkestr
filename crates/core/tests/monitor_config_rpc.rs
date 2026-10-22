//! `monitor.rules` / `ruleUpsert` / `ruleDelete` / `sourceAdd` / `sourceRemove` and
//! `detective.settings` / `setSettings` over a real WebSocket (ADR-0042).

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

async fn err(ws: &mut Ws, id: &str, method: &str, params: Value) {
    let res = call(ws, id, method, params).await;
    assert_eq!(res["ok"], false, "{method} ({id}) should fail: {res}");
}

fn rule_ids(v: &Value) -> Vec<String> {
    v["rules"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect()
}

fn rule(id: &str, name: &str, kind: Value) -> Value {
    json!({"rule": {"id": id, "name": name, "severity": "Info", "enabled": true, "kind": kind}})
}

#[tokio::test]
async fn alert_rules_validate_persist_and_defaults_are_seeded_once() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;

    let ids = rule_ids(&ok(&mut ws, "r0", "monitor.rules", json!({})).await);
    assert_eq!(ids, ["check-down", "cpu-high", "mem-high", "disk-high", "log-errors"]);

    // Validation per kind.
    let bad = [
        rule("bad id", "x", json!({"type": "CheckDown"})),
        rule("x", "  ", json!({"type": "CheckDown"})),
        rule("x", "x", json!({"type": "MetricThreshold", "metric": "net", "above": 50.0, "samples": 3})),
        rule("x", "x", json!({"type": "MetricThreshold", "metric": "cpu", "above": 100.0, "samples": 3})),
        rule("x", "x", json!({"type": "MetricThreshold", "metric": "cpu", "above": 50.0, "samples": 0})),
        rule("x", "x", json!({"type": "LogRate", "count": 0, "windowSecs": 60})),
        rule("x", "x", json!({"type": "LogRate", "count": 5, "windowSecs": 86_401})),
        rule("x", "x", json!({"type": "LogRate", "count": 5, "windowSecs": 60, "pattern": " "})),
        rule("x", "x", json!({"type": "CheckDown", "checkId": "00000000-0000-0000-0000-000000000000"})),
        rule("x", "x", json!({"type": "Nope"})),
    ];
    for (i, p) in bad.into_iter().enumerate() {
        err(&mut ws, &format!("bad{i}"), "monitor.ruleUpsert", p).await;
    }

    // Create, then edit in place (the kind can change).
    let r = ok(
        &mut ws,
        "u1",
        "monitor.ruleUpsert",
        rule(
            "gpu0-hot",
            " GPU 0 hot ",
            json!({"type": "MetricThreshold", "metric": "gpu0", "above": 90.0, "samples": 4}),
        ),
    )
    .await;
    assert_eq!(r["rule"]["name"], "GPU 0 hot");
    let edited = json!({"rule": {"id": "gpu0-hot", "name": "GPU 0 hot", "severity": "Warning", "enabled": false,
        "kind": {"type": "LogRate", "pattern": "cuda", "count": 3, "windowSecs": 120, "minLevel": "warn"}}});
    ok(&mut ws, "u2", "monitor.ruleUpsert", edited).await;
    let rules = ok(&mut ws, "r1", "monitor.rules", json!({})).await;
    let g = rules["rules"].as_array().unwrap().iter().find(|r| r["id"] == "gpu0-hot").unwrap().clone();
    assert_eq!(
        (g["severity"].as_str(), g["enabled"].as_bool(), g["kind"]["type"].as_str()),
        (Some("Warning"), Some(false), Some("LogRate"))
    );
    assert_eq!(rule_ids(&rules).len(), 6);

    // Delete everything; unknown ids fail.
    for id in rule_ids(&rules) {
        ok(&mut ws, &format!("d-{id}"), "monitor.ruleDelete", json!({ "ruleId": id })).await;
    }
    err(&mut ws, "d-missing", "monitor.ruleDelete", json!({"ruleId": "cpu-high"})).await;
    assert!(rule_ids(&ok(&mut ws, "r2", "monitor.rules", json!({})).await).is_empty());
    ok(&mut ws, "u3", "monitor.ruleUpsert", rule("only", "Only rule", json!({"type": "CheckDown"}))).await;
    drop(ws);
    core.shutdown().await;

    // After a restart the defaults stay deleted; the user's rule survives.
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    assert_eq!(rule_ids(&ok(&mut ws, "r3", "monitor.rules", json!({})).await), ["only"]);
    ok(&mut ws, "d-only", "monitor.ruleDelete", json!({"ruleId": "only"})).await;
    drop(ws);
    core.shutdown().await;
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    assert!(rule_ids(&ok(&mut ws, "r4", "monitor.rules", json!({})).await).is_empty(), "defaults reappeared");
    drop(ws);
    core.shutdown().await;
}

#[tokio::test]
async fn file_sources_are_validated_tailed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut ws = connect(&core).await;
    let log = dir.path().join("app.log");
    std::fs::write(&log, "boot ok\n").unwrap();
    let log_s = log.to_string_lossy().into_owned();

    err(&mut ws, "rel", "monitor.sourceAdd", json!({"path": "relative/app.log"})).await;
    err(&mut ws, "dir", "monitor.sourceAdd", json!({"path": dir.path().to_string_lossy()})).await;
    err(&mut ws, "missing", "monitor.sourceAdd", json!({"path": dir.path().join("nope.log").to_string_lossy()})).await;

    let added = ok(&mut ws, "add", "monitor.sourceAdd", json!({"name": "App", "path": log_s})).await;
    let id = added["source"]["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("file:"), "{id}");
    assert_eq!(added["source"]["kind"]["type"], "File");
    err(&mut ws, "dup-path", "monitor.sourceAdd", json!({"name": "Other", "path": log_s})).await;
    let other = dir.path().join("other.log");
    std::fs::write(&other, "").unwrap();
    err(&mut ws, "dup-name", "monitor.sourceAdd", json!({"name": "app", "path": other.to_string_lossy()})).await;

    // Lines appear (backfill + appended).
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "ERROR request failed").unwrap();
    }
    let mut n = 0;
    for i in 0..100 {
        let l = ok(&mut ws, &format!("lg{i}"), "monitor.logs", json!({ "sourceId": id })).await;
        n = l["lines"].as_array().unwrap().len();
        if n >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(n >= 2, "file lines never appeared ({n})");

    // Built-in sources cannot be removed.
    ok(&mut ws, "in", "monitor.ingest", json!({"stream": "web", "lines": [{"message": "hi"}]})).await;
    err(&mut ws, "rm-ingest", "monitor.sourceRemove", json!({"sourceId": "ingest:web"})).await;
    ok(&mut ws, "rm", "monitor.sourceRemove", json!({ "sourceId": id })).await;
    err(&mut ws, "rm2", "monitor.sourceRemove", json!({ "sourceId": id })).await;
    let srcs = ok(&mut ws, "ls", "monitor.logSources", json!({})).await;
    assert!(srcs["sources"].as_array().unwrap().iter().all(|s| s["id"] != id.as_str()));
    drop(ws);
    core.shutdown().await;
}

#[tokio::test]
async fn detective_settings_round_trip_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    let s = ok(&mut ws, "s0", "detective.settings", json!({})).await;
    assert_eq!(s, json!({"autoOpenOnAlert": false, "defaultBudgetUsd": 2.0, "maxFixAttempts": 2}));
    let bad = [
        json!({"autoOpenOnAlert": true, "defaultBudgetUsd": 0.0, "maxFixAttempts": 2}),
        json!({"autoOpenOnAlert": true, "defaultBudgetUsd": 1000.01, "maxFixAttempts": 2}),
        json!({"autoOpenOnAlert": true, "defaultBudgetUsd": 5.0, "maxFixAttempts": 0}),
        json!({"autoOpenOnAlert": true, "defaultBudgetUsd": 5.0, "maxFixAttempts": 6}),
    ];
    for (i, p) in bad.into_iter().enumerate() {
        err(&mut ws, &format!("b{i}"), "detective.setSettings", p).await;
    }
    assert_eq!(ok(&mut ws, "s1", "detective.settings", json!({})).await, s, "a refused save changed nothing");
    let want = json!({"autoOpenOnAlert": true, "defaultBudgetUsd": 1000.0, "maxFixAttempts": 5});
    assert_eq!(ok(&mut ws, "set", "detective.setSettings", want.clone()).await, want);
    drop(ws);
    core.shutdown().await;
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut ws = connect(&core).await;
    assert_eq!(ok(&mut ws, "s2", "detective.settings", json!({})).await, want);
    drop(ws);
    core.shutdown().await;
}
