//! End-to-end security RPC: permissions, approvals, secrets, emergency, audit
//! and WebSocket hardening, over a real WebSocket.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use orkestr_domain::{AgentId, Capability, ProjectId, RiskLevel};
use orkestr_security::{ApprovalRequest, SecretBackendChoice};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start() -> (tempfile::TempDir, orkestr_core::CoreHandle) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::with_data_dir(dir.path());
    // Never touch the real OS keyring in tests.
    cfg.secret_backend = SecretBackendChoice::VaultFileKey;
    let core = orkestr_core::start(cfg).await.unwrap();
    (dir, core)
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

struct Client {
    ws: Ws,
    events: Vec<Value>,
    n: u32,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        send(&mut ws, json!({"type": "auth", "token": core.token})).await;
        let res = recv(&mut ws).await;
        assert_eq!(res["ok"], true, "{res}");
        Self { ws, events: Vec::new(), n: 0 }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.n += 1;
        let id = format!("r{}", self.n);
        send(&mut self.ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id.as_str() => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method}: {res}");
        res["result"].clone()
    }

    async fn err(&mut self, method: &str, params: Value) -> String {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} should fail: {res}");
        res["error"]["code"].as_str().unwrap().to_owned()
    }

    async fn subscribe(&mut self, topics: Value) {
        send(&mut self.ws, json!({"type": "sub", "id": "sub", "topics": topics})).await;
        loop {
            let v = recv(&mut self.ws).await;
            if v["type"] == "res" && v["id"] == "sub" {
                assert_eq!(v["ok"], true);
                return;
            }
            self.events.push(v);
        }
    }

    async fn wait_event(&mut self, kind: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pos) = self.events.iter().position(|e| e["event"]["kind"] == kind) {
                return self.events.remove(pos);
            }
            assert!(tokio::time::Instant::now() < deadline, "no {kind} event");
            let v = recv(&mut self.ws).await;
            self.events.push(v);
        }
    }
}

#[tokio::test]
async fn permissions_modes_rules_and_checks() {
    let (dir, core) = start().await;
    let mut c = Client::connect(&core).await;
    c.subscribe(json!(["global"])).await;
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let project = c.ok("projects.create", json!({"name": "Sec", "rootPath": root.to_string_lossy()})).await["project"].clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    c.wait_event("ProjectCreated").await;

    // Modes.
    let m = c.ok("permissions.getMode", json!({"projectId": pid})).await;
    assert_eq!(m["projectMode"], "Ask");
    let m = c.ok("permissions.setMode", json!({"projectId": pid, "mode": "SemiAuto"})).await;
    assert_eq!(m["effectiveMode"], "SemiAuto");
    let ev = c.wait_event("ProjectUpdated").await;
    assert_eq!(ev["event"]["payload"]["project"]["approvalMode"], "SemiAuto");
    let agent = AgentId::new().to_string();
    let m = c.ok("permissions.setMode", json!({"projectId": pid, "agentId": agent, "mode": "FullAuto"})).await;
    assert_eq!(m["agentMode"], "FullAuto");
    assert_eq!(m["effectiveMode"], "FullAuto");
    assert_eq!(c.err("permissions.setMode", json!({"projectId": pid, "mode": null})).await, "bad_request");

    // Checks (SemiAuto, worktree = project root).
    let chk = |cap: &str, resource: Value| json!({"capability": cap, "resource": resource, "projectId": pid, "agentId": AgentId::new().to_string()});
    let d = c.ok("permissions.check", chk("Write", json!({"type": "path", "path": "src/main.rs"}))).await;
    assert_eq!(d["decision"], "Allow", "{d}");
    let d = c.ok("permissions.check", chk("Terminal", json!({"type": "command", "command": "git push --force", "shell": "Bash"}))).await;
    assert_eq!(d["decision"], "Ask");
    assert_eq!(d["danger"], "GitDestructive");
    assert_eq!(d["rememberable"], false);
    let d = c.ok("permissions.check", chk("Terminal", json!({"type": "command", "command": "Remove-Item C:\\ -Recurse", "shell": "PowerShell"}))).await;
    assert_eq!(d["decision"], "Deny");
    assert_eq!(d["hard"], true);
    let d = c.ok("permissions.check", chk("Write", json!({"type": "path", "path": "../escape.txt"}))).await;
    assert_eq!(d["decision"], "Deny");
    assert_eq!(d["code"], "path_escape");
    let d = c.ok("permissions.check", chk("Network", json!({"type": "url", "url": "https://api.example.com/x"}))).await;
    assert_eq!(d["decision"], "Ask");

    // Rules: allow the API host for this project → Allow; delete → Ask again.
    let rule = c
        .ok(
            "permissions.set",
            json!({"rule": {"capability": "Network", "effect": "Allow", "resource": "https://api.example.com/*", "scope": {"scope": "Project", "projectId": pid}}, "note": "api"}),
        )
        .await["rule"]
        .clone();
    let rid = rule["id"].as_str().unwrap().to_owned();
    let d = c.ok("permissions.check", chk("Network", json!({"type": "url", "url": "https://api.example.com/x"}))).await;
    assert_eq!(d["decision"], "Allow");
    let rules = c.ok("permissions.list", json!({"projectId": pid})).await;
    assert_eq!(rules["rules"].as_array().unwrap().len(), 1);
    assert_eq!(c.err("permissions.set", json!({"rule": {"capability": "Read", "effect": "Allow", "resource": "[", "scope": {"scope": "Global"}}})).await, "bad_request");
    assert_eq!(c.ok("permissions.delete", json!({"id": rid})).await["deleted"], true);
    assert_eq!(c.ok("permissions.delete", json!({"id": rid})).await["deleted"], false);
    let d = c.ok("permissions.check", chk("Network", json!({"type": "url", "url": "https://api.example.com/x"}))).await;
    assert_eq!(d["decision"], "Ask");

    let audit = c.ok("audit.list", json!({"verify": true, "action": "permissions."})).await;
    assert_eq!(audit["chainValid"], true);
    assert!(audit["entries"].as_array().unwrap().len() >= 3);
    core.shutdown().await;
}

#[tokio::test]
async fn approvals_inbox_flow() {
    let (_dir, core) = start().await;
    let mut c = Client::connect(&core).await;
    let pid = ProjectId::new();
    c.subscribe(json!([format!("project:{pid}"), "global"])).await;

    let mut req = ApprovalRequest::new(Capability::Terminal, "Run `git push --force`", "dangerous git operation", RiskLevel::High);
    req.project_id = Some(pid);
    req.command_preview = Some("git push --force origin main".into());
    let approval = core.services.security.approvals.request(req).await.unwrap();
    let ev = c.wait_event("ApprovalRequested").await;
    assert_eq!(ev["event"]["payload"]["approval"]["id"], approval.id.to_string());

    // Background waiter (the "agent").
    let svc = std::sync::Arc::clone(&core.services.security.approvals);
    let id = approval.id;
    let waiter = tokio::spawn(async move { svc.wait(id, Duration::from_secs(20), &tokio_util::sync::CancellationToken::new()).await });

    let pending = c.ok("approvals.list", json!({"status": "Pending"})).await;
    assert_eq!(pending["approvals"].as_array().unwrap().len(), 1);
    let got = c.ok("approvals.get", json!({"approvalId": id.to_string()})).await;
    assert_eq!(got["approval"]["commandPreview"], "git push --force origin main");
    let res = c.ok("approvals.resolve", json!({"approvalId": id.to_string(), "decision": "Approve", "note": "go"})).await;
    assert_eq!(res["approval"]["status"], "Approved");
    assert_eq!(res["approval"]["resolvedBy"]["type"], "User");
    let ev = c.wait_event("ApprovalResolved").await;
    assert_eq!(ev["event"]["payload"]["status"], "Approved");
    assert_eq!(waiter.await.unwrap().unwrap(), orkestr_domain::ApprovalStatus::Approved);
    assert_eq!(c.err("approvals.resolve", json!({"approvalId": id.to_string(), "decision": "Reject"})).await, "conflict");
    assert_eq!(c.err("approvals.get", json!({"approvalId": orkestr_domain::ApprovalId::new().to_string()})).await, "not_found");
    let history = c.ok("approvals.list", json!({})).await;
    assert_eq!(history["approvals"].as_array().unwrap().len(), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn secrets_are_write_only_and_vault_locks() {
    let (dir, core) = start().await;
    let mut c = Client::connect(&core).await;
    c.subscribe(json!(["*"])).await;
    let value = "sk-integration-SECRET-value-0123456789";
    let res = c.call("secrets.set", json!({"name": "OPENAI_API_KEY", "scope": {"scope": "Global"}, "value": value})).await;
    assert_eq!(res["ok"], true, "{res}");
    assert!(!res.to_string().contains(value), "value never echoed");
    let sid = res["result"]["secret"]["id"].as_str().unwrap().to_owned();
    assert_eq!(res["result"]["secret"]["backend"], "EncryptedVault");
    let list = c.ok("secrets.list", json!({})).await;
    assert_eq!(list["secrets"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(value));
    let st = c.ok("secrets.status", json!({})).await;
    assert_eq!(st["locked"], false);
    assert_eq!(st["secretCount"], 1);

    // Value is not in the DB file or the vault file in plaintext, nor in the audit log.
    let db_bytes = std::fs::read(dir.path().join("orkestr.db")).unwrap();
    assert!(!String::from_utf8_lossy(&db_bytes).contains(value));
    let vault = std::fs::read_to_string(dir.path().join("vault").join("secrets.bin")).unwrap();
    assert!(!vault.contains(value));
    let audit = c.ok("audit.list", json!({"action": "secrets."})).await;
    assert!(!audit.to_string().contains(value));
    assert!(audit.to_string().contains("secrets.set"));
    // ... and is redacted from logs.
    assert!(!orkestr_core::redact::redact(&format!("leaked {value}")).contains(value));

    // Engines resolve it in-process.
    let got = core
        .services
        .security
        .secrets
        .resolve(sid.parse().unwrap(), orkestr_domain::Actor::System, orkestr_security::AccessContext::default())
        .await
        .unwrap();
    assert_eq!(got.as_str(), value);

    // Lock → writes refused; unlock → OK.
    let st = c.ok("emergency.lockVault", json!({})).await;
    assert_eq!(st["locked"], true);
    assert_eq!(c.err("secrets.set", json!({"name": "B", "scope": {"scope": "Global"}, "value": "x"})).await, "forbidden");
    let em = c.ok("emergency.status", json!({})).await;
    assert_eq!(em["flags"]["vaultLocked"], true);
    assert_eq!(c.ok("secrets.unlock", json!({})).await["locked"], false);
    assert_eq!(c.err("secrets.set", json!({"name": "bad name", "scope": {"scope": "Global"}, "value": "x"})).await, "bad_request");
    assert_eq!(c.ok("secrets.delete", json!({"secretId": sid})).await["deleted"], true);
    assert_eq!(c.ok("secrets.list", json!({})).await["secrets"].as_array().unwrap().len(), 0);
    // No event payload ever contained the value.
    assert!(!serde_json::to_string(&c.events).unwrap().contains(value));
    core.shutdown().await;
}

#[tokio::test]
async fn emergency_stop_all_project_agent_and_reset() {
    let (_dir, core) = start().await;
    let mut c = Client::connect(&core).await;
    c.subscribe(json!(["*"])).await;
    let em = &core.services.security.emergency;
    let pid = ProjectId::new();
    let agent = AgentId::new();
    let t_agent = em.register_agent(agent, Some(pid)).unwrap();
    let t_other = em.register_agent(AgentId::new(), None).unwrap();

    let r = c.ok("emergency.stopAgent", json!({"agentId": agent.to_string()})).await;
    assert_eq!(r["agentsStopped"], 1);
    assert!(t_agent.is_cancelled() && !t_other.is_cancelled());
    let ev = c.wait_event("EmergencyStop").await;
    assert_eq!(ev["event"]["payload"]["scope"]["scope"], "Agent");

    let t_p = em.register_agent(AgentId::new(), Some(pid)).unwrap();
    let r = c.ok("emergency.stopProject", json!({"projectId": pid.to_string()})).await;
    assert_eq!(r["agentsStopped"], 1);
    assert!(t_p.is_cancelled());
    c.wait_event("EmergencyStop").await;

    let f = c.ok("emergency.disableFullAuto", json!({})).await;
    assert_eq!(f["flags"]["fullAutoDisabled"], true);
    let f = c.ok("emergency.disableNetworkTools", json!({"disabled": true})).await;
    assert_eq!(f["flags"]["networkToolsDisabled"], true);

    let r = c.ok("emergency.stopAll", json!({})).await;
    assert_eq!(r["agentsStopped"], 1);
    assert_eq!(r["flags"]["allStopped"], true);
    assert!(t_other.is_cancelled());
    let ev = c.wait_event("EmergencyStop").await;
    assert_eq!(ev["event"]["payload"]["scope"]["scope"], "All");
    assert!(em.register_agent(AgentId::new(), None).is_err(), "new AI work refused while stopped");
    // Agents are denied everything while stopped.
    let d = c.ok("permissions.check", json!({"capability": "Read", "resource": {"type": "none"}, "agentId": AgentId::new().to_string()})).await;
    assert_eq!(d["decision"], "Deny");
    assert_eq!(d["code"], "emergency_stop");

    let s = c.ok("emergency.reset", json!({})).await;
    assert_eq!(s["flags"], json!({"allStopped": false, "fullAutoDisabled": false, "networkToolsDisabled": false, "vaultLocked": false}));
    assert!(em.register_agent(AgentId::new(), None).is_ok());
    let audit = c.ok("audit.list", json!({"action": "emergency.", "verify": true})).await;
    assert_eq!(audit["chainValid"], true);
    assert!(audit["entries"].as_array().unwrap().len() >= 6);
    core.shutdown().await;
}

#[tokio::test]
async fn websocket_hardening() {
    let (_dir, core) = start().await;

    // Allowed Origin works; foreign Origin is rejected before upgrade.
    let mut req = core.url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("Origin", "tauri://localhost".parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": core.token})).await;
    assert_eq!(recv(&mut ws).await["ok"], true);
    drop(ws);
    for origin in ["https://evil.example", "http://localhost:3000", "null"] {
        let mut req = core.url.as_str().into_client_request().unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
        assert!(err.to_string().contains("403"), "{origin}: {err}");
    }

    // Host must carry the bound port.
    let port = core.addr.port();
    let mut req = core.url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("Host", format!("127.0.0.1:{}", port.wrapping_add(1)).parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // Failed-auth backoff: after the free attempts the peer gets 429.
    for _ in 0..orkestr_core::server::FREE_AUTH_FAILURES {
        let mut req = core.url.as_str().into_client_request().unwrap();
        req.headers_mut().insert("Authorization", "Bearer wrong".parse().unwrap());
        let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
    }
    let mut req = core.url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("Authorization", "Bearer wrong".parse().unwrap());
    let _ = tokio_tungstenite::connect_async(req).await.unwrap_err(); // 6th failure → blocked
    let err = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap_err();
    assert!(err.to_string().contains("429"), "{err}");
    core.shutdown().await;
}
