//! End-to-end `merge.*` over the real WebSocket (temp repository): policy,
//! proposal (dependency order, conflict prediction), reorder validation,
//! approval-gated execution, conflict stop + manual resolution + continue,
//! abort with rollback.

use std::path::Path;
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

    /// Polls `merge.get` until the proposal reaches one of `want`.
    async fn wait_status(&mut self, id: &str, want: &[&str]) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let p = self.call("merge.get", json!({"proposalId": id})).await["proposal"].clone();
            if want.iter().any(|w| p["status"] == *w) {
                return p;
            }
            assert!(tokio::time::Instant::now() < deadline, "proposal stuck in {}: {p}", p["status"]);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit_file(root: &Path, rel: &str, content: &str, msg: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
    git(root, &["add", "--", rel]);
    git(root, &["commit", "-q", "-m", msg]);
}

fn branch_from(root: &Path, branch: &str, base: &str, files: &[(&str, &str)]) {
    git(root, &["checkout", "-q", "-b", branch, base]);
    for (rel, content) in files {
        commit_file(root, rel, content, &format!("{branch}: {rel}"));
    }
    git(root, &["checkout", "-q", "main"]);
}

#[tokio::test]
async fn merge_manager_over_websocket() {
    if !orkestr_vcs::GitCli::discover().is_available() {
        eprintln!("git not installed — skipping");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Test"), ("user.email", "t@example.invalid"), ("core.autocrlf", "false")] {
        git(root, &["config", k, v]);
    }
    commit_file(root, "README.md", "readme\n", "initial");
    commit_file(root, "src/shared.rs", "pub fn shared() -> u32 {\n    1\n}\n", "shared");
    // api ← ui (ui builds on api), docs independent.
    branch_from(root, "agent/api", "main", &[("src/api.rs", "pub fn api() -> u32 {\n    1\n}\n")]);
    branch_from(root, "agent/ui", "agent/api", &[("src/ui.rs", "pub fn ui() {}\n")]);
    branch_from(root, "agent/docs", "main", &[("docs/guide.md", "# Guide\n")]);
    // Two branches that change the same line.
    branch_from(root, "agent/left", "main", &[("src/shared.rs", "pub fn shared() -> u32 {\n    2\n}\n")]);
    branch_from(root, "agent/right", "main", &[("src/shared.rs", "pub fn shared() -> u32 {\n    3\n}\n")]);
    let main_before = git(root, &["rev-parse", "main"]);

    let core = orkestr_core::start(CoreConfig::with_data_dir(data.path())).await.unwrap();
    let mut c = Client::connect(&core).await;
    let project = c.call("projects.create", json!({"name": "Repo", "rootPath": root.to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();
    c.send_frame(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]})).await;

    // ── policy ────────────────────────────────────────────
    let pol = c.call("merge.policy", json!({"projectId": pid})).await;
    assert_eq!(pol["policy"]["mode"], "Manual");
    assert_eq!(pol["policy"]["requireReview"], true);
    assert_eq!(
        c.error_code("merge.setPolicy", json!({"projectId": pid, "policy": {"targetBranch": "bad..name"}})).await,
        "bad_request"
    );
    let pol = c
        .call(
            "merge.setPolicy",
            json!({"projectId": pid, "policy": {"mode": "Manual", "targetBranch": "main", "requireReview": false,
                   "requireTests": false, "cleanup": "Keep"}}),
        )
        .await;
    assert_eq!(pol["policy"]["targetBranch"], "main");
    assert_eq!(pol["policy"]["cleanup"], "Keep");
    assert_eq!(
        c.error_code("merge.list", json!({"projectId": "00000000-0000-7000-8000-000000000000"})).await,
        "not_found"
    );

    // ── propose: dependency order + semantic diff ────────
    assert_eq!(c.error_code("merge.propose", json!({"projectId": pid, "branches": []})).await, "bad_request");
    let p = c
        .call(
            "merge.propose",
            json!({"projectId": pid, "branches": [
                {"branch": "agent/ui"}, {"branch": "agent/docs"}, {"branch": "agent/api"}]}),
        )
        .await["proposal"]
        .clone();
    let id = p["id"].as_str().unwrap().to_owned();
    assert_eq!(p["status"], "Proposed");
    assert_eq!(p["target"], "main");
    let order: Vec<&str> = p["order"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let pos = |b: &str| order.iter().position(|o| *o == b).unwrap();
    assert!(pos("agent/api") < pos("agent/ui"), "ui depends on api: {order:?}");
    assert!(p["conflicts"].as_array().unwrap().is_empty(), "{p}");
    assert!(p["blockers"].as_array().unwrap().is_empty(), "{p}");
    let api = p["branches"].as_array().unwrap().iter().find(|b| b["branch"] == "agent/api").unwrap();
    assert_eq!(api["diff"]["files"][0]["path"], "src/api.rs");
    assert!(
        api["diff"]["symbols"].as_array().unwrap().iter().any(|s| s["name"] == "api" && s["change"] == "Added"),
        "{api}"
    );
    let ui = p["branches"].as_array().unwrap().iter().find(|b| b["branch"] == "agent/ui").unwrap();
    assert_eq!(ui["dependsOn"], json!(["agent/api"]));

    // Reorder: violating the dependency is rejected; a valid order is kept.
    assert_eq!(
        c.error_code("merge.reorder", json!({"proposalId": id, "order": ["agent/ui", "agent/api", "agent/docs"]}))
            .await,
        "bad_request"
    );
    let p = c.call("merge.reorder", json!({"proposalId": id, "order": ["agent/docs", "agent/api", "agent/ui"]})).await
        ["proposal"]
        .clone();
    assert_eq!(p["order"], json!(["agent/docs", "agent/api", "agent/ui"]));
    let listed = c.call("merge.list", json!({"projectId": pid})).await;
    assert_eq!(listed["proposals"].as_array().unwrap().len(), 1);

    // ── execute: approval required (Manual policy) ───────
    let p = c.call("merge.execute", json!({"proposalId": id})).await["proposal"].clone();
    assert_eq!(p["status"], "AwaitingApproval");
    assert_eq!(git(root, &["rev-parse", "main"]), main_before, "nothing merges before approval");
    let approval = p["execution"]["approvalId"].as_str().unwrap().to_owned();
    assert_eq!(c.error_code("merge.execute", json!({"proposalId": id})).await, "conflict");
    c.call("approvals.resolve", json!({"approvalId": approval, "decision": "Approve"})).await;
    let done = c.wait_status(&id, &["Completed", "Failed"]).await;
    assert_eq!(done["status"], "Completed", "{done}");
    for f in ["src/api.rs", "src/ui.rs", "docs/guide.md"] {
        assert!(root.join(f).is_file(), "{f} merged into the main checkout");
    }
    let exec = &done["execution"];
    assert!(exec["snapshotId"].is_string(), "pre-merge snapshot: {exec}");
    assert_eq!(git(root, &["rev-parse", "main"]), exec["finalCommit"].as_str().unwrap());
    // --no-ff: three merge commits on the first-parent chain.
    let merges = git(root, &["rev-list", "--merges", "--first-parent", &format!("{main_before}..main")]);
    assert_eq!(merges.lines().count(), 3);
    assert!(git(root, &["branch", "--list", "orkestr/integration-*"]).is_empty(), "integration branch removed");
    assert!(c.events.iter().any(|e| e["event"]["kind"] == "MergeProposalUpdated"), "progress events streamed");

    // ── conflicts: predicted, stop, resolve, continue ────
    let p = c
        .call(
            "merge.propose",
            json!({"projectId": pid, "branches": [{"branch": "agent/left"}, {"branch": "agent/right"}]}),
        )
        .await["proposal"]
        .clone();
    let cid = p["id"].as_str().unwrap().to_owned();
    assert_eq!(p["recommendation"], "ResolveConflicts", "{p}");
    assert!(p["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["scope"] == "Pair" && c["paths"].as_array().unwrap().iter().any(|x| x == "src/shared.rs")));
    let p = c.call("merge.execute", json!({"proposalId": cid})).await["proposal"].clone();
    let approval = p["execution"]["approvalId"].as_str().unwrap().to_owned();
    c.call("approvals.resolve", json!({"approvalId": approval, "decision": "Approve"})).await;
    let stopped = c.wait_status(&cid, &["Conflicted", "Failed", "Completed"]).await;
    assert_eq!(stopped["status"], "Conflicted", "{stopped}");
    let conflict = &stopped["execution"]["conflicts"][0];
    assert_eq!(conflict["path"], "src/shared.rs");
    assert!(conflict["ours"].as_str().unwrap().contains('2'));
    assert!(conflict["theirs"].as_str().unwrap().contains('3'));
    let main_mid = git(root, &["rev-parse", "main"]);
    // Unresolved → continue refused; markers in the content refused.
    assert_eq!(c.error_code("merge.execute", json!({"proposalId": cid})).await, "conflict");
    assert_eq!(
        c.error_code(
            "merge.resolveConflict",
            json!({"proposalId": cid, "path": "src/shared.rs",
                   "resolution": {"kind": "Content", "content": "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n"}})
        )
        .await,
        "bad_request"
    );
    assert_eq!(
        c.error_code(
            "merge.resolveConflict",
            json!({"proposalId": cid, "path": "nope.rs", "resolution": {"kind": "Ours"}})
        )
        .await,
        "not_found"
    );
    let resolved = "pub fn shared() -> u32 {\n    5\n}\n";
    let p = c
        .call(
            "merge.resolveConflict",
            json!({"proposalId": cid, "path": "src/shared.rs", "resolution": {"kind": "Content", "content": resolved}}),
        )
        .await["proposal"]
        .clone();
    assert_eq!(p["execution"]["conflicts"][0]["resolved"], true);
    assert_eq!(git(root, &["rev-parse", "main"]), main_mid, "target untouched until the merge completes");
    let p = c.call("merge.execute", json!({"proposalId": cid})).await["proposal"].clone();
    assert_eq!(p["status"], "Merging");
    let done = c.wait_status(&cid, &["Completed", "Failed"]).await;
    assert_eq!(done["status"], "Completed", "{done}");
    assert_eq!(std::fs::read_to_string(root.join("src/shared.rs")).unwrap(), resolved);

    // ── abort: awaiting approval → aborted, approval cancelled ──
    branch_from(root, "agent/late", "main", &[("late.txt", "late\n")]);
    let p = c.call("merge.propose", json!({"projectId": pid, "branches": [{"branch": "agent/late"}]})).await
        ["proposal"]
        .clone();
    let aid = p["id"].as_str().unwrap().to_owned();
    let p = c.call("merge.execute", json!({"proposalId": aid})).await["proposal"].clone();
    let approval = p["execution"]["approvalId"].as_str().unwrap().to_owned();
    let p = c.call("merge.abort", json!({"proposalId": aid, "reason": "not now"})).await["proposal"].clone();
    assert_eq!(p["status"], "Aborted");
    let a = c.call("approvals.get", json!({"approvalId": approval})).await;
    assert_ne!(a["approval"]["status"], "Pending", "{a}");
    assert!(!root.join("late.txt").exists());
    assert_eq!(c.error_code("merge.abort", json!({"proposalId": aid})).await, "conflict");
    assert_eq!(
        c.error_code("merge.get", json!({"proposalId": "00000000-0000-7000-8000-000000000001"})).await,
        "not_found"
    );

    core.shutdown().await;
}
