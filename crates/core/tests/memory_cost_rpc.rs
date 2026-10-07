//! End-to-end over a real WebSocket: memory.*, decisions.*, knowledge.*,
//! cost.*, budgets.* and their events.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use orkestr_domain::{ProjectId, ProviderId, UsageRecord, UsageRecordId};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    events: Vec<Value>,
    next: u64,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        let mut c = Self { ws, events: Vec::new(), next: 0 };
        c.send(json!({"type": "auth", "token": core.token})).await;
        let res = c.recv().await;
        assert_eq!(res["ok"], true, "{res}");
        c.send(json!({"type": "sub", "id": "sub", "topics": ["*"]})).await;
        c.wait_res("sub").await;
        c
    }

    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
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

    async fn wait_res(&mut self, id: &str) -> Value {
        loop {
            let v = self.recv().await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    /// Calls a method and returns the full response frame.
    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.send(json!({"type": "req", "id": id, "method": method, "params": params})).await;
        self.wait_res(&id).await
    }

    /// Calls a method, asserts success and returns the result.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let res = self.call_raw(method, params).await;
        assert_eq!(res["ok"], true, "{method} failed: {res}");
        res["result"].clone()
    }

    async fn wait_event(&mut self, kind: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pos) = self.events.iter().position(|e| e["event"]["kind"] == kind) {
                return self.events.remove(pos)["event"].clone();
            }
            assert!(tokio::time::Instant::now() < deadline, "no {kind} event");
            let v = self.recv().await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

#[tokio::test]
async fn memory_decisions_and_knowledge_over_websocket() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c.call("projects.create", json!({"name": "Payroll"})).await["project"].clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let pscope = json!({"scope": "Project", "projectId": pid});

    // --- memory ------------------------------------------------------------
    let global = c
        .call(
            "memory.create",
            json!({"scope": {"scope": "Global"}, "kind": "Convention", "title": "Indentation",
                   "content": "Use **4 spaces**.", "key": "indent", "tags": ["style"]}),
        )
        .await["item"]
        .clone();
    assert_eq!(global["source"], "user");
    let item = c
        .call(
            "memory.create",
            json!({"scope": pscope, "kind": "Convention", "title": "Tabs here", "content": "Payroll uses tabs for indentation.",
                   "key": "INDENT", "tags": ["style", "payroll"]}),
        )
        .await["item"]
        .clone();
    let ev = c.wait_event("MemoryUpdated").await;
    assert_eq!(ev["payload"]["deleted"], false);
    let ev2 = c.wait_event("MemoryUpdated").await;
    assert_eq!(ev2["projectId"], pid, "project memory events carry the project scope");

    let list = c.call("memory.list", json!({"scope": pscope})).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let kinds = c.call("memory.list", json!({"scope": {"scope": "Global"}, "kinds": ["Todo"]})).await;
    assert!(kinds["items"].as_array().unwrap().is_empty());

    let hits = c.call("memory.search", json!({"query": "indentation", "projectId": pid})).await;
    let hits = hits["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits[0]["score"].as_f64().unwrap() > 0.0);

    let rules = c.call("memory.effectiveRules", json!({"projectId": pid})).await;
    let rules = rules["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1, "project rule overrides the global rule with the same key");
    assert_eq!(rules[0]["origin"], "Project");
    assert_eq!(rules[0]["overrides"], global["id"]);

    let id = item["id"].as_str().unwrap();
    let updated = c.call("memory.update", json!({"id": id, "patch": {"pinned": true}})).await;
    assert_eq!(updated["item"]["pinned"], true);
    assert_eq!(c.call("memory.get", json!({"id": id})).await["item"]["pinned"], true);
    assert_eq!(c.call("memory.delete", json!({"id": id})).await["id"], id);
    let res = c.call_raw("memory.get", json!({"id": id})).await;
    assert_eq!(res["error"]["code"], "not_found");
    let res = c.call_raw("memory.create", json!({"scope": {"scope": "Global"}, "kind": "Todo", "title": "  "})).await;
    assert_eq!(res["error"]["code"], "bad_request");
    let res = c
        .call_raw(
            "memory.create",
            json!({"scope": {"scope": "Project", "projectId": ProjectId::new().to_string()}, "kind": "Todo", "title": "x"}),
        )
        .await;
    assert_eq!(res["error"]["code"], "not_found");

    // --- decisions ---------------------------------------------------------
    let d = c
        .call(
            "decisions.create",
            json!({"projectId": pid, "title": "Use SQLite FTS5", "decision": "FTS5 for memory search",
                   "alternatives": [{"title": "Tantivy", "summary": "separate index", "proposedBy": null, "rejectedReason": "extra moving part"}],
                   "reasoningSummary": "one database", "files": ["crates/memory/src/lib.rs"]}),
        )
        .await["decision"]
        .clone();
    assert_eq!(d["number"], 1);
    assert_eq!(d["status"], "Accepted");
    let ev = c.wait_event("DecisionRecorded").await;
    assert_eq!(ev["projectId"], pid);
    let d2 = c.call("decisions.create", json!({"projectId": pid, "title": "Second", "decision": "x"})).await;
    assert_eq!(d2["decision"]["number"], 2);
    let up = c.call("decisions.update", json!({"id": d["id"], "patch": {"supersededBy": d2["decision"]["id"]}})).await;
    assert_eq!(up["decision"]["status"], "Superseded");
    let list = c.call("decisions.list", json!({"projectId": pid})).await;
    assert_eq!(list["decisions"].as_array().unwrap().len(), 2);
    assert_eq!(list["decisions"][0]["number"], 2, "newest first");
    assert_eq!(c.call("decisions.get", json!({"id": d["id"]})).await["decision"]["title"], "Use SQLite FTS5");

    // --- knowledge ---------------------------------------------------------
    let docs = tempfile::tempdir().unwrap();
    std::fs::write(docs.path().join("guide.md"), "# Guide\n## Deploy\nRun the zebra pipeline.\n").unwrap();
    std::fs::write(docs.path().join("spec.pdf"), b"%PDF").unwrap();
    let src = c.call("knowledge.add", json!({"projectId": pid, "location": docs.path().to_string_lossy()})).await
        ["source"]
        .clone();
    assert_eq!(src["status"], "Ready");
    assert_eq!(src["chunkCount"], 2);
    assert!(src["error"].as_str().unwrap().contains("PDF"));
    let hits = c.call("knowledge.search", json!({"query": "zebra", "projectId": pid})).await;
    assert_eq!(hits["hits"][0]["headingPath"], "Guide › Deploy");
    assert_eq!(hits["hits"][0]["path"], "guide.md");
    let pdf = c.call("knowledge.add", json!({"location": docs.path().join("spec.pdf").to_string_lossy()})).await
        ["source"]
        .clone();
    assert_eq!(pdf["status"], "Failed");
    assert!(pdf["error"].as_str().unwrap().contains("not supported yet"));
    let res = c.call_raw("knowledge.add", json!({"location": "relative/dir"})).await;
    assert_eq!(res["error"]["code"], "bad_request");
    std::fs::write(docs.path().join("faq.md"), "# FAQ\nOkapi answers.\n").unwrap();
    let re = c.call("knowledge.reindex", json!({"id": src["id"]})).await;
    assert_eq!(re["source"]["fileCount"], 2);
    assert_eq!(c.call("knowledge.list", json!({"projectId": pid})).await["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        c.call("knowledge.list", json!({"projectId": pid, "includeGlobal": true})).await["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    c.call("knowledge.remove", json!({"id": src["id"]})).await;
    let hits = c.call("knowledge.search", json!({"query": "okapi", "projectId": pid})).await;
    assert!(hits["hits"].as_array().unwrap().is_empty());

    core.shutdown().await;
}

#[tokio::test]
async fn cost_and_budgets_over_websocket() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut c = Client::connect(&core).await;
    let project = c.call("projects.create", json!({"name": "Shop"})).await["project"].clone();
    let pid: ProjectId = project["id"].as_str().unwrap().parse().unwrap();

    let b = c
        .call(
            "budgets.set",
            json!({"name": "Shop monthly", "scope": {"scope": "Project", "projectId": pid.to_string()},
                   "period": "Monthly", "limitUsd": 10.0}),
        )
        .await["budget"]
        .clone();
    assert_eq!(b["level"], "Ok");
    assert_eq!(b["budget"]["warnAt"], 0.8);
    let bid = b["budget"]["id"].as_str().unwrap().to_owned();
    let res = c.call_raw("budgets.set", json!({"scope": {"scope": "Global"}, "period": "Daily"})).await;
    assert_eq!(res["error"]["code"], "bad_request", "limit required unless unrestricted");

    // Engines ingest usage directly; the threshold event reaches RPC clients.
    let provider = ProviderId::new();
    let record = UsageRecord {
        id: UsageRecordId::new(),
        ts: chrono::Utc::now(),
        provider_id: provider,
        model: "claude-test".into(),
        project_id: Some(pid),
        task_id: None,
        agent_id: None,
        execution_id: None,
        input_tokens: 12_000,
        output_tokens: 3_000,
        cached_input_tokens: 0,
        reasoning_tokens: 0,
        cost_usd: 8.5,
        api_equivalent_usd: None,
        estimated: true,
        latency_ms: Some(900),
        success: true,
    };
    let fired = core.services.cost.ingest(&record).await.unwrap();
    assert_eq!(fired.len(), 1);
    let ev = c.wait_event("BudgetThreshold").await;
    assert_eq!(ev["projectId"], pid.to_string());
    assert!((ev["payload"]["fraction"].as_f64().unwrap() - 0.85).abs() < 1e-9);

    let s = c.call("cost.summary", json!({"projectId": pid.to_string()})).await;
    assert_eq!(s["today"]["costUsd"], 8.5);
    assert_eq!(s["today"]["estimatedCostUsd"], 8.5);
    assert_eq!(s["month"]["inputTokens"], 12_000);
    assert!((s["budgetUsed"].as_f64().unwrap() - 0.85).abs() < 1e-9);
    assert_eq!(s["budgetId"], bid);

    let rows = c.call("cost.byDimension", json!({"dimension": "Model"})).await;
    assert_eq!(rows["rows"][0]["model"], "claude-test");
    assert_eq!(rows["rows"][0]["providerId"], provider.to_string());
    let rows = c.call("cost.byDimension", json!({"dimension": "Agent", "window": "All"})).await;
    assert_eq!(rows["rows"][0]["key"], "none");

    let ts = c.call("cost.timeseries", json!({"granularity": "Day", "buckets": 7})).await;
    let points = ts["points"].as_array().unwrap();
    assert_eq!(points.len(), 7);
    assert_eq!(points[6]["totals"]["costUsd"], 8.5);
    let ts = c.call("cost.timeseries", json!({"granularity": "Month"})).await;
    assert_eq!(ts["points"].as_array().unwrap().len(), 12);

    let recs = c.call("cost.records", json!({"projectId": pid.to_string()})).await;
    assert_eq!(recs["records"].as_array().unwrap().len(), 1);
    assert_eq!(recs["records"][0]["id"], record.id.to_string());

    let check = c.call("budgets.check", json!({"projectId": pid.to_string(), "estimatedCostUsd": 0.5})).await;
    assert_eq!(check["outcome"], "Warn");
    let check = c.call("budgets.check", json!({"projectId": pid.to_string(), "estimatedCostUsd": 2.0})).await;
    assert_eq!(check["outcome"], "Exceeded");
    assert_eq!(check["entries"][0]["budgetId"], bid);
    let check = c.call("budgets.check", json!({"estimatedCostUsd": 2.0})).await;
    assert_eq!(check["outcome"], "Allow", "project budget does not apply outside the project");

    // Provider unrestricted exempts the project limit.
    c.call(
        "budgets.set",
        json!({"scope": {"scope": "Provider", "providerId": provider.to_string()}, "period": "Monthly", "unrestricted": true}),
    )
    .await;
    let check = c
        .call(
            "budgets.check",
            json!({"projectId": pid.to_string(), "providerId": provider.to_string(), "estimatedCostUsd": 50.0}),
        )
        .await;
    assert_eq!(check["outcome"], "Allow");

    let list = c.call("budgets.list", json!({"projectId": pid.to_string()})).await;
    assert_eq!(list["budgets"].as_array().unwrap().len(), 2);
    c.call("budgets.delete", json!({"id": bid})).await;
    let res = c.call_raw("budgets.delete", json!({"id": bid})).await;
    assert_eq!(res["error"]["code"], "not_found");

    core.shutdown().await;
}
