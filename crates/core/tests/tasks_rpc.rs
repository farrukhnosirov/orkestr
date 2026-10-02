//! End-to-end `tasks.*` over a real WebSocket, plus crash recovery of tasks.

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

#[tokio::test]
async fn tasks_crud_transitions_dependencies_graph_over_ws() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c.ok("projects.create", json!({"name": "Payroll"})).await["project"].clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // Create A, B (depends on A).
    let a = c
        .ok(
            "tasks.create",
            json!({"projectId": pid, "title": "Schema", "priority": "High", "acceptanceCriteria": ["migrations reversible"]}),
        )
        .await["task"]
        .clone();
    let aid = a["id"].as_str().unwrap().to_owned();
    assert_eq!(a["status"], "Planned");
    assert_eq!(a["acceptanceCriteria"][0]["met"], false);
    let b = c.ok("tasks.create", json!({"projectId": pid, "title": "API", "dependsOn": [aid]})).await["task"].clone();
    let bid = b["id"].as_str().unwrap().to_owned();
    assert_eq!(b["dependencies"], json!([aid]));
    let created = c.wait_events("TaskCreated", 2).await;
    assert_eq!(created[0]["event"]["projectId"], pid);
    assert_eq!(created[0]["event"]["taskId"], aid);

    // Cycle → conflict with the path.
    let e = c.err("tasks.addDependency", json!({"taskId": aid, "dependsOn": bid})).await;
    assert_eq!(e["code"], "conflict");
    assert_eq!(e["data"]["cycle"], json!([aid, bid, aid]));

    // Guard: B cannot be Ready yet. Invalid transition carries allowed list.
    let e = c.err("tasks.transition", json!({"taskId": bid, "to": "Ready"})).await;
    assert_eq!(e["code"], "conflict");
    assert_eq!(e["data"]["guard"], "dependencies_completed");
    let e = c.err("tasks.transition", json!({"taskId": aid, "to": "Completed"})).await;
    assert_eq!(e["data"]["allowed"], json!(["Ready", "Blocked", "Cancelled"]));

    // Drive A to Completed → B is auto-promoted.
    for to in ["Ready", "Running", "Testing"] {
        c.ok("tasks.transition", json!({"taskId": aid, "to": to})).await;
    }
    let done = c.ok("tasks.transition", json!({"taskId": aid, "to": "Completed", "reason": "all green"})).await;
    assert_eq!(done["task"]["status"], "Completed");
    assert_eq!(done["changes"].as_array().unwrap().len(), 2);
    assert_eq!(done["changes"][1]["taskId"], bid);
    assert_eq!(done["changes"][1]["to"], "Ready");
    let changed = c.wait_events("TaskStatusChanged", 5).await;
    let last = &changed[4]["event"];
    assert_eq!(last["taskId"], bid);
    assert_eq!(last["projectId"], pid);
    assert_eq!(last["payload"]["reason"], "dependencies_completed");

    // Optimistic concurrency on update.
    let got = c.ok("tasks.get", json!({"taskId": bid})).await["task"].clone();
    let v = got["version"].as_u64().unwrap();
    c.ok("tasks.update", json!({"taskId": bid, "expectedVersion": v, "patch": {"title": "Public API"}})).await;
    let e = c.err("tasks.update", json!({"taskId": bid, "expectedVersion": v, "patch": {"title": "x"}})).await;
    assert_eq!(e["code"], "conflict");
    assert_eq!(e["data"]["currentVersion"], v + 1);

    // List filters, graph, history.
    let ready = c.ok("tasks.list", json!({"projectId": pid, "status": ["Ready"]})).await;
    assert_eq!(ready["tasks"].as_array().unwrap().len(), 1);
    let graph = c.ok("tasks.graph", json!({"projectId": pid})).await;
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"], json!([{"dependsOn": aid, "taskId": bid, "kind": "FinishToStart"}]));
    assert_eq!(graph["ready"], json!([bid]));
    assert_eq!(graph["topoOrder"], json!([aid, bid]));
    assert_eq!(graph["criticalPath"], json!([bid]));
    let hist = c.ok("tasks.history", json!({"taskId": aid})).await;
    assert_eq!(hist["entries"].as_array().unwrap().len(), 5);
    assert_eq!(hist["entries"][4]["reason"], "all green");
    assert_eq!(hist["entries"][4]["actor"], json!({"type": "User"}));
    assert_eq!(hist["executions"][0]["status"], "Succeeded");

    // Retry, delete rules, remove dependency.
    let e = c.err("tasks.retry", json!({"taskId": bid})).await;
    assert_eq!(e["code"], "conflict");
    let e = c.err("tasks.delete", json!({"taskId": bid})).await;
    assert_eq!(e["code"], "conflict");
    c.ok("tasks.removeDependency", json!({"taskId": bid, "dependsOn": aid})).await;
    let e = c.err("tasks.removeDependency", json!({"taskId": bid, "dependsOn": aid})).await;
    assert_eq!(e["code"], "not_found");
    c.ok("tasks.transition", json!({"taskId": bid, "to": "Cancelled"})).await;
    let del = c.ok("tasks.delete", json!({"taskId": bid})).await;
    assert_eq!(del["taskId"], bid);
    let deleted = c.wait_events("TaskDeleted", 1).await;
    assert_eq!(deleted[0]["event"]["payload"]["taskId"], bid);
    assert_eq!(c.err("tasks.get", json!({"taskId": bid})).await["code"], "not_found");

    // Bad input.
    assert_eq!(c.err("tasks.create", json!({"projectId": pid, "title": ""})).await["code"], "bad_request");
    assert_eq!(c.err("tasks.transition", json!({"taskId": aid, "to": "Bogus"})).await["code"], "bad_request");

    core.shutdown().await;
}

#[tokio::test]
async fn crash_recovery_interrupts_in_flight_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let (pid, running, waiting, ready) = {
        let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
        let mut c = Client::connect(&core).await;
        let pid =
            c.ok("projects.create", json!({"name": "Crashy"})).await["project"]["id"].as_str().unwrap().to_owned();
        let mut ids = Vec::new();
        for (title, path) in [
            ("running", &["Ready", "Running"][..]),
            ("waiting", &["Ready", "Running", "Waiting"][..]),
            ("ready", &["Ready"][..]),
        ] {
            let id = c.ok("tasks.create", json!({"projectId": pid, "title": title})).await["task"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            for to in path {
                c.ok("tasks.transition", json!({"taskId": id, "to": to})).await;
            }
            ids.push(id);
        }
        // Simulate a crash: drop without shutdown (session marker stays), then
        // rewrite the marker with a dead pid so the next start sees a crash.
        drop(c);
        drop(core);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = std::fs::remove_file(dir.path().join("core.json"));
        std::fs::write(dir.path().join("session.lock"), r#"{"pid":4294967284,"startedAt":"2026-01-01T00:00:00Z"}"#)
            .unwrap();
        (pid, ids[0].clone(), ids[1].clone(), ids[2].clone())
    };

    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    assert!(core.services.recovered_from_crash);
    let mut c = Client::connect(&core).await;
    for (id, expected) in [(&running, "Interrupted"), (&waiting, "Interrupted"), (&ready, "Ready")] {
        let t = c.ok("tasks.get", json!({"taskId": id})).await["task"].clone();
        assert_eq!(t["status"], expected, "{id}");
    }
    let hist = c.ok("tasks.history", json!({"taskId": waiting})).await;
    let last = hist["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["actor"], json!({"type": "SystemRecovery"}));
    assert_eq!(last["forced"], true);
    assert_eq!(last["reason"], "crash_recovery");
    // Events were recorded before the server accepted connections.
    let replay = c.ok("events.replay", json!({"topics": [format!("project:{pid}")]})).await;
    let recovered = replay["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "TaskStatusChanged" && e["payload"]["to"] == "Interrupted")
        .count();
    assert_eq!(recovered, 2);
    // Clients cannot use the privileged edges.
    let id =
        c.ok("tasks.create", json!({"projectId": pid, "title": "x"})).await["task"]["id"].as_str().unwrap().to_owned();
    for to in ["Ready", "Running", "Review"] {
        c.ok("tasks.transition", json!({"taskId": id, "to": to})).await;
    }
    assert_eq!(c.err("tasks.transition", json!({"taskId": id, "to": "Interrupted"})).await["code"], "conflict");
    // Resume an interrupted task.
    let r = c.ok("tasks.retry", json!({"taskId": running})).await;
    assert_eq!(r["task"]["status"], "Ready");

    // Graceful shutdown interrupts in-flight work too.
    core.shutdown().await;
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    assert!(!core.services.recovered_from_crash);
    let t = core.services.tasks.get(id.parse().unwrap()).await.unwrap();
    assert_eq!(t.status, orkestr_domain::TaskStatus::Interrupted);
    core.shutdown().await;
}
