//! End-to-end `teams.*` and `pm.*` over a real WebSocket with the offline
//! Mock provider: seeded teams, CRUD, assignment with a permission scope,
//! AI Product Manager draft → edit → approve into a task hierarchy, events.

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
async fn teams_and_product_manager_over_ws_with_the_mock_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;
    let pid = c
        .ok("projects.create", json!({"name": "Shop", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}))
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
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}"), "global"]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // Seeded teams.
    let teams = c.ok("teams.list", json!({"projectId": pid})).await["teams"].as_array().unwrap().clone();
    let names: Vec<&str> = teams.iter().map(|t| t["team"]["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ERP Team", "Web Team", "Mobile Team", "Security Team", "Rapid Prototype Team"]);
    let web = teams[1]["team"]["id"].as_str().unwrap().to_owned();
    assert_eq!(teams[1]["team"]["policies"]["hierarchy"], true);
    let backend = teams[4]["team"]["members"][0]["templateId"].clone();

    // CRUD.
    let created = c
        .ok(
            "teams.create",
            json!({"name": "Payments", "members": [{"templateId": backend, "isLead": false, "count": 2, "model": null}]}),
        )
        .await;
    let tid = created["team"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["team"]["builtin"], false);
    c.wait_events("TeamUpdated", 1).await;
    assert_eq!(c.err("teams.create", json!({"name": " ", "members": []})).await["code"], "bad_request");
    let updated = c
        .ok(
            "teams.update",
            json!({"teamId": tid, "name": "Payments Team", "policies": {"maxFanOut": 3, "requireReview": false}}),
        )
        .await;
    assert_eq!(updated["team"]["name"], "Payments Team");
    assert_eq!(updated["team"]["policies"]["maxFanOut"], 3);
    assert_eq!(c.ok("teams.get", json!({"teamId": tid})).await["team"]["name"], "Payments Team");

    // Assignment with a permission scope (secrets denied by default).
    let assigned = c.ok("teams.assign", json!({"teamId": tid, "projectId": pid})).await;
    assert_eq!(assigned["assignments"][0]["permissions"]["secrets"], false);
    let allowed = c
        .ok(
            "teams.assign",
            json!({"teamId": tid, "projectId": pid, "permissions": {"secrets": true, "denied": ["Network"]}}),
        )
        .await;
    assert_eq!(allowed["assignments"][0]["permissions"]["denied"][0], "Network");
    let un = c.ok("teams.assign", json!({"teamId": tid, "projectId": pid, "unassign": true})).await;
    assert!(un["assignments"].as_array().unwrap().is_empty());
    c.ok("teams.delete", json!({"teamId": tid})).await;
    c.wait_events("TeamDeleted", 1).await;
    assert_eq!(c.err("teams.get", json!({"teamId": tid})).await["code"], "not_found");

    // AI Product Manager: draft → edit → approve.
    let draft = c.ok("pm.draft", json!({"projectId": pid, "idea": "a payroll module for small shops"})).await;
    let did = draft["id"].as_str().unwrap().to_owned();
    assert_eq!(draft["status"], "Draft");
    assert_eq!(draft["epic"]["features"].as_array().unwrap().len(), 2);
    c.wait_events("PmDraftUpdated", 1).await;
    assert_eq!(c.err("pm.draft", json!({"projectId": pid, "idea": ""})).await["code"], "bad_request");
    let mut epic = draft["epic"].clone();
    epic["features"][0]["stories"][0]["tasks"][0]["dependsOn"] = json!(["t4"]);
    assert_eq!(c.err("pm.update", json!({"draftId": did, "epic": epic})).await["code"], "bad_request", "cycle");
    let mut epic = draft["epic"].clone();
    epic["title"] = json!("Payroll v1");
    assert_eq!(c.ok("pm.update", json!({"draftId": did, "epic": epic})).await["epic"]["title"], "Payroll v1");
    let list = c.ok("pm.list", json!({"projectId": pid})).await;
    assert_eq!(list["drafts"].as_array().unwrap().len(), 1);

    let approved = c.ok("pm.approve", json!({"draftId": did, "startRun": false, "teamId": web})).await;
    assert_eq!(approved["draft"]["status"], "Approved");
    assert_eq!(approved["plan"]["status"], "Approved");
    assert_eq!(approved["plan"]["options"]["teamId"], web.as_str());
    assert!(approved["run"].is_null());
    let tasks = c.ok("tasks.list", json!({"projectId": pid})).await["tasks"].as_array().unwrap().clone();
    assert_eq!(tasks.len(), 1 + 2 + 3 + 4, "epic + features + stories + tasks");
    let epic_id = approved["draft"]["epic"]["taskId"].as_str().unwrap();
    let children = tasks.iter().filter(|t| t["parentId"] == epic_id).count();
    assert_eq!(children, 2, "features are children of the epic");
    let got = c.ok("pm.get", json!({"draftId": did})).await;
    assert_eq!(got["progress"]["total"], 4);
    assert_eq!(c.err("pm.approve", json!({"draftId": did})).await["code"], "conflict");
    assert_eq!(c.err("pm.discard", json!({"draftId": did})).await["code"], "conflict");
    assert_eq!(c.err("pm.get", json!({"draftId": "nope"})).await["code"], "not_found");
    core.shutdown().await;
}
