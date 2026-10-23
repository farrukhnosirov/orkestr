//! `codex.*` + `engines.*` + an Orkestr agent on Codex, end to end over a
//! real WebSocket with the `orkestr-fake-codex` test double (ADR-0047): status,
//! limits, models, ChatGPT sign-in, panel approvals (forbidden auto-denied,
//! write approved), threads / transcript / fork, the Manager chat pool,
//! parallel-engines routing, Codex templates and a bridged agent run.

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
#[allow(clippy::too_many_lines)]
async fn codex_subscription_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    // Codex templates use a dedicated worktree: a git repository with a commit.
    for args in [
        &["init", "-q"][..],
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."],
        &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"],
    ] {
        assert!(std::process::Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path().join("data"))).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);
    c.sub("global".into()).await;

    // The binary pinned on the provider (as after an install into Orkestr's tools folder).
    let fake = env!("CARGO_BIN_EXE_orkestr-fake-codex");
    let created = c
        .ok(
            "providers.create",
            json!({"kind": "Codex", "name": "Codex", "binaryPath": fake, "defaultModel": "default"}),
        )
        .await;
    let provider =
        created["provider"]["provider"]["id"].as_str().or(created["provider"]["id"].as_str()).unwrap().to_owned();
    let st = c.ok("codex.status", json!({"refresh": true})).await;
    assert_eq!(st["binary"]["version"], "9.9.9", "{st}");
    assert_eq!(st["binary"]["source"], "override");
    assert_eq!(st["loggedIn"], true, "{st}");
    assert_eq!(st["account"]["plan"], "plus");
    assert_eq!(st["providerId"], provider.as_str());

    // Plan limits, models.
    let l = c.ok("codex.limits", json!({"refresh": true})).await;
    assert_eq!(l["available"], true, "{l}");
    assert_eq!(l["primary"]["percent"], 35.0);
    assert_eq!(l["secondary"]["windowMinutes"], 10080);
    let m = c.ok("codex.models", json!({"refresh": true})).await;
    assert_eq!(m["fallback"], false, "{m}");
    assert_eq!(m["models"][0]["id"], "gpt-5-codex");
    assert_eq!(m["models"][0]["efforts"], json!(["low", "medium", "high"]));

    // ChatGPT sign-in (browser flow) completes through the CLI.
    let login = c.ok("codex.login", json!({"mode": "browser"})).await;
    assert!(login["authUrl"].as_str().unwrap().starts_with("https://auth.openai.com/"), "{login}");
    let done = c
        .wait("CodexAccount succeeded", |e| {
            e["kind"] == "CodexAccount" && e["payload"]["login"]["phase"] == "succeeded"
        })
        .await;
    assert_eq!(done["payload"]["loggedIn"], true, "{done}");

    // Panel session: forbidden command auto-denied, a write approved by the user.
    let project = c
        .ok("projects.create", json!({"name": "Codex", "rootPath": root.to_string_lossy(), "approvalMode": "Ask"}))
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let s = c.ok("codex.start", json!({"projectId": pid, "model": "gpt-5-codex", "effort": "high"})).await;
    let sid = s["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(s["state"], "idle");
    assert_eq!(s["mode"], "ask");
    c.sub(format!("codex:{sid}")).await;
    c.sub(format!("project:{pid}")).await;
    c.ok("codex.send", json!({"sessionId": sid, "text": "please rm -rf the disk"})).await;
    let denied = c
        .wait("auto-denied", |e| e["kind"] == "CodexEvent" && e["payload"]["event"]["type"] == "approvalResolved")
        .await;
    assert_eq!(denied["payload"]["event"]["byPolicy"], true, "{denied}");
    assert_eq!(denied["payload"]["event"]["decision"], "deny");
    c.wait("turn done", |e| e["kind"] == "CodexEvent" && e["payload"]["event"]["type"] == "turnCompleted").await;
    c.ok("codex.send", json!({"sessionId": sid, "text": "write the hello file"})).await;
    let req = c
        .wait("approval request", |e| {
            e["kind"] == "CodexEvent"
                && e["payload"]["event"]["type"] == "approvalRequest"
                && e["payload"]["event"]["kind"] == "fileChange"
        })
        .await;
    let rid = req["payload"]["event"]["requestId"].as_str().unwrap().to_owned();
    assert_eq!(req["payload"]["event"]["toolName"], "Edit");
    assert!(!root.join("fake.txt").exists(), "nothing is written before the approval");
    c.ok("codex.respondApproval", json!({"sessionId": sid, "requestId": rid, "decision": "allow"})).await;
    c.wait("turn done", |e| e["kind"] == "CodexEvent" && e["payload"]["event"]["type"] == "turnCompleted").await;
    assert!(root.join("fake.txt").is_file());
    let threads = c.ok("codex.threads", json!({"projectId": pid})).await;
    assert!(threads["threads"].as_array().unwrap().iter().any(|t| t["id"] == s["threadId"]), "{threads}");
    assert_eq!(threads["live"][0]["sessionId"], sid.as_str());
    let tr = c.ok("codex.transcript", json!({"threadId": s["threadId"]})).await;
    assert_eq!(tr["events"][1]["text"], "pong: hello");
    let forked = c.ok("codex.start", json!({"projectId": pid, "threadId": s["threadId"], "fork": true})).await;
    assert_ne!(forked["threadId"], s["threadId"]);
    let stopped = c.ok("codex.stop", json!({"sessionId": forked["sessionId"]})).await;
    assert_eq!(stopped["state"], "exited");
    assert_eq!(c.call("codex.start", json!({"projectId": pid, "threadId": "--evil"})).await["ok"], false);
    std::fs::remove_file(root.join("fake.txt")).unwrap();

    // Manager-style chat through the persistent pool.
    let t = c.ok("providers.test", json!({"providerId": provider, "model": "default"})).await;
    assert_eq!(t["ok"], true, "{t}");
    assert!(t["sample"].as_str().unwrap_or_default().starts_with("pong"), "{t}");

    // Parallel engines: both statuses; split needs Claude Code; single on Codex.
    let es = c.ok("engines.status", json!({})).await;
    assert_eq!(es["codex"]["signedIn"], true, "{es}");
    assert_eq!(es["codex"]["limitPercent"], 35.0);
    assert_eq!(es["config"]["mode"], "off");
    let mut cfg = es["config"].clone();
    cfg["mode"] = json!("split");
    assert_eq!(c.call("engines.setConfig", cfg.clone()).await["ok"], false, "split needs Claude Code too");
    cfg["mode"] = json!("single");
    cfg["primary"] = json!("codex");
    cfg["manager"] = json!("codex");
    let es = c.ok("engines.setConfig", cfg).await;
    assert_eq!(es["config"]["primary"], "codex");
    let mgr = c.ok("providers.getManager", json!({})).await["config"].clone();
    assert_eq!(mgr["model"]["providerId"], provider.as_str(), "{mgr}");
    let routing = c.ok("providers.getRouting", json!({})).await["config"].clone();
    let backend = routing["rules"].as_array().unwrap().iter().find(|r| r["purpose"] == "Backend").unwrap().clone();
    assert_eq!(backend["model"]["providerId"], provider.as_str());

    // Codex agents as templates.
    let tpls = c.ok("codex.templates", json!({"projectId": pid})).await;
    let eng = tpls["templates"].as_array().unwrap().iter().find(|t| t["name"] == "codex-engineer").cloned().unwrap();
    assert!(eng["templateId"].is_string(), "{eng}");
    let all = c.ok("templates.list", json!({})).await["templates"].as_array().unwrap().clone();
    let mirrored = all.iter().find(|t| t["name"] == "codex-engineer").unwrap().clone();
    assert_eq!(mirrored["source"], "Codex");
    assert_eq!(mirrored["preferredModel"]["providerId"], provider.as_str());

    // An Orkestr agent on Codex: its file change goes through Orkestr's approvals.
    let tid = mirrored["id"].as_str().unwrap().to_owned();
    let spawned = c
        .ok(
            "agents.spawn",
            json!({
                "projectId": pid,
                "templateId": tid,
                "newTask": {"title": "Please write the hello file", "acceptanceCriteria": ["fake.txt exists"]},
                "model": {"providerId": provider, "model": "default"}
            }),
        )
        .await;
    let aid = spawned["agent"]["id"].as_str().unwrap().to_owned();
    let approval =
        c.wait("ApprovalRequested", |e| e["kind"] == "ApprovalRequested" && e["agentId"] == aid.as_str()).await;
    assert_eq!(approval["payload"]["approval"]["capability"], "Write", "{approval}");
    assert!(approval["payload"]["approval"].to_string().contains("(Codex)"), "{approval}");
    let approval_id = approval["payload"]["approval"]["id"].as_str().unwrap().to_owned();
    c.ok("approvals.resolve", json!({"approvalId": approval_id, "decision": "Approve"})).await;
    let a = wait_agent_final(&mut c, &aid).await;
    assert_ne!(a["state"], "Error", "{a}");
    let tr = c.ok("agents.transcript", json!({"agentId": aid})).await;
    let calls = tr["toolCalls"].as_array().unwrap();
    let edit = calls.iter().find(|t| t["toolCall"]["tool"] == "Edit").cloned().expect("Edit recorded");
    assert_eq!(edit["toolCall"]["approvalId"], approval_id.as_str(), "{edit}");
    assert!(a["cliSession"].as_str().unwrap_or_default().starts_with("01fake-thread"), "{a}");

    core.shutdown().await;
}

/// Live, read-only check against the real signed-in Codex CLI (no model
/// turn, no usage spent): status, account, limits, models, threads.
/// `ORKESTR_REAL_CODEX=C:\path\to\codex.cmd cargo test -p orkestr-core --test codex_rpc -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "needs the real Codex CLI (ORKESTR_REAL_CODEX)"]
async fn real_codex_status_limits_and_models() {
    let Ok(bin) = std::env::var("ORKESTR_REAL_CODEX") else { return };
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path().join("data"))).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
    let mut c = Client { ws, events: VecDeque::new(), seq: 0 };
    assert_eq!(c.recv().await["ok"], true);
    c.ok("providers.create", json!({"kind": "Codex", "name": "Codex", "binaryPath": bin})).await;
    let st = c.ok("codex.status", json!({"refresh": true})).await;
    println!("status: {st}");
    assert!(st["binary"]["version"].is_string());
    let l = c.ok("codex.limits", json!({"refresh": true})).await;
    println!("limits: {l}");
    let m = c.ok("codex.models", json!({"refresh": true})).await;
    println!(
        "models: {}",
        m["models"]
            .as_array()
            .map(|a| a.iter().map(|x| x["id"].to_string()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    );
    assert_eq!(m["fallback"], false, "{m}");
    let t = c.ok("codex.threads", json!({"limit": 3})).await;
    println!("threads: {}", t["threads"].as_array().map_or(0, Vec::len));
    core.shutdown().await;
}
