//! End-to-end `manager.*` over a real WebSocket with the offline Mock provider:
//! chat → plan card → approve → supervised run → report → resume summary.

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
            tokio::time::timeout(Duration::from_secs(20), ws.next()).await.expect("timeout").expect("closed").unwrap();
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

fn node_available() -> bool {
    std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

async fn wait_run(c: &mut Client, run_id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let runs = c.ok("manager.status", json!({"runId": run_id})).await["runs"].clone();
        let run = runs[0].clone();
        let review = run["tasks"].as_array().unwrap().iter().any(|t| t["awaitingReview"] == true);
        if matches!(run["status"].as_str(), Some("Completed" | "Failed" | "Cancelled")) || review {
            return run;
        }
        assert!(tokio::time::Instant::now() < deadline, "run did not finish: {run}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn manager_chat_plan_approve_run_over_ws_with_the_mock_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("README.md"),
        "# demo
",
    )
    .unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c
        .ok(
            "projects.create",
            json!({"name": "Managed", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}),
        )
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    c.ok(
        "providers.setManager",
        json!({"config": {"model": model, "maxParallelAgents": 2, "approvalPolicy": "SemiAuto"}}),
    )
    .await;
    // Agents are routed to the offline Mock too (never a real CLI in tests).
    c.ok(
        "providers.setRouting",
        json!({"config": {"mode": "Manual", "rules": [], "defaultFallback": [model], "preferLocal": true, "maxRateLimitWaitSecs": 0}}),
    )
    .await;
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]}), "s1").await;
    assert_eq!(sub["ok"], true);

    let empty = c.ok("manager.resumeSummary", json!({"projectId": pid})).await;
    assert_eq!(empty["isEmpty"], true, "{empty}");

    // Chat: the reply streams and proposes a plan.
    let sent = c.ok("manager.chat", json!({"projectId": pid, "text": "Create a hello module with tests"})).await;
    assert_eq!(sent["userMessage"]["role"], "User");
    let reply_id = sent["replyMessageId"].as_str().unwrap().to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let reply = loop {
        let found = c.events.iter().find(|e| {
            e["event"]["kind"] == "ManagerMessageAdded" && e["event"]["payload"]["message"]["id"] == reply_id.as_str()
        });
        if let Some(e) = found {
            break e["event"]["payload"]["message"].clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "no reply");
        let v = recv(&mut c.ws).await;
        if v["type"] == "event" {
            c.events.push(v);
        }
    };
    assert_eq!(reply["role"], "Manager", "{reply}");
    let plan_id = reply["planId"].as_str().unwrap().to_owned();
    c.wait_events("ManagerMessage", 1).await;
    c.wait_events("ManagerPlanUpdated", 1).await;
    let hist = c.ok("manager.history", json!({"projectId": pid})).await;
    assert_eq!(hist["plans"][0]["id"], plan_id.as_str());
    assert_eq!(hist["plans"][0]["tasks"].as_array().unwrap().len(), 4);
    assert_eq!(hist["config"]["maxParallelAgents"], 2);
    assert!(hist["messages"].as_array().unwrap().len() >= 2);
    assert_eq!(c.err("manager.chat", json!({"projectId": pid, "text": ""})).await["code"], "bad_request");

    // Approve → tasks + run.
    let approved = c.ok("manager.approvePlan", json!({"planId": plan_id})).await;
    assert_eq!(approved["plan"]["status"], "Running");
    let run_id = approved["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(c.err("manager.approvePlan", json!({"planId": plan_id})).await["code"], "conflict");
    let tasks = c.ok("tasks.list", json!({"projectId": pid})).await["tasks"].as_array().unwrap().clone();
    assert_eq!(tasks.len(), 4);
    let run = wait_run(&mut c, &run_id).await;
    let done: Vec<&Value> = run["tasks"].as_array().unwrap().iter().filter(|t| t["status"] == "Completed").collect();
    assert!(done.len() >= 3, "{run}");
    if node_available() {
        assert_eq!(run["status"], "Completed", "{run}");
        assert_eq!(run["report"]["completed"], 4);
        assert!(root.join("src/hello.mjs").is_file(), "agents share the (non-git) checkout");
    }
    c.wait_events("ManagerRunUpdated", 1).await;
    let summary = c.ok("manager.resumeSummary", json!({"projectId": pid})).await;
    assert!(summary["tasksCompleted"].as_u64().unwrap() >= 3, "{summary}");
    assert!(summary["headline"].as_str().unwrap().starts_with("Last session:"), "{summary}");

    // Errors.
    assert_eq!(c.err("manager.run", json!({})).await["code"], "bad_request");
    assert_eq!(c.err("manager.pause", json!({"runId": run_id})).await["code"], "conflict");
    assert_eq!(
        c.err("manager.rejectPlan", json!({"planId": "0192a000-0000-7000-8000-000000000000"})).await["code"],
        "not_found"
    );
    let cleared = c.ok("manager.clear", json!({"projectId": pid})).await;
    assert!(cleared["archived"]["summary"].as_str().unwrap().contains("hello module"));
    core.shutdown().await;
}
