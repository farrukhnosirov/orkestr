//! End-to-end `scheduler.*` over a real WebSocket: status, limits (validated,
//! persisted across a core restart, `SchedulerUpdated` event), explain, and a
//! Manager run with the offline Mock provider that respects the global cap.

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

    async fn wait_event(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = self.events.iter().find(|e| pred(&e["event"])) {
                return e["event"].clone();
            }
            assert!(tokio::time::Instant::now() < deadline, "event not received");
            let v = recv(&mut self.ws).await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

async fn wait_run(c: &mut Client, run_id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
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
async fn scheduler_status_limits_explain_and_a_capped_run_over_ws() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(data.clone())).await.unwrap();
    let mut c = Client::connect(&core).await;
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": ["global"]}), "s1").await;
    assert_eq!(sub["ok"], true);

    // Defaults.
    let st = c.ok("scheduler.status", json!({})).await;
    assert_eq!(st["settings"]["maxParallelGlobal"], 8);
    assert_eq!(st["settings"]["autoResume"], "followApproval");
    assert_eq!(st["counts"], json!({"running": 0, "queued": 0, "waiting": 0}));
    assert!(st["queue"].as_array().unwrap().is_empty());
    assert!(st["resources"]["logicalCores"].as_u64().is_some());

    // Validation.
    let mut settings = st["settings"].clone();
    settings["maxParallelGlobal"] = json!(0);
    assert_eq!(c.err("scheduler.setLimits", json!({"settings": settings})).await["code"], "bad_request");
    assert_eq!(c.err("scheduler.setLimits", json!({})).await["code"], "bad_request");

    // One agent at a time across the core; host thresholds relaxed so a busy
    // CI machine cannot hold the run back.
    let mut settings = st["settings"].clone();
    settings["maxParallelGlobal"] = json!(1);
    settings["maxCpuPercent"] = json!(100.0);
    settings["minFreeRamMb"] = json!(0);
    settings["minFreeDiskMb"] = json!(0);
    let saved = c.ok("scheduler.setLimits", json!({"settings": settings})).await;
    assert_eq!(saved["settings"]["maxParallelGlobal"], 1);
    let ev = c.wait_event(|e| e["kind"] == "SchedulerUpdated" && e["payload"]["settingsChanged"] == true).await;
    assert_eq!(ev["payload"]["counts"]["running"], 0);

    // Project, mock provider, Manager config.
    let project = c
        .ok(
            "projects.create",
            json!({"name": "Scheduled", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}),
        )
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    c.ok(
        "providers.setManager",
        json!({"config": {"model": model, "maxParallelAgents": 4, "approvalPolicy": "SemiAuto"}}),
    )
    .await;
    c.ok(
        "providers.setRouting",
        json!({"config": {"mode": "Manual", "rules": [], "defaultFallback": [model], "preferLocal": true, "maxRateLimitWaitSecs": 0}}),
    )
    .await;

    // Explain a task no run schedules.
    let task = c.ok("tasks.create", json!({"projectId": pid, "title": "Loose task"})).await["task"].clone();
    let tid = task["id"].as_str().unwrap().to_owned();
    let ex = c.ok("scheduler.explain", json!({"taskId": tid})).await;
    assert_eq!(ex["scheduled"], false);
    assert_eq!(ex["checks"].as_array().unwrap().len(), 11);
    assert!(ex["summary"].as_str().unwrap().starts_with("not ready"), "{ex}");
    assert_eq!(
        c.err("scheduler.explain", json!({"taskId": "0192a000-0000-7000-8000-000000000000"})).await["code"],
        "not_found"
    );

    // A plan with two independent tasks runs one agent at a time.
    let sent = c.ok("manager.chat", json!({"projectId": pid, "text": "Create a hello module with tests"})).await;
    let reply_id = sent["replyMessageId"].as_str().unwrap().to_owned();
    let sub = c.raw(json!({"type": "sub", "id": "s2", "topics": [format!("project:{pid}")]}), "s2").await;
    assert_eq!(sub["ok"], true);
    let plan_id = loop {
        let hist = c.ok("manager.history", json!({"projectId": pid})).await;
        let msg = hist["messages"].as_array().unwrap().iter().find(|m| m["id"] == reply_id.as_str()).cloned();
        if let Some(id) = msg.and_then(|m| m["planId"].as_str().map(str::to_owned)) {
            break id;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let approved = c.ok("manager.approvePlan", json!({"planId": plan_id})).await;
    let run_id = approved["run"]["id"].as_str().unwrap().to_owned();
    let mut saw_wait = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let st = c.ok("scheduler.status", json!({"projectId": pid})).await;
        assert!(st["running"].as_array().unwrap().len() <= 1, "global cap 1: {st}");
        if st["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["state"] == "waiting" && e["reason"]["kind"] == "globalLimit" && e["runId"] == run_id.as_str())
        {
            saw_wait = true;
        }
        let runs = c.ok("manager.status", json!({"runId": run_id})).await["runs"].clone();
        let s = runs[0]["status"].as_str().unwrap_or("").to_owned();
        let review = runs[0]["tasks"].as_array().unwrap().iter().any(|t| t["awaitingReview"] == true);
        if matches!(s.as_str(), "Completed" | "Failed" | "Cancelled") || review {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "run did not finish");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let run = wait_run(&mut c, &run_id).await;
    assert_eq!(run["peakParallel"], 1, "never more than one agent: {run}");
    let done = run["tasks"].as_array().unwrap().iter().filter(|t| t["status"] == "Completed").count();
    assert!(done >= 3, "{run}");
    // The implement ∥ document pair had to queue behind the cap at least once
    // (sampled; the scheduler may also be faster than the poll).
    eprintln!("observed a globalLimit wait: {saw_wait}");
    let ev = c.wait_event(|e| e["kind"] == "SchedulerUpdated" && e["payload"]["settingsChanged"] == false).await;
    assert!(ev["payload"]["counts"].is_object());
    core.shutdown().await;

    // Limits survive a restart.
    let core = orkestr_core::start(CoreConfig::with_data_dir(data)).await.unwrap();
    let mut c = Client::connect(&core).await;
    let st = c.ok("scheduler.status", json!({})).await;
    assert_eq!(st["settings"]["maxParallelGlobal"], 1);
    assert_eq!(st["settings"]["minFreeRamMb"], 0);
    core.shutdown().await;
}
