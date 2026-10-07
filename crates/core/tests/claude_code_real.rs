//! Live check against the user's real, signed-in Claude Code binary
//! (ignored by default: it spends a little of the account's usage).
//!
//! ```text
//! ORKESTR_REAL_CLAUDE=C:\path\to\claude.exe cargo test -p orkestr-core --test claude_code_real -- --ignored --nocapture
//! ```

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(180), ws.next()).await.expect("timeout").expect("closed").unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

async fn call(ws: &mut Ws, events: &mut Vec<Value>, id: &str, method: &str, params: Value) -> Value {
    ws.send(Message::Text(json!({"type": "req", "id": id, "method": method, "params": params}).to_string().into()))
        .await
        .unwrap();
    loop {
        let v = recv(ws).await;
        if v["type"] == "res" && v["id"] == id {
            assert_eq!(v["ok"], true, "{method}: {v}");
            return v["result"].clone();
        }
        if v["type"] == "event" {
            events.push(v["event"].clone());
        }
    }
}

async fn wait_item(ws: &mut Ws, events: &mut Vec<Value>, ty: &str) -> Value {
    let is = |e: &Value| e["kind"] == "ClaudeCodeEvent" && e["payload"]["event"]["type"] == ty;
    if let Some(pos) = events.iter().position(is) {
        return events.remove(pos)["payload"]["event"].clone();
    }
    loop {
        let v = recv(ws).await;
        if v["type"] == "event" {
            let e = v["event"].clone();
            println!("{}", e["payload"]["event"]["type"].as_str().or(e["kind"].as_str()).unwrap_or("?"));
            if is(&e) {
                return e["payload"]["event"].clone();
            }
            events.push(e);
        }
    }
}

#[tokio::test]
#[ignore = "needs a signed-in Claude Code binary (ORKESTR_REAL_CLAUDE)"]
async fn real_claude_write_permission_round_trip() {
    let Ok(bin) = std::env::var("ORKESTR_REAL_CLAUDE") else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    recv(&mut ws).await;
    let mut ev = Vec::new();
    let p = call(
        &mut ws,
        &mut ev,
        "1",
        "projects.create",
        json!({"name": "Real", "rootPath": root.path().to_string_lossy()}),
    )
    .await;
    let pid = p["project"]["id"].as_str().unwrap().to_owned();
    let st = call(&mut ws, &mut ev, "2", "claudeCode.setBinaryPath", json!({"path": bin})).await;
    println!("status: {st}");
    let s = call(&mut ws, &mut ev, "3", "claudeCode.start", json!({"projectId": pid, "model": "haiku"})).await;
    let sid = s["session"]["sessionId"].as_str().unwrap().to_owned();
    ws.send(Message::Text(
        json!({"type": "sub", "id": "s", "topics": [format!("claude:{sid}"), format!("project:{pid}")]})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let caps = wait_item(&mut ws, &mut ev, "capabilities").await;
    println!("account: {}", caps["account"]);
    call(
        &mut ws,
        &mut ev,
        "4",
        "claudeCode.send",
        json!({"sessionId": sid, "text": "Create a file named hi.txt containing the word hi using the Write tool. Do not ask questions."}),
    )
    .await;
    let req = wait_item(&mut ws, &mut ev, "permissionRequest").await;
    println!("permission: {req}");
    assert_eq!(req["toolName"], "Write");
    call(
        &mut ws,
        &mut ev,
        "5",
        "claudeCode.respondPermission",
        json!({"sessionId": sid, "requestId": req["requestId"], "decision": "allowOnce"}),
    )
    .await;
    wait_item(&mut ws, &mut ev, "fileChanged").await;
    let res = wait_item(&mut ws, &mut ev, "result").await;
    println!("result: {res}");
    assert_eq!(std::fs::read_to_string(root.path().join("hi.txt")).unwrap().trim(), "hi");
    let list = call(&mut ws, &mut ev, "6", "claudeCode.sessions", json!({"projectId": pid})).await;
    println!("history: {}", list["history"]);
    assert!(!list["history"].as_array().unwrap().is_empty(), "the CLI's transcript is listed");
    call(&mut ws, &mut ev, "7", "claudeCode.stop", json!({"sessionId": sid})).await;
    core.shutdown().await;
}
