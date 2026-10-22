//! `detective.*` over a real WebSocket with the core-wired ports (VCS,
//! providers with the offline Mock, quality, monitor/deploy areas): an
//! incident collects real VCS evidence, asks the provider, and can be
//! cancelled at any time; gates refuse out-of-order approvals.

use std::path::Path;
use std::process::Command;
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

fn git(args: &[&str], dir: &Path) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detective_over_ws_collects_vcs_evidence_and_cancels() {
    if !Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("git not installed — skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&["init", "-q", "-b", "main"], &root);
    git(&["config", "user.name", "Test"], &root);
    git(&["config", "user.email", "test@example.com"], &root);
    std::fs::write(root.join("src/app.js"), "export function div(a, b) {\n  return a / b;\n}\n").unwrap();
    git(&["add", "-A"], &root);
    git(&["commit", "-q", "-m", "add div"], &root);

    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": core.token})).await;
    assert_eq!(recv(&mut ws).await["ok"], true);

    let pid = ok(&mut ws, "p", "projects.create", json!({"name": "Det", "rootPath": root.to_string_lossy()})).await
        ["project"]["id"]
        .clone();
    let inc = ok(
        &mut ws,
        "o",
        "detective.open",
        json!({
            "projectId": pid,
            "trigger": {"type": "LogError", "sourceId": "web", "line": "TypeError: x is undefined\n    at div (/srv/app/src/app.js:2:3)"},
            "budgetUsd": 1.0
        }),
    )
    .await["incident"]
    .clone();
    let id = inc["id"].clone();
    assert_eq!(inc["stage"], "Opened");
    assert_eq!(inc["title"], "TypeError: x is undefined");

    // The investigation runs through Collecting with real VCS + snippet evidence.
    let mut last = Value::Null;
    for i in 0..300 {
        last = ok(&mut ws, &format!("g{i}"), "detective.get", json!({"incidentId": id})).await["incident"].clone();
        let stage = last["stage"].as_str().unwrap().to_owned();
        if !matches!(stage.as_str(), "Opened" | "Collecting") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let kinds: Vec<&str> = last["evidence"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"Commits"), "{kinds:?}");
    assert!(kinds.contains(&"CodeSnippet"), "{kinds:?}");
    assert!(kinds.contains(&"StackTrace"), "{kinds:?}");

    // Gate 2 cannot be approved out of order.
    let e = call(&mut ws, "m", "detective.approveMerge", json!({"incidentId": id})).await;
    assert_eq!(e["ok"], false);

    // Cancel works wherever the run is (or is refused once it ended).
    let res = call(&mut ws, "c", "detective.cancel", json!({"incidentId": id, "note": "test"})).await;
    let after = ok(&mut ws, "g", "detective.get", json!({"incidentId": id})).await["incident"].clone();
    if res["ok"] == true {
        assert_eq!(after["stage"], "Cancelled");
    } else {
        assert!(["Failed", "Resolved", "Cancelled"].contains(&after["stage"].as_str().unwrap()), "{after}");
    }
    let list = ok(&mut ws, "l", "detective.list", json!({"projectId": pid, "includeClosed": true})).await;
    assert_eq!(list["incidents"].as_array().unwrap().len(), 1);
    let open = ok(&mut ws, "l2", "detective.list", json!({"projectId": pid})).await;
    assert!(open["incidents"].as_array().unwrap().is_empty());

    // Validation.
    let e = call(
        &mut ws,
        "v",
        "detective.open",
        json!({"projectId": pid, "trigger": {"type": "Manual", "description": " "}}),
    )
    .await;
    assert_eq!(e["error"]["code"], "bad_request", "{e}");
    core.shutdown().await;
}
