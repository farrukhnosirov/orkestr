//! End-to-end: `terminal.*` over the real WebSocket, with events.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(windows)]
const SHELL: &str = "cmd";
#[cfg(not(windows))]
const SHELL: &str = "sh";
#[cfg(windows)]
const SIX_TIMES_SEVEN: &str = "set /a 6*7\r";
#[cfg(not(windows))]
const SIX_TIMES_SEVEN: &str = "echo $((6*7))\r";

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

struct Client {
    ws: Ws,
    events: Vec<Value>,
    next: u64,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        send(&mut ws, json!({"type": "auth", "token": core.token})).await;
        let res = recv(&mut ws, Duration::from_secs(5)).await;
        assert_eq!(res["ok"], true, "{res}");
        Self { ws, events: Vec::new(), next: 0 }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        send(&mut self.ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
        loop {
            let v = recv(&mut self.ws, Duration::from_secs(30)).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method} failed: {res}");
        res["result"].clone()
    }

    async fn err_code(&mut self, method: &str, params: Value) -> String {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} unexpectedly succeeded: {res}");
        res["error"]["code"].as_str().unwrap().to_owned()
    }

    async fn subscribe(&mut self, topic: &str) {
        self.next += 1;
        let id = format!("s{}", self.next);
        send(&mut self.ws, json!({"type": "sub", "id": id, "topics": [topic]})).await;
        loop {
            let v = recv(&mut self.ws, Duration::from_secs(5)).await;
            if v["type"] == "res" && v["id"] == id {
                return;
            }
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }

    /// Waits until the collected events satisfy `pred`.
    async fn wait_events(&mut self, timeout: Duration, pred: impl Fn(&[Value]) -> bool) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !pred(&self.events) {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!left.is_zero(), "timeout waiting for events: {:?}", self.events);
            let v = recv(&mut self.ws, left).await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

fn output_of(events: &[Value], terminal_id: &str) -> String {
    events
        .iter()
        .filter(|e| e["event"]["kind"] == "TerminalOutput" && e["event"]["payload"]["terminalId"] == terminal_id)
        .map(|e| e["event"]["payload"]["data"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn terminal_rpc_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("sub")).unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Shells.
    let shells = c.ok("terminal.shells", json!({})).await;
    let list = shells["shells"].as_array().unwrap();
    assert!(!list.is_empty());
    assert_eq!(list[0]["isDefault"], true);
    assert!(list.iter().any(|s| s["id"] == SHELL), "{list:?}");

    let project = c.ok("projects.create", json!({"name": "Term", "rootPath": root.path().to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();

    // run (user): allowed, confined.
    let out = c.ok("terminal.run", json!({"projectId": pid, "shellId": SHELL, "command": "echo rpc-run"})).await;
    assert_eq!(out["exitCode"], 0, "{out}");
    assert_eq!(out["stdout"].as_str().unwrap().trim(), "rpc-run");
    assert_eq!(out["timedOut"], false);
    assert_eq!(
        c.err_code("terminal.run", json!({"projectId": pid, "shellId": SHELL, "command": "echo x", "cwd": ".."})).await,
        "forbidden"
    );
    // run (agent): the default gate requires approval until security is wired.
    let agent = orkestr_domain::AgentId::new().to_string();
    let res = c
        .call("terminal.run", json!({"projectId": pid, "agentId": agent, "shellId": SHELL, "command": "echo hi"}))
        .await;
    assert_eq!(res["error"]["code"], "approval_required", "{res}");

    // Interactive PTY.
    let opened = c
        .ok(
            "terminal.open",
            json!({"projectId": pid, "shellId": SHELL, "cwd": "sub", "cols": 100, "rows": 30, "title": "Main"}),
        )
        .await;
    let term = opened["terminal"].clone();
    let tid = term["terminalId"].as_str().unwrap().to_owned();
    assert_eq!(term["status"], "running");
    assert_eq!(term["readOnly"], false);
    assert_eq!(term["owner"]["kind"], "user");
    assert_eq!(term["title"], "Main");
    assert!(term["cwd"].as_str().unwrap().ends_with("sub"));

    c.subscribe(&format!("terminal:{tid}")).await;
    c.ok("terminal.write", json!({"terminalId": tid, "data": SIX_TIMES_SEVEN})).await;
    let t2 = tid.clone();
    c.wait_events(Duration::from_secs(20), move |ev| output_of(ev, &t2).contains("42")).await;
    let first = c.events.iter().find(|e| e["event"]["kind"] == "TerminalOutput").unwrap();
    assert!(first["event"]["payload"]["offset"].is_u64(), "{first}");

    let hist = c.ok("terminal.history", json!({"terminalId": tid})).await;
    assert!(hist["data"].as_str().unwrap().contains("42"));
    assert_eq!(
        hist["endOffset"].as_u64().unwrap(),
        hist["startOffset"].as_u64().unwrap() + hist["data"].as_str().unwrap().len() as u64
    );

    c.ok("terminal.resize", json!({"terminalId": tid, "cols": 140, "rows": 40})).await;
    assert_eq!(c.err_code("terminal.resize", json!({"terminalId": tid, "cols": 0, "rows": 40})).await, "bad_request");
    let renamed = c.ok("terminal.rename", json!({"terminalId": tid, "title": "Build"})).await;
    assert_eq!(renamed["terminal"]["title"], "Build");
    assert_eq!(renamed["terminal"]["cols"], 140);

    // The PTY plus the agent's command view (it shows the "awaiting approval" notice).
    let listed = c.ok("terminal.list", json!({"projectId": pid})).await;
    let sessions = listed["terminals"].as_array().unwrap().clone();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    let view = sessions.iter().find(|s| s["kind"] == "commandView").expect("agent command view");
    assert_eq!(view["owner"]["kind"], "agent");
    assert_eq!(view["readOnly"], true);
    let view_id = view["terminalId"].as_str().unwrap().to_owned();
    let view_hist = c.ok("terminal.history", json!({"terminalId": view_id})).await;
    assert!(view_hist["data"].as_str().unwrap().contains("awaiting approval"), "{view_hist}");
    c.ok("terminal.close", json!({"terminalId": view_id})).await;

    // Agent-owned PTY: read-only for the user; execLine is gated (approval needed by default).
    let agent_term = c.ok("terminal.open", json!({"projectId": pid, "shellId": SHELL, "agentId": agent})).await;
    let atid = agent_term["terminal"]["terminalId"].as_str().unwrap().to_owned();
    assert_eq!(agent_term["terminal"]["readOnly"], true);
    assert_eq!(c.err_code("terminal.write", json!({"terminalId": atid, "data": "dir\r"})).await, "forbidden");
    assert_eq!(c.err_code("terminal.execLine", json!({"terminalId": atid, "line": "dir"})).await, "approval_required");
    assert_eq!(c.err_code("terminal.execLine", json!({"terminalId": tid, "line": "dir"})).await, "forbidden");
    c.ok("terminal.close", json!({"terminalId": atid})).await;

    // Close the user PTY: TerminalExited arrives, the session is gone.
    c.ok("terminal.close", json!({"terminalId": tid})).await;
    let t3 = tid.clone();
    c.wait_events(Duration::from_secs(15), move |ev| {
        ev.iter().any(|e| e["event"]["kind"] == "TerminalExited" && e["event"]["payload"]["terminalId"] == t3)
    })
    .await;
    assert_eq!(c.err_code("terminal.history", json!({"terminalId": tid})).await, "not_found");
    let listed = c.ok("terminal.list", json!({})).await;
    assert!(listed["terminals"].as_array().unwrap().is_empty());

    // Validation.
    assert_eq!(c.err_code("terminal.open", json!({"shellId": "no-such-shell"})).await, "bad_request");
    assert_eq!(c.err_code("terminal.open", json!({"agentId": agent})).await, "bad_request");

    // Command history was recorded (redacted) in SQLite.
    let rows: i64 = core
        .services
        .db
        .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM terminal_commands", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert!(rows >= 3, "expected history rows, got {rows}");

    core.shutdown().await;
}

#[tokio::test]
async fn shutdown_kills_open_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut c = Client::connect(&core).await;
    let root = tempfile::tempdir().unwrap();
    let opened = c.ok("terminal.open", json!({"shellId": SHELL, "cwd": root.path().to_string_lossy()})).await;
    let pid = opened["terminal"]["pid"].as_u64().unwrap() as u32;
    let services = std::sync::Arc::clone(&core.services);
    core.shutdown().await;
    assert!(services.terminals.list(None).is_empty());
    tokio::time::sleep(Duration::from_millis(500)).await;
    use orkestr_platform::ProcessControl;
    assert!(!orkestr_platform::NativeProcessControl.is_alive(pid), "terminal shell survived core shutdown");
}
