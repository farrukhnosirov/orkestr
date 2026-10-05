//! End-to-end over a real WebSocket: `context.preview` / `context.explain`
//! (Context Optimizer) and `benchmarks.*` (collection from a real Mock agent
//! run, post-merge bug attribution, router statistics).

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use orkestr_domain::{ModelRef, ProviderId};
use orkestr_providers::ModelStatsSource;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    next: u64,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        let mut c = Self { ws, next: 0 };
        c.send(json!({"type": "auth", "token": core.token})).await;
        let res = c.recv().await;
        assert_eq!(res["ok"], true, "{res}");
        c
    }

    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

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

    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.send(json!({"type": "req", "id": id, "method": method, "params": params})).await;
        loop {
            let v = self.recv().await;
            if v["type"] == "res" && v["id"] == id {
                return v;
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call_raw(method, params).await;
        assert_eq!(res["ok"], true, "{method} failed: {res}");
        res["result"].clone()
    }

    async fn err(&mut self, method: &str, params: Value) -> Value {
        let res = self.call_raw(method, params).await;
        assert_eq!(res["ok"], false, "{method} unexpectedly succeeded: {res}");
        res["error"].clone()
    }
}

fn big_payroll() -> String {
    let mut s = String::from("//! Payroll.\n");
    for i in 0..300 {
        if i == 150 {
            s.push_str("pub fn compute_overtime(hours: u32) -> u32 {\n    hours * 3 / 2\n}\n");
        } else {
            s.push_str(&format!("pub const PAD_{i}: u32 = {i}; // filler\n"));
        }
    }
    s
}

fn project_files(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub mod payroll;\npub mod report;\npub mod ui;\n").unwrap();
    std::fs::write(root.join("src/payroll.rs"), big_payroll()).unwrap();
    std::fs::write(
        root.join("src/report.rs"),
        "use crate::payroll::compute_overtime;\n\npub fn monthly() -> u32 {\n    compute_overtime(10)\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/ui.rs"), "pub fn render_button() {}\n").unwrap();
    std::fs::write(root.join("README.md"), "# Payroll demo\n").unwrap();
}

fn items_of<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["items"].as_array().unwrap().iter().filter(|i| i["kind"] == kind).collect()
}

fn has_reason(item: &Value, kind: &str) -> bool {
    item["reasons"].as_array().unwrap().iter().any(|r| r["kind"] == kind)
}

#[tokio::test]
async fn context_preview_and_explain_over_websocket() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    project_files(&root);
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    let pid = c.ok("projects.create", json!({"name": "Payroll", "rootPath": root.to_string_lossy()})).await["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let pscope = json!({"scope": "Project", "projectId": pid});
    c.ok(
        "memory.create",
        json!({"scope": {"scope": "Global"}, "kind": "Convention", "title": "Money", "content": "Use floats.", "key": "money"}),
    )
    .await;
    c.ok(
        "memory.create",
        json!({"scope": pscope, "kind": "Convention", "title": "Money", "content": "Use integer cents.", "key": "money"}),
    )
    .await;
    c.ok(
        "memory.create",
        json!({"scope": pscope, "kind": "Mistake", "title": "Overtime rounding was truncated",
               "content": "compute_overtime rounded down; always round half up.", "tags": ["overtime"]}),
    )
    .await;
    let task = c
        .ok(
            "tasks.create",
            json!({"projectId": pid, "title": "Fix rounding in compute_overtime",
                   "description": "Overtime must round half up.", "acceptanceCriteria": ["report totals match"]}),
        )
        .await["task"]
        .clone();
    let tid = task["id"].as_str().unwrap().to_owned();

    let preview = c.ok("context.preview", json!({"taskId": tid, "maxFiles": 6})).await;
    assert_eq!(preview["taskId"], tid);
    // Prompt order: task first, then rules, memory, files.
    assert_eq!(preview["items"][0]["kind"], "Task");
    let files = items_of(&preview, "File");
    assert_eq!(files[0]["source"], "src/payroll.rs", "{preview}");
    assert_eq!(files[0]["decision"], "Excerpt", "big file is excerpted");
    let ranges = files[0]["ranges"].as_array().unwrap();
    assert!(ranges.iter().any(|r| r["startLine"].as_u64().unwrap() <= 152 && r["endLine"].as_u64().unwrap() >= 152));
    let report = files.iter().find(|f| f["source"] == "src/report.rs").expect("importer of payroll.rs included");
    assert!(has_reason(report, "imports") || has_reason(report, "references"), "{report}");
    // An unrelated file only ever enters as a low-ranked neighbour of a hub
    // (lib.rs declares `mod report`), never through term relevance.
    if let Some(pos) = files.iter().position(|f| f["source"] == "src/ui.rs") {
        assert_eq!(pos, files.len() - 1, "{files:?}");
        assert_eq!(files[pos]["baseScore"], 0.0);
    }
    assert!(files.iter().all(|f| f["source"] != "README.md"));
    assert!(files.iter().all(|f| f["tokens"].as_u64().unwrap() > 0));
    let rules = items_of(&preview, "Rule");
    let money = rules.iter().find(|r| r["title"] == "Money").unwrap();
    assert!(money["reasons"].as_array().unwrap().iter().any(|r| r["kind"] == "rule" && r["overridesGlobal"] == true));
    // Mistakes are effective rules by default; either way they are surfaced and labelled.
    let mut notes = items_of(&preview, "Memory");
    notes.extend(rules.iter().copied());
    let mistake = notes.iter().find(|m| m["memoryKind"] == "Mistake").expect("mistake surfaced");
    assert!(has_reason(mistake, "mistake"));
    let budget = &preview["budget"];
    assert!(budget["used"].as_u64().unwrap() <= budget["budget"].as_u64().unwrap());
    assert!(preview["index"]["files"].as_u64().unwrap() >= 5);
    assert!(preview["index"]["importEdges"].as_u64().unwrap() >= 3);
    assert_eq!(preview["index"]["hasRoot"], true);
    assert!(preview["rendered"].as_str().unwrap().contains("compute_overtime"));
    assert!(preview["rendered"].as_str().unwrap().contains("Use integer cents."));
    assert!(!preview["rendered"].as_str().unwrap().contains("Use floats."), "overridden global rule left out");

    // Deterministic.
    let again = c.ok("context.preview", json!({"taskId": tid, "maxFiles": 6})).await;
    assert_eq!(again["items"], preview["items"]);

    // Explain: rejected candidates with their decision, filter by source.
    let explain = c.ok("context.explain", json!({"taskId": tid, "maxFiles": 1})).await;
    let all = explain["items"].as_array().unwrap();
    assert!(all.iter().any(|i| i["kind"] == "Rule" && i["decision"] == "Duplicate"), "global rule shadowed");
    assert!(all.iter().any(|i| i["kind"] == "File" && i["decision"] == "BelowCutoff"));
    let one = c.ok("context.explain", json!({"taskId": tid, "source": "src/payroll.rs"})).await;
    assert_eq!(one["items"].as_array().unwrap().len(), 1);
    assert!(one["items"][0]["baseScore"].as_f64().unwrap() > 0.0);

    // Model-aware budget: Mock's descriptor window (128k) caps nothing below the requested cap…
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    let big = c.ok("context.preview", json!({"taskId": tid, "model": model, "tokenBudget": 1_000_000})).await;
    assert_eq!(big["budget"]["contextWindow"], 128_000);
    assert!(big["budget"]["reservedOutput"].as_u64().unwrap() > 0);
    let b = big["budget"]["budget"].as_u64().unwrap();
    assert!(b < 128_000 && b > 50_000, "window minus output/system reserve, with a safety margin: {b}");
    // …while a small cap wins.
    let small = c.ok("context.preview", json!({"taskId": tid, "model": model, "tokenBudget": 600})).await;
    assert_eq!(small["budget"]["budget"], 600);
    assert!(small["budget"]["used"].as_u64().unwrap() <= 600);

    // Errors.
    let e = c.err("context.preview", json!({"taskId": tid, "tokenBudget": 10})).await;
    assert_eq!(e["code"], "bad_request");
    let e = c.err("context.preview", json!({"taskId": "0192a000-0000-7000-8000-000000000000"})).await;
    assert_eq!(e["code"], "not_found");

    // A file change invalidates the cached index (collector → global cache).
    c.ok(
        "files.write",
        json!({"projectId": pid, "path": "src/overtime_rules.rs", "create": true,
               "content": "pub fn compute_overtime_bands() -> u32 { 2 }\n"}),
    )
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let p = c.ok("context.preview", json!({"taskId": tid, "maxFiles": 8})).await;
        if p["index"]["files"].as_u64().unwrap() > preview["index"]["files"].as_u64().unwrap() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "index not refreshed after FileChanged: {}", p["index"]);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    core.shutdown().await;
}

async fn wait_record(c: &mut Client, agent: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let recs = c.ok("benchmarks.records", json!({"agentId": agent})).await["records"].clone();
        if let Some(r) = recs.as_array().unwrap().first() {
            if pred(r) {
                return r.clone();
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "benchmark record not as expected: {recs}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn benchmarks_collected_from_a_mock_agent_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Empty tables first.
    let empty = c.ok("benchmarks.models", json!({})).await;
    assert_eq!(empty["days"], 30);
    assert_eq!(empty["rows"], json!([]));

    // A custom backend template without quality gates (as in agents_rpc).
    let list = c.ok("templates.list", json!({})).await["templates"].as_array().unwrap().clone();
    let backend = list.iter().find(|t| t["role"] == "Backend").unwrap().clone();
    let exported = c.ok("templates.export", json!({"templateId": backend["id"]})).await;
    let imported = c.ok("templates.import", json!({"json": exported["json"]})).await;
    let tid = imported["template"]["id"].as_str().unwrap().to_owned();
    let mut draft = imported["template"].clone();
    for k in ["id", "source", "createdAt", "updatedAt"] {
        draft.as_object_mut().unwrap().remove(k);
    }
    draft["tools"] = json!(["write_file", "read_file", "submit_result"]);
    draft["qualityGates"] = json!([]);
    draft["worktreePolicy"] = "SharedWorktree".into();
    c.ok("templates.update", json!({"templateId": tid, "template": draft})).await;

    let pid = c
        .ok("projects.create", json!({"name": "Bench", "rootPath": root.to_string_lossy(), "approvalMode": "SemiAuto"}))
        .await["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let spawned = c
        .ok(
            "agents.spawn",
            json!({"projectId": pid, "templateId": tid, "newTask": {"title": "Write a hello file"},
                   "model": {"providerId": mock, "model": "mock-smart"}}),
        )
        .await;
    let aid = spawned["agent"]["id"].as_str().unwrap().to_owned();
    let task_id = spawned["task"]["id"].as_str().unwrap().to_owned();

    let rec = wait_record(&mut c, &aid, |r| r["taskStatus"] == "Completed" && r["outcome"] != "Running").await;
    assert_eq!(rec["outcome"], "Succeeded", "{rec}");
    assert_eq!(rec["firstPass"], true);
    assert_eq!(rec["model"], "mock-smart");
    assert_eq!(rec["role"], "Backend");
    assert_eq!(rec["taskId"], task_id);
    assert!(rec["calls"].as_u64().unwrap() >= 1);
    assert!(rec["durationMs"].is_u64());
    assert_eq!(rec["postMergeBugs"], 0);

    let models = c.ok("benchmarks.models", json!({"projectId": pid})).await["rows"].clone();
    let row = models.as_array().unwrap().iter().find(|r| r["model"] == "mock-smart").expect("mock row").clone();
    assert_eq!(row["executions"], 1);
    assert_eq!(row["finished"], 1);
    assert_eq!(row["firstPassSuccessRate"], 1.0);
    assert!(row["calls"].as_u64().unwrap() >= 1);
    assert_eq!(row["roles"], json!(["Backend"]));
    let by_role = c.ok("benchmarks.models", json!({"role": "Reviewer"})).await["rows"].clone();
    assert!(by_role.as_array().unwrap().iter().all(|r| r["executions"] == 0));

    let agents = c.ok("benchmarks.agents", json!({"projectId": pid})).await["rows"].clone();
    let a = &agents[0];
    assert_eq!(a["templateId"], tid);
    assert_eq!((a["executions"].as_u64(), a["completed"].as_u64(), a["failed"].as_u64()), (Some(1), Some(1), Some(0)));
    assert_eq!(a["firstPassSuccessRate"], 1.0);
    let inst = c.ok("benchmarks.agents", json!({"projectId": pid, "groupBy": "Instance"})).await["rows"].clone();
    assert_eq!(inst[0]["agentId"], aid);

    // A bug filed against the completed task counts as a post-merge bug.
    c.ok("tasks.create", json!({"projectId": pid, "title": "Hello file is empty", "tags": [format!("bug:{task_id}")]}))
        .await;
    let rec = wait_record(&mut c, &aid, |r| r["postMergeBugs"] == 1).await;
    assert_eq!(rec["postMergeBugs"], 1);
    let agents = c.ok("benchmarks.agents", json!({"projectId": pid})).await["rows"].clone();
    assert_eq!(agents[0]["postMergeBugs"], 1);

    // Router statistics: refreshed snapshot answers for the model and its role.
    core.services.benchmarks.refresh(&core.services.cost).await.unwrap();
    let provider: ProviderId = mock.as_str().unwrap().parse().unwrap();
    let mr = ModelRef { provider_id: provider, model: "mock-smart".into() };
    let h = core.services.benchmarks.history(&mr, Some("Backend")).expect("history for mock-smart");
    assert!(h.samples >= 1);
    assert!(h.call_success_rate > 0.0);

    // Filters and validation.
    let none = c.ok("benchmarks.records", json!({"model": "no-such-model"})).await;
    assert_eq!(none["records"], json!([]));
    let bad = c.err("benchmarks.agents", json!({"groupBy": "Nope"})).await;
    assert_eq!(bad["code"], "bad_request");

    // Restart: the cursor survives, nothing is counted twice.
    core.shutdown().await;
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rec = wait_record(&mut c, &aid, |r| r["postMergeBugs"] == 1).await;
    assert_eq!(rec["outcome"], "Succeeded", "a late stop notice does not rewrite a finished execution");
    let models = c.ok("benchmarks.models", json!({"projectId": pid})).await["rows"].clone();
    let row = models.as_array().unwrap().iter().find(|r| r["model"] == "mock-smart").unwrap().clone();
    assert_eq!(row["executions"], 1, "replay after restart does not double count");
    core.shutdown().await;
}
