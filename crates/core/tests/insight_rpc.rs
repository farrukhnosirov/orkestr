//! End-to-end `architecture.*` and `replay.*` over a real WebSocket:
//! a fixture repository is mapped, changed, refreshed and diffed; then a
//! Manager run with the offline Mock provider is replayed (runs → timeline →
//! agent filter → step-through).

use std::path::Path;
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
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A small polyglot repository: Cargo workspace (api + shared), an npm web
/// app, docker-compose with postgres + redis, SQL migrations, OpenAPI.
fn fixture(root: &Path) {
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"crates/api\", \"crates/shared\"]\n");
    write(
        root,
        "crates/api/Cargo.toml",
        "[package]\nname = \"api\"\nversion = \"0.1.0\"\n\n[dependencies]\nshared = { path = \"../shared\" }\naxum = \"0.8\"\nsqlx = \"0.8\"\n",
    );
    write(
        root,
        "crates/api/src/main.rs",
        "use axum::{routing::get, Router};\nfn main() {\n    let app = Router::new().route(\"/health\", get(health)).route(\"/users/{id}\", get(user));\n}\n",
    );
    write(
        root,
        "crates/shared/Cargo.toml",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
    );
    write(root, "crates/shared/src/lib.rs", "pub fn hello() {}\n");
    write(
        root,
        "web/package.json",
        "{\"name\": \"web\", \"version\": \"1.0.0\", \"dependencies\": {\"react\": \"^18\"}}\n",
    );
    write(root, "web/src/main.ts", "import React from 'react';\nexport const x = 1;\n");
    write(
        root,
        "docker-compose.yml",
        "services:\n  api:\n    build: .\n    ports:\n      - \"8080:8080\"\n    depends_on:\n      - db\n  db:\n    image: postgres:16\n  cache:\n    image: redis:7\n",
    );
    write(root, "migrations/0001_init.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);\n");
    write(
        root,
        "openapi.yaml",
        "openapi: 3.0.0\ninfo:\n  title: Demo\n  version: '1'\npaths:\n  /users:\n    get:\n      summary: list\n",
    );
}

fn node_ids(map: &Value) -> Vec<String> {
    map["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_owned()).collect()
}

fn kinds(map: &Value, kind: &str) -> usize {
    map["nodes"].as_array().unwrap().iter().filter(|n| n["kind"] == kind).count()
}

#[tokio::test]
async fn architecture_map_refresh_and_diff_over_ws() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    fixture(&root);
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;
    let pid = c.ok("projects.create", json!({"name": "Fixture", "rootPath": root.to_string_lossy()})).await["project"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // Diff before any snapshot → not_found; first map scans.
    assert_eq!(c.err("architecture.diff", json!({"projectId": pid})).await["code"], "not_found");
    let first = c.ok("architecture.map", json!({"projectId": pid})).await;
    let map = &first["map"];
    let ids = node_ids(map);
    assert!(ids.iter().any(|i| i.contains("api")), "{ids:?}");
    assert!(ids.iter().any(|i| i.contains("shared")), "{ids:?}");
    assert!(ids.iter().any(|i| i.contains("web")), "{ids:?}");
    assert!(kinds(map, "Module") >= 3, "{map}");
    assert!(kinds(map, "Service") >= 1, "{map}");
    assert!(kinds(map, "Database") >= 2, "postgres + redis (+ migrations): {map}");
    assert!(kinds(map, "Api") >= 1, "{map}");
    assert!(kinds(map, "External") >= 1, "{map}");
    assert!(map["stats"]["routes"].as_u64().unwrap() >= 2, "{}", map["stats"]);
    let edges = map["edges"].as_array().unwrap();
    assert!(
        edges.iter().any(|e| e["kind"] == "DependsOn"
            && e["from"].as_str().unwrap().contains("api")
            && e["to"].as_str().unwrap().contains("shared")),
        "api → shared manifest edge: {edges:?}"
    );
    assert_eq!(first["snapshots"].as_array().unwrap().len(), 1);
    let first_id = map["snapshotId"].as_str().unwrap().to_owned();

    // A second map call reuses the snapshot; an unchanged refresh stores nothing.
    let again = c.ok("architecture.map", json!({"projectId": pid})).await;
    assert_eq!(again["map"]["snapshotId"], first_id.as_str());
    let same = c.ok("architecture.refresh", json!({"projectId": pid})).await;
    assert_eq!(same["changed"], false);

    // Add a module with a route → new snapshot + diff.
    write(
        &root,
        "crates/billing/Cargo.toml",
        "[package]\nname = \"billing\"\nversion = \"0.1.0\"\n\n[dependencies]\nshared = { path = \"../shared\" }\n",
    );
    write(&root, "crates/billing/src/lib.rs", "pub fn bill() {}\n");
    write(&root, "Cargo.toml", "[workspace]\nmembers = [\"crates/api\", \"crates/shared\", \"crates/billing\"]\n");
    let changed = c.ok("architecture.refresh", json!({"projectId": pid})).await;
    assert_eq!(changed["changed"], true, "{changed}");
    let diff = &changed["diff"];
    assert!(diff["addedNodes"].as_array().unwrap().iter().any(|n| n["name"] == "billing"), "billing added: {diff}");
    let second_id = changed["map"]["snapshotId"].as_str().unwrap().to_owned();
    assert_ne!(first_id, second_id);
    let d = c.ok("architecture.diff", json!({"projectId": pid})).await["diff"].clone();
    assert_eq!(d["fromSnapshotId"], first_id.as_str());
    assert_eq!(d["toSnapshotId"], second_id.as_str());
    let back =
        c.ok("architecture.diff", json!({"projectId": pid, "from": second_id, "to": first_id})).await["diff"].clone();
    assert!(back["removedNodes"].as_array().unwrap().iter().any(|n| n["name"] == "billing"), "{back}");
    let old = c.ok("architecture.map", json!({"projectId": pid, "snapshotId": first_id})).await;
    assert_eq!(old["map"]["snapshotId"], first_id.as_str());
    assert_eq!(old["snapshots"].as_array().unwrap().len(), 2);

    // Full refresh works; the change event was published.
    c.ok("architecture.refresh", json!({"projectId": pid, "full": true})).await;
    assert!(
        c.events.iter().any(|e| e["event"]["kind"] == "ArchitectureMapUpdated"),
        "ArchitectureMapUpdated published"
    );

    // Errors.
    assert_eq!(c.err("architecture.map", json!({"projectId": pid, "snapshotId": "nope"})).await["code"], "not_found");
    assert_eq!(c.err("architecture.refresh", json!({})).await["code"], "bad_request");
    let rootless = c.ok("projects.create", json!({"name": "NoRoot"})).await["project"]["id"].clone();
    assert_eq!(c.err("architecture.map", json!({"projectId": rootless})).await["code"], "bad_request");
    core.shutdown().await;
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
async fn replay_of_a_mock_manager_run_over_ws() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    write(&root, "README.md", "# demo\n");
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;
    let pid = c
        .ok(
            "projects.create",
            json!({"name": "Replay", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}),
        )
        .await["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    c.ok(
        "providers.setManager",
        json!({"config": {"model": model, "maxParallelAgents": 2, "approvalPolicy": "SemiAuto"}}),
    )
    .await;
    c.ok(
        "providers.setRouting",
        json!({"config": {"mode": "Manual", "rules": [], "defaultFallback": [model], "preferLocal": true, "maxRateLimitWaitSecs": 0}}),
    )
    .await;

    assert!(c.ok("replay.runs", json!({"projectId": pid})).await["runs"].as_array().unwrap().is_empty());

    let request = "Create a hello module with tests";
    let sent = c.ok("manager.chat", json!({"projectId": pid, "text": request})).await;
    let reply_id = sent["replyMessageId"].as_str().unwrap().to_owned();
    // Wait for the stored reply carrying the plan.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let plan_id = loop {
        let hist = c.ok("manager.history", json!({"projectId": pid})).await;
        let found = hist["messages"].as_array().unwrap().iter().find(|m| m["id"] == reply_id.as_str()).cloned();
        if let Some(plan) = found.and_then(|m| m["planId"].as_str().map(str::to_owned)) {
            break plan;
        }
        assert!(tokio::time::Instant::now() < deadline, "no plan");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let approved = c.ok("manager.approvePlan", json!({"planId": plan_id})).await;
    let run_id = approved["run"]["id"].as_str().unwrap().to_owned();
    let run = wait_run(&mut c, &run_id).await;

    let runs = c.ok("replay.runs", json!({"projectId": pid})).await["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["runId"], run_id.as_str());
    assert_eq!(runs[0]["tasks"].as_array().unwrap().len(), run["tasks"].as_array().unwrap().len());

    let tl = c.ok("replay.timeline", json!({"projectId": pid, "runId": run_id})).await["timeline"].clone();
    let steps = tl["steps"].as_array().unwrap();
    assert!(steps.len() >= 5, "{tl}");
    assert_eq!(steps[0]["phase"], "Request");
    assert_eq!(steps[0]["kind"], "UserRequest");
    assert_eq!(steps[0]["title"], request);
    let phases: Vec<&str> = tl["phases"].as_array().unwrap().iter().map(|p| p["phase"].as_str().unwrap()).collect();
    for want in ["Request", "Plan", "Agents", "Delegation", "Run"] {
        assert!(phases.contains(&want), "phase {want} missing in {phases:?}");
    }
    // Timestamps are monotonic and indexes dense.
    for (i, w) in steps.windows(2).enumerate() {
        assert!(w[0]["ts"].as_str().unwrap() <= w[1]["ts"].as_str().unwrap(), "order at {i}");
        assert_eq!(w[1]["index"].as_u64().unwrap(), i as u64 + 1);
    }
    let agents = tl["agents"].as_array().unwrap();
    assert!(!agents.is_empty(), "{tl}");
    assert!(steps.iter().any(|s| s["kind"] == "AgentSpawned"));
    assert!(steps.iter().any(|s| s["kind"] == "TaskCreated"));

    // Agent filter keeps only that agent's steps (or agent-less steps of its task).
    let agent = agents[0]["agentId"].as_str().unwrap().to_owned();
    let agent_task = agents[0]["taskId"].clone();
    let only =
        c.ok("replay.timeline", json!({"projectId": pid, "runId": run_id, "agentId": agent})).await["timeline"].clone();
    let only_steps = only["steps"].as_array().unwrap();
    assert!(!only_steps.is_empty());
    assert!(only_steps.len() < steps.len());
    for s in only_steps {
        assert!(s["agentId"] == agent.as_str() || (s["agentId"].is_null() && s["taskId"] == agent_task), "{s}");
    }
    // Phase filter.
    let plan_only = c.ok("replay.timeline", json!({"projectId": pid, "runId": run_id, "phases": ["Plan"]})).await
        ["timeline"]
        .clone();
    assert!(plan_only["steps"].as_array().unwrap().iter().all(|s| s["phase"] == "Plan"));

    // Step-through.
    let first = c.ok("replay.step", json!({"projectId": pid, "runId": run_id, "direction": "First"})).await;
    assert_eq!(first["index"], 0);
    assert_eq!(first["hasPrev"], false);
    assert_eq!(first["hasNext"], true);
    assert_eq!(first["step"]["kind"], "UserRequest");
    let next = c.ok("replay.step", json!({"projectId": pid, "runId": run_id, "index": 0, "direction": "Next"})).await;
    assert_eq!(next["index"], 1);
    let last = c.ok("replay.step", json!({"projectId": pid, "runId": run_id, "direction": "Last"})).await;
    assert_eq!(last["hasNext"], false);
    assert_eq!(last["total"].as_u64().unwrap() as usize, steps.len());
    assert!(!last["state"]["agents"].as_array().unwrap().is_empty(), "{last}");
    let tasks_at_end = last["state"]["tasks"].as_array().unwrap();
    assert!(tasks_at_end.iter().any(|t| t["status"] == "Completed"), "{last}");
    let jump = c
        .ok(
            "replay.step",
            json!({"projectId": pid, "runId": run_id, "index": 0, "direction": "NextPhase", "phase": "Agents"}),
        )
        .await;
    assert_eq!(jump["step"]["phase"], "Agents");

    // Task subject.
    let task_id = run["tasks"][0]["taskId"].as_str().unwrap().to_owned();
    let task_tl = c.ok("replay.timeline", json!({"projectId": pid, "taskId": task_id})).await["timeline"].clone();
    assert_eq!(task_tl["subject"], "Task");
    assert!(!task_tl["steps"].as_array().unwrap().is_empty(), "{task_tl}");

    // Errors.
    assert_eq!(c.err("replay.timeline", json!({"projectId": pid})).await["code"], "bad_request");
    assert_eq!(
        c.err("replay.timeline", json!({"projectId": pid, "runId": run_id, "taskId": task_id})).await["code"],
        "bad_request"
    );
    assert_eq!(
        c.err("replay.timeline", json!({"projectId": pid, "runId": "0192a000-0000-7000-8000-000000000000"})).await
            ["code"],
        "not_found"
    );
    let other = c.ok("projects.create", json!({"name": "Other"})).await["project"]["id"].clone();
    assert_eq!(c.err("replay.timeline", json!({"projectId": other, "runId": run_id})).await["code"], "not_found");
    core.shutdown().await;
}
