//! End-to-end `quality.*` over the real WebSocket: detection, profile
//! overrides, gated runs through the terminal run API, events, gates.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    next: u64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
        let mut c = Self { ws, next: 0, events: Vec::new() };
        let res = c.recv().await;
        assert_eq!(res["ok"], true, "{res}");
        c
    }

    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(120), self.ws.next())
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

    async fn send_frame(&mut self, v: Value) -> Value {
        let id = v["id"].as_str().unwrap().to_owned();
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
        loop {
            let v = self.recv().await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id.as_str() => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.send_frame(json!({"type": "req", "id": id, "method": method, "params": params})).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let res = self.call_raw(method, params).await;
        assert_eq!(res["ok"], true, "{method} failed: {res}");
        res["result"].clone()
    }

    async fn error_code(&mut self, method: &str, params: Value) -> String {
        let res = self.call_raw(method, params).await;
        assert_eq!(res["ok"], false, "{method} unexpectedly succeeded: {res}");
        res["error"]["code"].as_str().unwrap().to_owned()
    }

    async fn wait_event(&mut self, kind: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(i) = self.events.iter().position(|e| e["event"]["kind"] == kind) {
                return self.events.remove(i);
            }
            assert!(tokio::time::Instant::now() < deadline, "no {kind} event; seen {:?}", self.events);
            let v = self.recv().await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

fn check(id: &str, kind: &str, command: &str) -> Value {
    json!({"id": id, "kind": kind, "toolchain": "Custom", "label": id, "command": command, "cwd": ".", "enabled": true, "source": "User"})
}

#[tokio::test]
async fn quality_over_websocket() {
    let data = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(data.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c.call("projects.create", json!({"name": "Q", "rootPath": repo.path().to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();
    c.send_frame(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]})).await;

    // Detection + live profile.
    let det = c.call("quality.detect", json!({"projectId": pid})).await;
    let cmds: Vec<&str> = det["checks"].as_array().unwrap().iter().map(|c| c["command"].as_str().unwrap()).collect();
    assert!(cmds.contains(&"cargo test"), "{det}");
    assert_eq!(det["roots"][0]["toolchains"][0], "Cargo");
    let prof = c.call("quality.getProfile", json!({"projectId": pid})).await;
    assert_eq!(prof["customized"], false);

    // Overrides: fast commands. Checks run in the platform command shell —
    // cmd.exe on Windows (echo prints quotes verbatim, `;` is literal), sh
    // elsewhere (needs quotes around `;`).
    let test_echo = if cfg!(windows) {
        "echo test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured"
    } else {
        "echo 'test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured'"
    };
    let checks =
        json!([check("build", "Build", "exit 0"), check("lint", "Lint", "exit 2"), check("test", "Test", test_echo),]);
    let saved = c.call("quality.setProfile", json!({"projectId": pid, "checks": checks})).await;
    assert_eq!(saved["customized"], true);
    assert_eq!(saved["checks"].as_array().unwrap().len(), 3);
    assert_eq!(
        c.error_code("quality.setProfile", json!({"projectId": pid, "checks": [check("x", "Build", "a\nb")]})).await,
        "bad_request"
    );

    // Run (wait) → structured results.
    let run = c.call("quality.run", json!({"projectId": pid, "wait": true})).await;
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "{run}");
    assert_eq!(results[0]["status"], "Passed", "{run}");
    assert_eq!(results[1]["status"], "Failed", "{run}");
    assert_eq!(results[1]["exitCode"], 2);
    assert_eq!(results[2]["status"], "Passed", "{run}");
    assert_eq!(results[2]["passed"], 3, "{run}");
    assert_eq!(results[2]["skipped"], 1);
    assert_eq!(run["status"], "Failed");
    let run_id = run["id"].as_str().unwrap().to_owned();

    // Events scoped to the project.
    let started = c.wait_event("BuildStarted").await;
    assert_eq!(started["event"]["projectId"], pid.as_str());
    c.wait_event("BuildSucceeded").await;
    let failed = c.wait_event("BuildFailed").await;
    assert!(failed["event"]["payload"]["summary"].as_str().unwrap().contains("exit code 2"));
    let tests = c.wait_event("TestRunFinished").await;
    assert_eq!(tests["event"]["payload"]["passed"], 3);

    // Subset run (kinds) in the background, then fetch.
    let bg = c.call("quality.run", json!({"projectId": pid, "kinds": ["Build"]})).await;
    assert_eq!(bg["results"].as_array().unwrap().len(), 1);
    let bg_id = bg["id"].as_str().unwrap().to_owned();
    let mut fetched = Value::Null;
    for _ in 0..200 {
        fetched = c.call("quality.getRun", json!({"runId": bg_id})).await;
        if fetched["status"] != "Running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(fetched["status"], "Passed", "{fetched}");

    let runs = c.call("quality.runs", json!({"projectId": pid})).await;
    let ids: Vec<&str> = runs["runs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![bg_id.as_str(), run_id.as_str()]);
    assert_eq!(c.error_code("quality.getRun", json!({"runId": "nope"})).await, "not_found");
    assert_eq!(c.call("quality.cancel", json!({"runId": "nope"})).await["cancelled"], false);
    assert_eq!(
        c.error_code("quality.run", json!({"projectId": pid, "kinds": ["E2e"]})).await,
        "bad_request",
        "no E2E check configured"
    );
    // Arbitrary directories are rejected: only worktrees of the project's repository.
    let code = c
        .error_code(
            "quality.run",
            json!({"projectId": pid, "target": {"type": "worktree", "path": "C:/not/a/worktree"}}),
        )
        .await;
    assert!(["not_found", "bad_request", "internal"].contains(&code.as_str()), "{code}");

    // Task-scoped run + gates.
    let task =
        c.call("tasks.create", json!({"projectId": pid, "title": "Feature", "acceptanceCriteria": ["works"]})).await;
    let tid = task["task"]["id"].as_str().unwrap().to_owned();
    let trun = c
        .call("quality.run", json!({"projectId": pid, "target": {"type": "task", "taskId": tid}, "kinds": ["Build", "Test"], "wait": true}))
        .await;
    assert_eq!(trun["taskId"], tid.as_str());
    assert_eq!(trun["status"], "Passed", "{trun}");

    let cfg = c.call("quality.gates", json!({"projectId": pid})).await;
    assert_eq!(cfg["customized"], false);
    // Lint gate advisory, review/security not required → task gates pass.
    let cfg = c
        .call(
            "quality.setGates",
            json!({"projectId": pid, "gates": [
                {"gate": "LintPasses", "enabled": true, "required": false},
                {"gate": "ReviewerApproved", "enabled": true, "required": false},
                {"gate": "NoCriticalSecurity", "enabled": false, "required": true},
                {"gate": "AcceptanceCriteriaMet", "enabled": true, "required": false}
            ]}),
        )
        .await;
    assert_eq!(cfg["customized"], true);
    let report = c.call("quality.evaluate", json!({"projectId": pid, "taskId": tid})).await;
    let gate = |k: &str| report["gates"].as_array().unwrap().iter().find(|g| g["gate"] == k).unwrap().clone();
    assert_eq!(gate("BuildPasses")["status"], "Passed", "{report}");
    assert_eq!(gate("TestsPass")["status"], "Passed");
    assert_eq!(gate("LintPasses")["status"], "Pending", "lint never ran for this task: {report}");
    assert_eq!(gate("AcceptanceCriteriaMet")["status"], "Pending", "criterion not met yet");
    assert_eq!(gate("AcceptanceCriteriaMet")["detail"], "0/1");
    assert_eq!(gate("NoCriticalSecurity")["reason"], "Disabled");
    assert_eq!(report["overall"], "Passed", "{report}");
    assert_eq!(report["dod"].as_array().unwrap().len(), 7);

    // Project-level evaluation sees the failing lint run.
    let project_report = c.call("quality.evaluate", json!({"projectId": pid})).await;
    let lint = project_report["gates"].as_array().unwrap().iter().find(|g| g["gate"] == "LintPasses").unwrap().clone();
    assert_eq!(lint["status"], "Failed", "{project_report}");

    // Reset profile → detection again.
    let reset = c.call("quality.setProfile", json!({"projectId": pid, "reset": true})).await;
    assert_eq!(reset["customized"], false);

    core.shutdown().await;
}
