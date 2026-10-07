//! `claudeCode.*` end to end over a real WebSocket, driving the
//! `orkestr-fake-claude` test double (same stream-json + control protocol as
//! the official CLI): binary pinning + status, a session, a `can_use_tool`
//! round-trip (allow → file written → `FileChanged`), a forbidden command
//! auto-denied by the DangerousActionGuard, interrupt, mode / model changes,
//! the bypass confirmation, audit records and Stop All AI.

use std::collections::VecDeque;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    events: VecDeque<Value>,
    seq: u64,
}

impl Client {
    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(30), self.ws.next())
                .await
                .expect("timeout")
                .expect("closed")
                .unwrap();
            match msg {
                Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected message {other:?}"),
            }
        }
    }

    async fn frame(&mut self, frame: Value, id: &str) -> Value {
        self.ws.send(Message::Text(frame.to_string().into())).await.unwrap();
        loop {
            let v = self.recv().await;
            if v["type"] == "res" && v["id"] == id {
                return v;
            }
            if v["type"] == "event" {
                self.events.push_back(v["event"].clone());
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.seq += 1;
        let id = format!("r{}", self.seq);
        self.frame(json!({"type": "req", "id": id, "method": method, "params": params}), &id).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    async fn sub(&mut self, topic: String) {
        self.seq += 1;
        let id = format!("s{}", self.seq);
        let r = self.frame(json!({"type": "sub", "id": id, "topics": [topic]}), &id).await;
        assert_eq!(r["ok"], true, "{r}");
    }

    /// Waits for an event matching `pred` (buffered events first).
    async fn wait(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(pos) = self.events.iter().position(&pred) {
            return self.events.remove(pos).unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
            let v = self.recv().await;
            if v["type"] == "event" {
                let ev = v["event"].clone();
                if pred(&ev) {
                    return ev;
                }
                self.events.push_back(ev);
            }
        }
    }

    async fn item(&mut self, ty: &str) -> Value {
        let ty = ty.to_owned();
        self.wait(&ty.clone(), move |e| e["kind"] == "ClaudeCodeEvent" && e["payload"]["event"]["type"] == ty.as_str())
            .await["payload"]["event"]
            .clone()
    }
}

#[tokio::test]
async fn claude_code_session_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);

    let project = c.ok("projects.create", json!({"name": "Claude", "rootPath": root.path().to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();

    // Pinning a bad path is refused; the fake binary is accepted and probed.
    let r = c.call("claudeCode.setBinaryPath", json!({"path": "relative/claude"})).await;
    assert_eq!(r["error"]["code"], "bad_request", "{r}");
    let fake = env!("CARGO_BIN_EXE_orkestr-fake-claude");
    let st = c.ok("claudeCode.setBinaryPath", json!({"path": fake})).await;
    assert_eq!(st["binary"]["source"], "override", "{st}");
    assert_eq!(st["binary"]["version"], "9.9.9");
    assert_eq!(st["auth"]["loggedIn"], true);
    assert_eq!(st["auth"]["subscriptionType"], "max");

    // Sessions outside the project are refused.
    let other = tempfile::tempdir().unwrap();
    let r = c.call("claudeCode.start", json!({"projectId": pid, "cwd": other.path().to_string_lossy()})).await;
    assert_eq!(r["error"]["code"], "forbidden", "{r}");
    let r = c.call("claudeCode.start", json!({"projectId": pid, "permissionMode": "bypassPermissions"})).await;
    assert_eq!(r["error"]["code"], "bad_request", "bypass needs confirmation: {r}");

    let started = c.ok("claudeCode.start", json!({"projectId": pid, "model": "sonnet"})).await;
    let sid = started["session"]["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(started["session"]["permissionMode"], "default");
    c.sub(format!("claude:{sid}")).await;
    c.sub(format!("project:{pid}")).await;
    let caps = c.item("capabilities").await;
    assert!(caps["models"].as_array().unwrap().iter().any(|m| m["value"] == "haiku"), "{caps}");
    assert_eq!(caps["account"]["loggedIn"], true);

    // can_use_tool round trip: allow once → file written → FileChanged.
    c.ok("claudeCode.send", json!({"sessionId": sid, "text": "please write a file"})).await;
    let init = c.item("init").await;
    assert_eq!(init["claudeSessionId"], "7a0c9a52-5b3e-4a7e-9f3c-2f0f6c1d9e11");
    c.item("thinking").await;
    c.item("textDelta").await;
    let req = c.item("permissionRequest").await;
    assert_eq!(req["toolName"], "Write");
    assert_eq!(req["canRemember"], true);
    assert!(req["risk"].is_null());
    let waiting = c
        .wait("waiting state", |e| e["kind"] == "ClaudeCodeStatus" && e["payload"]["session"]["state"] == "waitingPermission")
        .await;
    assert_eq!(waiting["payload"]["session"]["title"], "please write a file");
    let rid = req["requestId"].as_str().unwrap().to_owned();
    let r = c.call("claudeCode.respondPermission", json!({"sessionId": sid, "requestId": "nope", "decision": "allowOnce"})).await;
    assert_eq!(r["error"]["code"], "bad_request");
    c.ok("claudeCode.respondPermission", json!({"sessionId": sid, "requestId": rid, "decision": "allowOnce"})).await;
    let resolved = c.item("permissionResolved").await;
    assert_eq!(resolved["decision"], "allowOnce");
    let tr = c.item("toolResult").await;
    assert_eq!(tr["isError"], false);
    c.item("fileChanged").await;
    let fc = c.wait("FileChanged", |e| e["kind"] == "FileChanged").await;
    assert_eq!(fc["payload"]["path"], "fake.txt");
    assert_eq!(fc["payload"]["actor"], "agent");
    let res = c.item("result").await;
    assert_eq!(res["isError"], false);
    assert_eq!(res["usage"]["cacheReadTokens"], 100);
    assert_eq!(std::fs::read_to_string(root.path().join("fake.txt")).unwrap(), "hello");

    // A forbidden command is denied by Orkestr's guard without asking.
    c.ok("claudeCode.send", json!({"sessionId": sid, "text": "rm everything"})).await;
    let req = c.item("permissionRequest").await;
    assert_eq!(req["toolName"], "Bash");
    assert_eq!(req["risk"]["level"], "forbidden", "{req}");
    assert_eq!(req["canRemember"], false);
    let resolved = c.item("permissionResolved").await;
    assert_eq!(resolved["decision"], "deny");
    assert_eq!(resolved["byPolicy"], true);
    let tr = c.item("toolResult").await;
    assert_eq!(tr["isError"], true);
    assert!(tr["content"].as_str().unwrap().contains("forbids"), "{tr}");
    c.item("result").await;

    // Interrupt a streaming turn.
    c.ok("claudeCode.send", json!({"sessionId": sid, "text": "go slow"})).await;
    c.item("textDelta").await;
    c.ok("claudeCode.interrupt", json!({"sessionId": sid})).await;
    c.item("interrupted").await;
    let res = c.item("result").await;
    assert_eq!(res["isError"], true);

    // Mode and model.
    let info = c.ok("claudeCode.setMode", json!({"sessionId": sid, "mode": "plan"})).await;
    assert_eq!(info["permissionMode"], "plan");
    c.ok("claudeCode.setModel", json!({"sessionId": sid, "model": "haiku"})).await;
    let mc = c.item("modelChanged").await;
    assert_eq!(mc["model"], "haiku");
    let r = c.call("claudeCode.setModel", json!({"sessionId": sid, "model": "--bad"})).await;
    assert_eq!(r["error"]["code"], "bad_request");
    let r = c.call("claudeCode.setMode", json!({"sessionId": sid, "mode": "bypassPermissions"})).await;
    assert_eq!(r["error"]["code"], "bad_request", "{r}");
    // Confirmed bypass restarts the process with the bypass flag (resuming the conversation).
    let info = c.ok("claudeCode.setMode", json!({"sessionId": sid, "mode": "bypassPermissions", "confirmBypass": true})).await;
    assert_eq!(info["bypassAllowed"], true);
    assert_eq!(info["permissionMode"], "bypassPermissions");
    c.item("capabilities").await; // the new process answered `initialize`
    c.ok("claudeCode.send", json!({"sessionId": sid, "text": "hello again"})).await;
    let text = c.wait("pong", |e| e["kind"] == "ClaudeCodeEvent" && e["payload"]["event"]["type"] == "text" && e["payload"]["event"]["text"].as_str().is_some_and(|t| t.starts_with("pong"))).await;
    assert!(text.to_string().contains("hello again"));

    // Every permission decision is audited.
    let audit = c.ok("audit.list", json!({"action": "claudeCode."})).await;
    let s = audit.to_string();
    assert!(s.contains("claudeCode.permission") && s.contains("claudeCode.start") && s.contains("autoDenied"), "{s}");

    // Live sessions + status.
    let list = c.ok("claudeCode.sessions", json!({"projectId": pid})).await;
    assert_eq!(list["live"].as_array().unwrap().len(), 1);
    let st = c.ok("claudeCode.status", json!({})).await;
    assert_eq!(st["sessions"].as_array().unwrap().len(), 1);

    // Stop All AI ends the session; new sessions are refused until reset.
    c.ok("emergency.stopAll", json!({})).await;
    c.wait("exited", |e| e["kind"] == "ClaudeCodeStatus" && e["payload"]["session"]["state"] == "exited").await;
    let r = c.call("claudeCode.start", json!({"projectId": pid})).await;
    assert_eq!(r["error"]["code"], "forbidden", "{r}");
    c.ok("emergency.reset", json!({})).await;
    let again = c.ok("claudeCode.start", json!({"projectId": pid})).await;
    let sid2 = again["session"]["sessionId"].as_str().unwrap().to_owned();
    let stopped = c.ok("claudeCode.stop", json!({"sessionId": sid2})).await;
    assert_eq!(stopped["state"], "exited");
    let r = c.call("claudeCode.send", json!({"sessionId": sid2, "text": "x"})).await;
    assert_eq!(r["error"]["code"], "conflict");

    // Login in a terminal runs `claude auth login` in an Orkestr terminal.
    let login = c.ok("claudeCode.loginInTerminal", json!({"projectId": pid})).await;
    assert!(login["terminal"]["terminalId"].is_string());

    core.shutdown().await;
}
