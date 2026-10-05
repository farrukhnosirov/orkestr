//! End-to-end `reviews.*` and `consensus.*` over a real WebSocket with the
//! offline Mock provider: code review → task moves Review → Testing, gate,
//! adversarial review with verification, resolving findings, consensus with a
//! Decision Log entry, events.

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

    async fn wait_events(&mut self, kind: &str, n: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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

fn git(args: &[&str], dir: &std::path::Path) -> bool {
    std::process::Command::new("git").args(args).current_dir(dir).output().is_ok_and(|o| o.status.success())
}

#[tokio::test]
async fn reviews_and_consensus_over_ws_with_the_mock_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let has_git = git(&["init", "-q", "-b", "main"], &root)
        && git(&["config", "user.name", "Test"], &root)
        && git(&["config", "user.email", "test@example.com"], &root)
        && git(&["add", "-A"], &root)
        && git(&["commit", "-q", "-m", "init"], &root);
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c
        .ok(
            "projects.create",
            json!({"name": "Reviewed", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}),
        )
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    c.ok("providers.setManager", json!({"config": {"model": model, "approvalPolicy": "SemiAuto"}})).await;
    c.ok(
        "providers.setRouting",
        json!({"config": {"mode": "Manual", "rules": [], "defaultFallback": [model], "preferLocal": true, "maxRateLimitWaitSecs": 0}}),
    )
    .await;
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // A task in Review whose change is an uncommitted file in the project folder.
    let task = c
        .ok(
            "tasks.create",
            json!({"projectId": pid, "title": "Add hello", "acceptanceCriteria": ["hello() greets by name"]}),
        )
        .await["task"]
        .clone();
    let tid = task["id"].as_str().unwrap().to_owned();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/hello.js"), "export function hello(name) {\n  return `Hello, ${name}!`;\n}\n")
        .unwrap();
    for to in ["Ready", "Running", "Review"] {
        c.ok("tasks.transition", json!({"taskId": tid, "to": to})).await;
    }

    // reviews.request (wait) → approve → Testing.
    let res = c.ok("reviews.request", json!({"taskId": tid, "wait": true})).await;
    let review = res["review"].clone();
    let rid = review["id"].as_str().unwrap().to_owned();
    assert_eq!(review["status"], "Finished", "{review}");
    assert_eq!(review["kind"], "Standard");
    if has_git {
        assert_eq!(review["verdict"], "Approve", "{review}");
        assert_eq!(review["transitionedTo"], "Testing");
        assert_eq!(review["acCheck"][0]["met"], true);
        let t = c.ok("tasks.get", json!({"taskId": tid})).await["task"].clone();
        assert_eq!(t["status"], "Testing");
        assert_eq!(t["reviewState"], "Approved");
    }
    c.wait_events("ReviewUpdated", 2).await;
    let finished = c.wait_events("ReviewFinished", 1).await;
    assert_eq!(finished[0]["event"]["taskId"], tid.as_str());

    let list = c.ok("reviews.list", json!({"taskId": tid})).await;
    assert_eq!(list["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(list["gate"]["passed"], has_git, "{list}");
    let got = c.ok("reviews.get", json!({"reviewId": rid})).await;
    assert_eq!(got["review"]["id"], rid.as_str());

    // reviews.adversarial (wait): the adversary always challenges something;
    // the verification pass answers every finding.
    let adv = c.ok("reviews.adversarial", json!({"taskId": tid, "wait": true})).await["review"].clone();
    assert_eq!(adv["kind"], "Adversarial");
    assert_eq!(adv["status"], "Finished", "{adv}");
    let findings = adv["findings"].as_array().unwrap().clone();
    assert!(!findings.is_empty(), "{adv}");
    assert!(findings.iter().all(|f| !f["response"].is_null()), "{adv}");
    let fid = findings[0]["id"].as_str().unwrap().to_owned();
    let aid = adv["id"].as_str().unwrap().to_owned();
    let resolved = c
        .ok("reviews.resolveFinding", json!({"reviewId": aid, "findingId": fid, "status": "Fixed", "note": "done"}))
        .await;
    assert_eq!(resolved["review"]["findings"][0]["status"], "Fixed");
    assert_eq!(
        c.err("reviews.resolveFinding", json!({"reviewId": aid, "findingId": "nope", "status": "Fixed"})).await["code"],
        "not_found"
    );
    assert_eq!(
        c.err("reviews.resolveFinding", json!({"reviewId": aid, "findingId": fid, "status": "Confirmed"})).await
            ["code"],
        "bad_request"
    );
    assert_eq!(c.err("reviews.list", json!({})).await["code"], "bad_request");
    assert_eq!(
        c.err("reviews.get", json!({"reviewId": "0192a000-0000-7000-8000-000000000000"})).await["code"],
        "not_found"
    );

    // consensus.run (wait): two participants + synthesis → Decision Log.
    let run = c
        .ok(
            "consensus.run",
            json!({"projectId": pid, "question": "How should we cache API responses?", "models": [model, model], "wait": true}),
        )
        .await["run"]
        .clone();
    assert_eq!(run["status"], "Finished", "{run}");
    assert_eq!(run["proposals"].as_array().unwrap().len(), 2);
    assert!(run["proposals"].as_array().unwrap().iter().all(|p| p["status"] == "Done" && p["score"].is_number()));
    assert!(!run["warnings"].as_array().unwrap().is_empty(), "duplicate model is reported");
    let decision_id = run["decisionId"].as_str().unwrap().to_owned();
    c.wait_events("DecisionRecorded", 1).await;
    c.wait_events("ConsensusUpdated", 3).await;
    let decisions = c.ok("decisions.list", json!({"projectId": pid})).await["decisions"].as_array().unwrap().clone();
    let d = decisions.iter().find(|d| d["id"] == decision_id.as_str()).unwrap();
    assert_eq!(d["alternatives"].as_array().unwrap().len(), 2);
    assert_eq!(d["decidedBy"]["type"], "Manager", "{d}");
    let listed = c.ok("consensus.list", json!({"projectId": pid})).await;
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);
    let one = c.ok("consensus.get", json!({"consensusId": run["id"]})).await;
    assert_eq!(one["run"]["decision"]["chosenIndex"], run["decision"]["chosenIndex"]);
    // Non-waiting run returns at once and finishes through events.
    let started = c.ok("consensus.run", json!({"projectId": pid, "question": "Pick a queue", "participants": 2})).await;
    assert_eq!(started["run"]["status"], "Proposing");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = c.ok("consensus.get", json!({"consensusId": started["run"]["id"]})).await["run"].clone();
        if r["status"] == "Finished" || r["status"] == "Failed" {
            assert_eq!(r["status"], "Finished", "{r}");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "consensus did not finish: {r}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        c.err("consensus.run", json!({"projectId": pid, "question": "q", "models": [model]})).await["code"],
        "bad_request"
    );
    core.shutdown().await;
}
