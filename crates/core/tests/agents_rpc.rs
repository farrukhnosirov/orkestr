//! End-to-end `templates.*` + `agents.*` over a real WebSocket with the offline Mock provider.

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
            tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

struct Client {
    ws: Ws,
    next: u64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        send(&mut ws, json!({"type": "auth", "token": core.token})).await;
        let res = recv(&mut ws).await;
        assert_eq!(res["ok"], true, "{res}");
        Self { ws, next: 0, events: Vec::new() }
    }

    async fn raw(&mut self, frame: Value, id: &str) -> Value {
        send(&mut self.ws, frame).await;
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.raw(json!({"type": "req", "id": id, "method": method, "params": params}), &id).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method}: {res}");
        res["result"].clone()
    }

    async fn err(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} should fail: {res}");
        res["error"].clone()
    }

    /// Waits until `n` events of `kind` were received.
    async fn wait_events(&mut self, kind: &str, n: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let found: Vec<Value> = self.events.iter().filter(|e| e["event"]["kind"] == kind).cloned().collect();
            if found.len() >= n {
                return found;
            }
            assert!(tokio::time::Instant::now() < deadline, "only {} {kind} events", found.len());
            let v = recv(&mut self.ws).await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

async fn wait_agent_final(c: &mut Client, id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let a = c.ok("agents.get", json!({"agentId": id})).await["agent"].clone();
        if matches!(a["state"].as_str(), Some("Completed" | "Error" | "Stopped")) && a["stoppedAt"].is_string() {
            return a;
        }
        assert!(tokio::time::Instant::now() < deadline, "agent did not finish: {a}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn templates_and_agents_over_ws_with_the_mock_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Built-in templates are seeded and read-only.
    let list = c.ok("templates.list", json!({})).await["templates"].as_array().unwrap().clone();
    assert_eq!(list.len(), 11);
    let backend = list.iter().find(|t| t["role"] == "Backend").unwrap().clone();
    assert_eq!(backend["source"], "Builtin");
    let e = c.err("templates.delete", json!({"templateId": backend["id"]})).await;
    assert_eq!(e["code"], "forbidden");

    // Export → import (permissions dropped unless accepted) → update → delete.
    let exported = c.ok("templates.export", json!({"templateId": backend["id"]})).await;
    assert_eq!(exported["fileName"], "backend-engineer.agent.json");
    let imported = c.ok("templates.import", json!({"json": exported["json"]})).await;
    assert_eq!(imported["template"]["source"], "Custom");
    assert!(!imported["droppedPermissions"].as_array().unwrap().is_empty());
    let tid = imported["template"]["id"].as_str().unwrap().to_owned();
    let mut draft = imported["template"].clone();
    for k in ["id", "source", "createdAt", "updatedAt"] {
        draft.as_object_mut().unwrap().remove(k);
    }
    draft["tools"] = json!(["write_file", "read_file", "submit_result"]);
    draft["qualityGates"] = json!([]);
    draft["worktreePolicy"] = "SharedWorktree".into();
    let updated = c.ok("templates.update", json!({"templateId": tid, "template": draft})).await;
    assert_eq!(updated["template"]["worktreePolicy"], "SharedWorktree");
    let e = c.err("templates.import", json!({"json": "{\"format\":\"nope\"}"})).await;
    assert_eq!(e["code"], "bad_request");

    // Project + Mock provider.
    let project = c
        .ok(
            "projects.create",
            json!({"name": "Agents", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}),
        )
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // Spawn on a new task with an explicit (offline) model.
    let spawned = c
        .ok(
            "agents.spawn",
            json!({
                "projectId": pid,
                "templateId": tid,
                "newTask": {"title": "Write a hello file", "acceptanceCriteria": ["file exists"]},
                "model": {"providerId": mock, "model": "mock-smart"}
            }),
        )
        .await;
    let aid = spawned["agent"]["id"].as_str().unwrap().to_owned();
    assert_eq!(spawned["task"]["agentId"], aid);
    assert_eq!(spawned["agent"]["worktree"]["path"], root.to_string_lossy().as_ref());
    let a = wait_agent_final(&mut c, &aid).await;
    assert_eq!(a["state"], "Completed", "{a}");
    c.wait_events("AgentSpawned", 1).await;
    c.wait_events("AgentStopped", 1).await;

    let tr = c.ok("agents.transcript", json!({"agentId": aid})).await;
    assert!(tr["toolCalls"].as_array().unwrap().iter().any(|t| t["toolCall"]["tool"] == "write_file"));
    assert!(!tr["messages"].as_array().unwrap().is_empty());
    let hs = c.ok("agents.handoff", json!({"agentId": aid})).await["handoffs"].as_array().unwrap().clone();
    assert_eq!(hs[0]["reason"], "Completed");
    let stats = c.ok("agents.stats", json!({"projectId": pid})).await;
    assert_eq!(stats["total"], 1);
    assert!(stats["toolCalls"].as_u64().unwrap() >= 2);
    let listed = c.ok("agents.list", json!({"projectId": pid})).await["agents"].as_array().unwrap().len();
    assert_eq!(listed, 1);
    let task = c.ok("tasks.get", json!({"taskId": spawned["task"]["id"]})).await["task"].clone();
    assert_eq!(
        task["status"], "Completed",
        "mock-smart writes the file, then submits a done result (no quality gates)"
    );

    // Stop on a finished agent is idempotent; pause on a finished agent conflicts.
    let stopped = c.ok("agents.stop", json!({"agentId": aid})).await;
    assert_eq!(stopped["agent"]["state"], "Stopped");
    assert_eq!(c.err("agents.pause", json!({"agentId": aid})).await["code"], "conflict");
    assert_eq!(
        c.err("agents.get", json!({"agentId": "0192a000-0000-7000-8000-000000000000"})).await["code"],
        "not_found"
    );
    core.shutdown().await;
}
