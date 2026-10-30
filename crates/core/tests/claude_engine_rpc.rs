//! `claudeEngine.*` + an Orkestr agent running on Claude Code, end to end
//! over a real WebSocket with the `orkestr-fake-claude` test double (same
//! stream-json + control protocol as the official CLI):
//!
//! * the signed-in binary is registered as a provider; "use for everything"
//!   moves the Manager and every role to it (previous model kept as fallback);
//! * an agent whose model is Claude Code runs with the permission bridge: the
//!   CLI's `can_use_tool` for `Write` becomes an Orkestr approval, the user
//!   approves, the file is written, the tool call is recorded;
//! * chat through Claude Code (print mode) works, incl. emulated tool calls;
//! * a project `.claude/agents/*.md` agent is written (with confirmation),
//!   listed and mirrored as a `ClaudeCode` template, then deleted.

use std::collections::VecDeque;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    events: VecDeque<Value>,
    seq: u64,
}

impl Client {
    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(60), self.ws.next())
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

    async fn frame(&mut self, frame: Value, id: &str) -> Value {
        self.ws.send(Message::Text(frame.to_string().into())).await.unwrap();
        loop {
            let v = self.recv().await;
            if v["type"] == "res" && v["id"] == id {
                return v;
            }
            if v["type"] == "event" {
                self.events.push_back(v["event"].clone());
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.seq += 1;
        let id = format!("r{}", self.seq);
        self.frame(json!({"type": "req", "id": id, "method": method, "params": params}), &id).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    async fn sub(&mut self, topic: String) {
        self.seq += 1;
        let id = format!("s{}", self.seq);
        let r = self.frame(json!({"type": "sub", "id": id, "topics": [topic]}), &id).await;
        assert_eq!(r["ok"], true, "{r}");
    }

    async fn wait(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(pos) = self.events.iter().position(&pred) {
            return self.events.remove(pos).unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
            let v = self.recv().await;
            if v["type"] == "event" {
                let ev = v["event"].clone();
                if pred(&ev) {
                    return ev;
                }
                self.events.push_back(ev);
            }
        }
    }
}

async fn wait_agent_final(c: &mut Client, id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let a = c.ok("agents.get", json!({"agentId": id})).await["agent"].clone();
        if matches!(a["state"].as_str(), Some("Completed" | "Error" | "Stopped")) && a["stoppedAt"].is_string() {
            return a;
        }
        assert!(tokio::time::Instant::now() < deadline, "agent did not finish: {a}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn claude_code_as_the_engine_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path().join("data"))).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);

    // Provider registration from the discovered, signed-in binary.
    let fake = env!("CARGO_BIN_EXE_orkestr-fake-claude");
    c.ok("claudeCode.setBinaryPath", json!({"path": fake})).await;
    let st = c.ok("claudeEngine.status", json!({"refresh": true})).await;
    assert_eq!(st["installed"], true, "{st}");
    assert_eq!(st["signedIn"], true, "{st}");
    assert_eq!(st["subscription"], "max");
    let provider = st["providerId"].as_str().expect("provider registered").to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let cc: Vec<&Value> = providers.iter().filter(|p| p["provider"]["kind"] == "ClaudeCode").collect();
    assert_eq!(cc.len(), 1, "exactly one Claude Code provider");
    assert_eq!(cc[0]["provider"]["settings"]["binaryPath"], fake);
    // Idempotent.
    let again = c.ok("claudeEngine.status", json!({})).await;
    assert_eq!(again["providerId"], provider.as_str());

    // "Use Claude Code for everything", previous Manager model kept as fallback.
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let mut mgr = c.ok("providers.getManager", json!({})).await["config"].clone();
    mgr["model"] = json!({"providerId": mock, "model": "mock-smart"});
    c.ok("providers.setManager", json!({"config": mgr})).await;
    let r = c.ok("claudeEngine.useForEverything", json!({"model": "haiku"})).await;
    assert_eq!(r["manager"]["model"]["providerId"], provider.as_str());
    assert_eq!(r["manager"]["model"]["model"], "haiku");
    assert_eq!(r["manager"]["fallbackChain"][0]["providerId"], mock);
    let rules = r["routing"]["rules"].as_array().unwrap();
    for purpose in ["Manager", "Backend", "Review", "Consensus", "Product Manager"] {
        let rule = rules.iter().find(|x| x["purpose"] == purpose).unwrap_or_else(|| panic!("{purpose} rule"));
        assert_eq!(rule["model"]["providerId"], provider.as_str(), "{purpose}");
    }
    assert_eq!(r["status"]["managerOnClaude"], true);
    assert!(r["status"]["rolesOnClaude"].as_u64().unwrap() >= 10);

    // Chat through Claude Code (print mode).
    let t = c.ok("providers.test", json!({"providerId": provider, "model": "haiku"})).await;
    assert_eq!(t["ok"], true, "{t}");
    assert!(t["sample"].as_str().unwrap_or_default().starts_with("pong"), "{t}");

    // An agent on Claude Code: Write goes through Orkestr's approvals.
    let project = c
        .ok("projects.create", json!({"name": "Engine", "rootPath": root.to_string_lossy(), "approvalMode": "Ask"}))
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    c.sub(format!("project:{pid}")).await;
    c.sub("global".into()).await;
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

    let spawned = c
        .ok(
            "agents.spawn",
            json!({
                "projectId": pid,
                "templateId": tid,
                "newTask": {"title": "Please write the hello file", "acceptanceCriteria": ["fake.txt exists"]},
                "model": {"providerId": provider, "model": "sonnet"}
            }),
        )
        .await;
    let aid = spawned["agent"]["id"].as_str().unwrap().to_owned();
    let approval =
        c.wait("ApprovalRequested", |e| e["kind"] == "ApprovalRequested" && e["agentId"] == aid.as_str()).await;
    let approval_id = approval["payload"]["approval"]["id"].as_str().expect("approval id").to_owned();
    assert_eq!(approval["payload"]["approval"]["capability"], "Write", "{approval}");
    assert!(!root.join("fake.txt").exists(), "nothing is written before the approval");
    c.ok("approvals.resolve", json!({"approvalId": approval_id, "decision": "Approve"})).await;
    let a = wait_agent_final(&mut c, &aid).await;
    assert_ne!(a["state"], "Error", "{a}");
    assert_eq!(std::fs::read_to_string(root.join("fake.txt")).unwrap(), "hello");
    let tr = c.ok("agents.transcript", json!({"agentId": aid})).await;
    let calls = tr["toolCalls"].as_array().unwrap();
    let write = calls.iter().find(|t| t["toolCall"]["tool"] == "Write").expect("Write tool call recorded");
    assert_eq!(write["toolCall"]["status"], "Succeeded", "{write}");
    assert_eq!(write["toolCall"]["approvalId"], approval_id.as_str());
    assert!(tr["messages"].to_string().contains("Done."), "{}", tr["messages"]);

    // Claude Code sub-agents: write (confirmed), list (mirrored), delete.
    let params = json!({
        "projectId": pid, "scope": "project", "name": "doc-writer", "description": "Writes docs",
        "tools": ["Read", "Edit"], "model": "haiku", "prompt": "You write documentation.", "confirmed": false
    });
    let r = c.call("claudeEngine.saveAgent", params.clone()).await;
    assert_eq!(r["error"]["code"], "approval_required", "{r}");
    let mut confirmed = params.clone();
    confirmed["confirmed"] = json!(true);
    let saved = c.ok("claudeEngine.saveAgent", confirmed).await;
    let file = root.join(".claude").join("agents").join("doc-writer.md");
    assert!(file.is_file());
    assert!(saved["agent"]["templateId"].is_string(), "{saved}");
    let mut bad = params.clone();
    bad["name"] = json!("../evil");
    bad["confirmed"] = json!(true);
    assert_eq!(c.call("claudeEngine.saveAgent", bad).await["error"]["code"], "bad_request");
    let agents = c.ok("claudeEngine.agents", json!({"projectId": pid})).await;
    let doc = agents["agents"].as_array().unwrap().iter().find(|a| a["name"] == "doc-writer").cloned().unwrap();
    assert_eq!(doc["scope"], "project");
    assert_eq!(doc["tools"], json!(["Read", "Edit"]));
    let templates = c.ok("templates.list", json!({})).await["templates"].as_array().unwrap().clone();
    let mirrored = templates.iter().find(|t| t["name"] == "doc-writer").expect("mirrored template");
    assert_eq!(mirrored["source"], "ClaudeCode");
    assert_eq!(mirrored["preferredModel"]["providerId"], provider.as_str());
    assert_eq!(mirrored["preferredModel"]["model"], "haiku");
    assert!(mirrored["tools"].as_array().unwrap().iter().any(|t| t == "write_file"));
    assert!(!mirrored["tools"].as_array().unwrap().iter().any(|t| t == "run_command"));
    c.ok(
        "claudeEngine.deleteAgent",
        json!({"projectId": pid, "scope": "project", "name": "doc-writer", "confirmed": true}),
    )
    .await;
    assert!(!file.exists());
    let templates = c.ok("templates.list", json!({})).await["templates"].as_array().unwrap().clone();
    assert!(!templates.iter().any(|t| t["name"] == "doc-writer"));

    core.shutdown().await;
}

async fn connect(core: &orkestr_core::CoreHandle) -> Client {
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);
    c
}

/// The «Model» of the Claude Code card is saved in the core: it survives a
/// status refresh (CLI re-detection), "use for everything" and a restart,
/// and moves whatever already runs on Claude Code. Before the fix the card
/// kept it in React state only and always came back as `sonnet`.
#[tokio::test]
async fn claude_model_choice_persists() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let fake = env!("CARGO_BIN_EXE_orkestr-fake-claude");
    let core = orkestr_core::start(CoreConfig::for_tests(data.clone())).await.unwrap();
    let mut c = connect(&core).await;
    c.ok("claudeCode.setBinaryPath", json!({"path": fake})).await;
    let st = c.ok("claudeEngine.status", json!({"refresh": true})).await;
    let provider = st["providerId"].as_str().expect("provider registered").to_owned();
    assert_eq!(st["model"], "sonnet", "default before any choice");

    // Everything on Claude Code with Sonnet, then the user picks Opus.
    c.ok("claudeEngine.useForEverything", json!({"model": "sonnet"})).await;
    let st = c.ok("claudeEngine.setModel", json!({"model": "opus"})).await;
    assert_eq!(st["model"], "opus");
    let mgr = c.ok("providers.getManager", json!({})).await["config"].clone();
    assert_eq!(mgr["model"], json!({"providerId": provider, "model": "opus"}));
    let routing = c.ok("providers.getRouting", json!({})).await["config"].clone();
    for rule in routing["rules"].as_array().unwrap() {
        if rule["model"]["providerId"] == provider.as_str() {
            assert_eq!(rule["model"]["model"], "opus", "{rule}");
        }
    }
    // A refresh (CLI re-detection) does not reset it.
    assert_eq!(c.ok("claudeEngine.status", json!({"refresh": true})).await["model"], "opus");
    // The parallel engines use the same model (their `balanced` re-pointing
    // would otherwise bring Sonnet back).
    let engines = c.ok("engines.status", json!({})).await;
    assert_eq!(engines["config"]["claudeModel"], "opus");
    assert_eq!(c.call("claudeEngine.setModel", json!({"model": "bad model!"})).await["error"]["code"], "bad_request");
    drop(c);
    core.shutdown().await;

    // After a restart the saved choice is still there.
    let core = orkestr_core::start(CoreConfig::for_tests(data.clone())).await.unwrap();
    let mut c = connect(&core).await;
    let st = c.ok("claudeEngine.status", json!({"refresh": true})).await;
    assert_eq!(st["providerId"], provider.as_str());
    assert_eq!(st["model"], "opus", "{st}");
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let cc = providers.iter().find(|p| p["provider"]["kind"] == "ClaudeCode").unwrap();
    assert_eq!(cc["provider"]["defaultModel"], "opus", "{cc}");

    // Not on Claude Code: the provider default still remembers the choice,
    // and "use for everything" applies the chosen model.
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let mut mgr = c.ok("providers.getManager", json!({})).await["config"].clone();
    mgr["model"] = json!({"providerId": mock, "model": "mock-smart"});
    c.ok("providers.setManager", json!({"config": mgr})).await;
    assert_eq!(c.ok("claudeEngine.setModel", json!({"model": "haiku"})).await["model"], "haiku");
    let r = c.ok("claudeEngine.useForEverything", json!({"model": "haiku"})).await;
    assert_eq!(r["manager"]["model"]["model"], "haiku");
    assert_eq!(r["status"]["model"], "haiku");
    drop(c);
    core.shutdown().await;
}

/// Live check against the real, signed-in Claude Code (haiku, one tiny task):
/// `ORKESTR_REAL_CLAUDE=C:\path\to\claude.exe cargo test -p orkestr-core
/// --test claude_engine_rpc -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "needs a signed-in Claude Code binary (ORKESTR_REAL_CLAUDE); spends a little usage"]
async fn real_claude_agent_writes_a_file_after_approval() {
    let Ok(bin) = std::env::var("ORKESTR_REAL_CLAUDE") else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path().join("data"))).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);
    c.ok("claudeCode.setBinaryPath", json!({"path": bin})).await;
    let st = c.ok("claudeEngine.status", json!({"refresh": true})).await;
    println!("status: {st}");
    let provider = st["providerId"].as_str().expect("provider").to_owned();
    let agents = c.ok("claudeEngine.agents", json!({"refresh": true})).await;
    let names: Vec<String> = agents["agents"].as_array().unwrap().iter().map(|a| a["name"].to_string()).collect();
    println!("Claude Code agents: {}", names.join(", "));
    let project = c
        .ok("projects.create", json!({"name": "Live", "rootPath": root.to_string_lossy(), "approvalMode": "Ask"}))
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    c.sub(format!("project:{pid}")).await;
    c.sub("global".into()).await;
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
    draft["maxTurns"] = json!(6);
    c.ok("templates.update", json!({"templateId": tid, "template": draft})).await;
    let spawned = c
        .ok(
            "agents.spawn",
            json!({
                "projectId": pid,
                "templateId": tid,
                "newTask": {
                    "title": "Create the file hello.txt containing exactly: hi",
                    "description": "Use the Write tool once, then finish. Do nothing else."
                },
                "model": {"providerId": provider, "model": "haiku"}
            }),
        )
        .await;
    let aid = spawned["agent"]["id"].as_str().unwrap().to_owned();
    let approval =
        c.wait("ApprovalRequested", |e| e["kind"] == "ApprovalRequested" && e["agentId"] == aid.as_str()).await;
    println!("approval: {}", approval["payload"]["approval"]["title"]);
    assert!(!root.join("hello.txt").exists());
    let id = approval["payload"]["approval"]["id"].as_str().unwrap().to_owned();
    c.ok("approvals.resolve", json!({"approvalId": id, "decision": "Approve"})).await;
    let a = wait_agent_final(&mut c, &aid).await;
    println!("agent: {} usage {}", a["state"], a["usage"]);
    assert_eq!(std::fs::read_to_string(root.join("hello.txt")).unwrap().trim(), "hi");
    core.shutdown().await;
}
