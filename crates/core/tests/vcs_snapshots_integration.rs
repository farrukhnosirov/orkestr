//! End-to-end `vcs.*` + `snapshots.*` over the real WebSocket (temp repo).

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

    /// Calls a method and returns the raw response frame.
    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.send_frame(json!({"type": "req", "id": id, "method": method, "params": params})).await
    }

    /// Calls a method expecting success.
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

#[tokio::test]
async fn vcs_and_snapshots_over_websocket() {
    let git = orkestr_vcs::GitCli::discover();
    if !git.is_available() {
        eprintln!("git not installed — skipping");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let core = orkestr_core::start(CoreConfig::with_data_dir(data.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c.call("projects.create", json!({"name": "Repo", "rootPath": root.to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();
    c.send_frame(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]})).await;

    // Not a repo yet → init.
    let st = c.call("vcs.status", json!({"projectId": pid})).await;
    assert_eq!(st["isRepo"], false);
    let st = c.call("vcs.init", json!({"projectId": pid, "initialBranch": "main"})).await;
    assert_eq!(st["isRepo"], true);
    assert_eq!(c.error_code("vcs.init", json!({"projectId": pid})).await, "conflict");
    for (k, v) in [("user.name", "Test"), ("user.email", "t@example.invalid"), ("core.autocrlf", "false")] {
        git.runner().run_ok(root, ["config", k, v], orkestr_vcs::RunOptions::write()).await.unwrap();
    }

    // Stage + commit.
    write(root, "src/app file.rs", "fn main() {}\n");
    write(root, "README.md", "readme\n");
    let st = c.call("vcs.status", json!({"projectId": pid})).await;
    assert_eq!(st["entries"].as_array().unwrap().len(), 2);
    let st = c.call("vcs.stage", json!({"projectId": pid, "all": true})).await;
    assert!(st["entries"].as_array().unwrap().iter().all(|e| e["index"] == "Added"));
    let commit = c.call("vcs.commit", json!({"projectId": pid, "message": "initial"})).await;
    assert_eq!(commit["commit"]["subject"], "initial");
    assert_eq!(c.error_code("vcs.commit", json!({"projectId": pid, "message": "empty"})).await, "conflict");

    // Diff, log, show, branches.
    write(root, "README.md", "readme v2\n");
    let d = c.call("vcs.diff", json!({"projectId": pid, "target": {"kind": "Unstaged"}})).await;
    assert_eq!(d["files"][0]["path"], "README.md");
    assert_eq!(d["additions"], 1);
    let log = c.call("vcs.log", json!({"projectId": pid, "limit": 10})).await;
    assert_eq!(log["commits"].as_array().unwrap().len(), 1);
    assert_eq!(log["hasMore"], false);
    let show = c.call("vcs.show", json!({"projectId": pid, "path": "README.md", "rev": {"kind": "Head"}})).await;
    assert_eq!(show["content"], "readme\n");
    assert_eq!(
        c.error_code("vcs.show", json!({"projectId": pid, "path": "../x", "rev": {"kind": "Head"}})).await,
        "bad_request"
    );
    c.call("vcs.createBranch", json!({"projectId": pid, "name": "feature"})).await;
    let b = c.call("vcs.branches", json!({"projectId": pid})).await;
    assert_eq!(b["current"], "main");
    assert_eq!(b["branches"].as_array().unwrap().len(), 2);

    // Discard is Dangerous: needs confirm, takes a safety snapshot first.
    assert_eq!(
        c.error_code("vcs.discard", json!({"projectId": pid, "paths": ["README.md"]})).await,
        "approval_required"
    );
    let r = c.call("vcs.discard", json!({"projectId": pid, "paths": ["README.md"], "confirm": true})).await;
    assert_eq!(r["restored"], json!(["README.md"]));
    assert!(r["safetySnapshotId"].is_string());
    assert_eq!(read(root, "README.md").as_deref(), Some("readme\n"));
    let ev = c.wait_event("SnapshotCreated").await;
    assert_eq!(ev["event"]["payload"]["snapshot"]["id"], r["safetySnapshotId"]);
    let fc = c.wait_event("FileChanged").await;
    assert_eq!(fc["event"]["payload"]["path"], "README.md");
    // The discarded edit is recoverable from the safety snapshot.
    let restored = c
        .call(
            "snapshots.restoreFile",
            json!({"snapshotId": r["safetySnapshotId"], "path": "README.md", "confirm": true}),
        )
        .await;
    assert_eq!(restored["restored"], json!(["README.md"]));
    assert_eq!(read(root, "README.md").as_deref(), Some("readme v2\n"));
    c.wait_event("RollbackPerformed").await;
    c.call("vcs.discard", json!({"projectId": pid, "paths": ["README.md"], "confirm": true})).await;

    // Agent worktree.
    let agent = "0190f0b0-1a2b-7c3d-8e4f-00003f9a1c2e";
    let wt = c.call("vcs.addWorktree", json!({"projectId": pid, "agentId": agent, "role": "QA"})).await;
    assert_eq!(wt["worktree"]["branch"], "agent/qa-3f9a1c2e");
    let wt_path = wt["worktree"]["path"].as_str().unwrap().to_owned();
    assert!(Path::new(&wt_path).join("README.md").exists());
    let list = c.call("vcs.worktrees", json!({"projectId": pid})).await;
    let wts = list["worktrees"].as_array().unwrap();
    assert_eq!(wts.len(), 2);
    assert_eq!(wts[1]["agentId"], agent);

    // Work in the worktree through the `worktree` param, then preview + merge.
    write(Path::new(&wt_path), "qa.txt", "tests\n");
    c.call("vcs.stage", json!({"projectId": pid, "worktree": wt_path, "all": true})).await;
    c.call(
        "vcs.commit",
        json!({"projectId": pid, "worktree": wt_path, "message": "qa work",
        "trailers": [{"key": "Orkestr-Agent", "value": agent}]}),
    )
    .await;
    let preview = c.call("vcs.mergePreview", json!({"projectId": pid, "source": "agent/qa-3f9a1c2e"})).await;
    assert_eq!(preview["clean"], true);
    assert_eq!(preview["target"], "main");
    assert_eq!(preview["commitsToMerge"], 1);
    let merged = c.call("vcs.merge", json!({"projectId": pid, "source": "agent/qa-3f9a1c2e"})).await;
    assert_eq!(merged["status"], "Merged");
    assert!(root.join("qa.txt").exists());
    assert_eq!(
        c.error_code("vcs.status", json!({"projectId": pid, "worktree": data.path().to_string_lossy()})).await,
        "not_found",
        "arbitrary directories are rejected"
    );

    // Dirty worktree removal: refused, then forced (confirm + safety snapshot).
    write(Path::new(&wt_path), "wip.txt", "wip\n");
    assert_eq!(c.error_code("vcs.removeWorktree", json!({"projectId": pid, "path": wt_path})).await, "conflict");
    assert_eq!(
        c.error_code("vcs.removeWorktree", json!({"projectId": pid, "path": wt_path, "force": true})).await,
        "approval_required"
    );
    let rm =
        c.call("vcs.removeWorktree", json!({"projectId": pid, "path": wt_path, "force": true, "confirm": true})).await;
    assert!(rm["safetySnapshotId"].is_string());
    assert!(!Path::new(&wt_path).exists());

    // Snapshots: create, list, get, diff, plan + rollback workspace.
    let snap = c.call("snapshots.create", json!({"projectId": pid, "label": "checkpoint"})).await;
    let sid = snap["snapshot"]["id"].as_str().unwrap().to_owned();
    assert_eq!(snap["snapshot"]["branch"], "main");
    assert_eq!(snap["snapshot"]["dirty"], false);
    let detail = c.call("snapshots.get", json!({"snapshotId": sid})).await;
    assert!(detail["entries"].as_array().unwrap().iter().any(|e| e["path"] == "src/app file.rs"));
    write(root, "src/app file.rs", "broken\n");
    write(root, "later.txt", "later\n");
    let diff = c.call("snapshots.diff", json!({"snapshotId": sid, "path": "src/app file.rs"})).await;
    assert_eq!(diff["changes"].as_array().unwrap().len(), 2);
    assert_eq!(diff["file"]["snapshot"], "fn main() {}\n");
    let plan = c.call("snapshots.planRollback", json!({"snapshotId": sid})).await;
    assert_eq!(plan["items"].as_array().unwrap().len(), 2);
    assert_eq!(c.error_code("snapshots.rollbackWorkspace", json!({"snapshotId": sid})).await, "approval_required");
    assert_eq!(
        c.error_code(
            "snapshots.rollbackWorkspace",
            json!({"snapshotId": sid, "confirm": true, "expectedFingerprint": "x"})
        )
        .await,
        "conflict"
    );
    let rb = c
        .call(
            "snapshots.rollbackWorkspace",
            json!({"snapshotId": sid, "confirm": true, "expectedFingerprint": plan["fingerprint"]}),
        )
        .await;
    assert_eq!(rb["restored"], json!(["src/app file.rs"]));
    assert!(rb["preRollbackSnapshotId"].is_string());
    assert_eq!(read(root, "src/app file.rs").as_deref(), Some("fn main() {}\n"));
    assert_eq!(read(root, "later.txt").as_deref(), Some("later\n"), "created file kept");
    let ev = c.wait_event("RollbackPerformed").await;
    assert_eq!(ev["event"]["payload"]["scope"], "Workspace");
    assert_eq!(ev["event"]["projectId"], pid.as_str());

    // Task rollback keeps unrelated changes.
    let task = "0190f0b0-1a2b-7c3d-8e4f-0000000000aa";
    c.call(
        "snapshots.create",
        json!({"projectId": pid, "taskId": task, "taskPhase": "Before", "reason": "TaskCheckpoint"}),
    )
    .await;
    write(root, "task.txt", "task output\n");
    c.call(
        "snapshots.create",
        json!({"projectId": pid, "taskId": task, "taskPhase": "After", "reason": "TaskCheckpoint"}),
    )
    .await;
    write(root, "later.txt", "user kept editing\n");
    assert_eq!(
        c.error_code("snapshots.rollbackTask", json!({"projectId": pid, "taskId": task})).await,
        "approval_required"
    );
    let rt = c.call("snapshots.rollbackTask", json!({"projectId": pid, "taskId": task, "confirm": true})).await;
    assert_eq!(rt["deleted"], json!(["task.txt"]));
    assert!(!root.join("task.txt").exists());
    assert_eq!(read(root, "later.txt").as_deref(), Some("user kept editing\n"));
    let ev = c.wait_event("RollbackPerformed").await;
    assert_eq!(ev["event"]["payload"]["scope"], "Task");
    assert_eq!(ev["event"]["taskId"], task);

    let list = c.call("snapshots.list", json!({"projectId": pid})).await;
    assert!(list["snapshots"].as_array().unwrap().len() >= 6);
    let listed_task = c.call("snapshots.list", json!({"projectId": pid, "taskId": task})).await;
    assert!(listed_task["snapshots"].as_array().unwrap().len() >= 2);
    c.call("snapshots.delete", json!({"snapshotId": sid})).await;
    assert_eq!(c.error_code("snapshots.get", json!({"snapshotId": sid})).await, "not_found");

    core.shutdown().await;
}
