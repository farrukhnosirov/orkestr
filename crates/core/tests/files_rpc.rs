//! End-to-end `files.*` over a real WebSocket against a core on a temp dir.

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

    async fn send_frame(&mut self, frame: Value) -> Value {
        let id = frame["id"].as_str().unwrap().to_owned();
        self.ws.send(Message::Text(frame.to_string().into())).await.unwrap();
        loop {
            let v = self.recv().await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id.as_str() => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.send_frame(json!({"type": "req", "id": id, "method": method, "params": params})).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method} failed: {res}");
        res["result"].clone()
    }

    async fn err(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} unexpectedly succeeded: {res}");
        res["error"].clone()
    }

    /// Waits for a FileChanged event matching `pred`.
    async fn file_event(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) =
                self.events.iter().find(|e| e["event"]["kind"] == "FileChanged" && pred(&e["event"]["payload"]))
            {
                return e["event"].clone();
            }
            assert!(tokio::time::Instant::now() < deadline, "no matching FileChanged event in {:?}", self.events);
            let v = self.recv().await;
            if v["type"] == "event" {
                self.events.push(v);
            }
        }
    }
}

#[tokio::test]
async fn files_area_end_to_end() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::create_dir_all(w.join("src")).unwrap();
    std::fs::create_dir_all(w.join("target")).unwrap();
    std::fs::write(w.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(w.join("src/main.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
    std::fs::write(w.join("target/junk.rs"), "hello from build\n").unwrap();

    let core = orkestr_core::start(CoreConfig::with_data_dir(data.path())).await.unwrap();
    let mut c = Client::connect(&core).await;

    let project = c.ok("projects.create", json!({"name": "Files", "rootPath": w.to_string_lossy()})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();
    let sub = c.send_frame(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}")]})).await;
    assert_eq!(sub["ok"], true, "{sub}");

    // list: dirs first, gitignored hidden unless asked.
    let list = c.ok("files.list", json!({"projectId": pid})).await;
    let names: Vec<_> = list["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["src", ".gitignore"]);
    let list = c.ok("files.list", json!({"projectId": pid, "showIgnored": true})).await;
    let target = list["entries"].as_array().unwrap().iter().find(|e| e["name"] == "target").unwrap();
    assert_eq!(target["ignored"], true);
    assert_eq!(target["kind"], "dir");

    // read + optimistic write.
    let read = c.ok("files.read", json!({"projectId": pid, "path": "src/main.rs"})).await;
    assert_eq!(read["language"], "rust");
    assert_eq!(read["encoding"], "utf8");
    assert_eq!(read["binary"], false);
    let hash = read["hash"].as_str().unwrap().to_owned();
    let written = c
        .ok(
            "files.write",
            json!({"projectId": pid, "path": "src/main.rs", "content": "fn main() {}\n", "expectedHash": hash}),
        )
        .await;
    assert_eq!(written["created"], false);
    let ev = c.file_event(|p| p["path"] == "src/main.rs" && p["change"] == "Modified").await;
    assert_eq!(ev["payload"]["actor"], "user");
    assert_eq!(ev["projectId"], pid.as_str());
    assert!(ev["correlationId"].is_string());

    let stale = c
        .err("files.write", json!({"projectId": pid, "path": "src/main.rs", "content": "x", "expectedHash": hash}))
        .await;
    assert_eq!(stale["code"], "conflict");
    assert_eq!(stale["data"]["reason"], "stale");
    assert_eq!(stale["data"]["actualHash"], written["hash"]);

    // Agent writes go through the permission engine: in the default Ask
    // mode they need approval; in Semi-Auto, writes inside the root pass.
    let agent = "0190f5c2-0000-7000-8000-000000000001";
    let gated = c
        .err(
            "files.write",
            json!({"projectId": pid, "path": "src/gen.rs", "content": "// gen
", "create": true, "agentId": agent}),
        )
        .await;
    assert_eq!(gated["code"], "approval_required", "{gated}");
    let gated =
        c.err("files.delete", json!({"projectId": pid, "path": "src", "recursive": true, "agentId": agent})).await;
    assert_eq!(gated["code"], "approval_required", "{gated}");
    c.ok("permissions.setMode", json!({"projectId": pid, "mode": "SemiAuto"})).await;
    let escape = c
        .err(
            "files.write",
            json!({"projectId": pid, "path": "../x.rs", "content": "x", "create": true, "agentId": agent}),
        )
        .await;
    assert!(escape["code"] == "forbidden" || escape["code"] == "bad_request", "{escape}");

    // Agent attribution.
    c.ok(
        "files.write",
        json!({"projectId": pid, "path": "src/gen.rs", "content": "// gen\n", "create": true, "agentId": agent}),
    )
    .await;
    let ev = c.file_event(|p| p["path"] == "src/gen.rs").await;
    assert_eq!(ev["payload"]["actor"], "agent");
    assert_eq!(ev["payload"]["agentId"], agent);
    assert_eq!(ev["payload"]["change"], "Created");
    assert_eq!(ev["agentId"], agent);

    // Confinement.
    for bad in ["../outside.txt", "C:\\Windows\\win.ini", "/etc/passwd", "\\\\server\\share\\x", "a.txt:ads", "NUL"] {
        let e = c.err("files.read", json!({"projectId": pid, "path": bad})).await;
        assert_eq!(e["code"], "bad_request", "{bad}: {e}");
    }
    let e = c
        .err("files.write", json!({"projectId": pid, "path": ".git/hooks/pre-commit", "content": "x", "create": true}))
        .await;
    assert_eq!(e["code"], "forbidden");

    // create / mkdir / rename / search / find.
    c.ok("files.mkdir", json!({"projectId": pid, "path": "docs/guide"})).await;
    let created = c
        .ok("files.create", json!({"projectId": pid, "path": "docs/guide/intro.md", "content": "# hello world\n"}))
        .await;
    assert_eq!(created["entry"]["kind"], "file");
    let e = c.err("files.create", json!({"projectId": pid, "path": "docs/guide/intro.md"})).await;
    assert_eq!(e["data"]["reason"], "exists");
    let renamed =
        c.ok("files.rename", json!({"projectId": pid, "from": "docs/guide/intro.md", "to": "docs/README.md"})).await;
    assert_eq!(renamed["entry"]["path"], "docs/README.md");
    let ev = c.file_event(|p| p["change"] == "Renamed").await;
    assert_eq!(ev["payload"]["oldPath"], "docs/guide/intro.md");

    let search = c.ok("files.search", json!({"projectId": pid, "query": "HELLO"})).await;
    let hits: Vec<_> = search["matches"].as_array().unwrap().iter().map(|m| m["path"].as_str().unwrap()).collect();
    assert_eq!(hits, vec!["docs/README.md"], "target/ is gitignored, main.rs no longer says hello");
    let e = c.err("files.search", json!({"projectId": pid, "query": "(", "isRegex": true})).await;
    assert_eq!(e["code"], "bad_request");
    let find = c.ok("files.find", json!({"projectId": pid, "query": "readme"})).await;
    assert_eq!(find["matches"][0]["path"], "docs/README.md");

    // delete → trash → restore.
    let e = c.err("files.delete", json!({"projectId": pid, "path": "docs"})).await;
    assert_eq!(e["data"]["reason"], "recursive_required");
    let del = c.ok("files.delete", json!({"projectId": pid, "path": "docs", "recursive": true})).await;
    assert_eq!(del["trashed"].as_array().unwrap().len(), 1);
    assert!(!w.join("docs").exists());
    let trash = c.ok("files.trash", json!({"projectId": pid})).await;
    let tid = trash["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(trash["items"][0]["path"], "docs/README.md");
    let restored = c.ok("files.restore", json!({"projectId": pid, "trashId": tid})).await;
    assert_eq!(restored["path"], "docs/README.md");
    assert_eq!(std::fs::read_to_string(w.join("docs/README.md")).unwrap(), "# hello world\n");

    // Watcher: external change → FileChanged with actor external.
    let watch = c.ok("files.watch", json!({"projectId": pid})).await;
    assert_eq!(watch["watching"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(w.join("external.txt"), "from another editor").unwrap();
    let ev = c.file_event(|p| p["path"] == "external.txt").await;
    assert_eq!(ev["payload"]["actor"], "external");
    let unwatch = c.ok("files.unwatch", json!({"projectId": pid})).await;
    assert_eq!(unwatch["watching"], false);

    // openExternal: only allow-listed IDEs (nothing is launched here).
    let e = c.err("files.openExternal", json!({"projectId": pid, "target": "ide", "ide": "powershell"})).await;
    assert_eq!(e["code"], "bad_request");

    // Workspace resolution errors.
    let e = c.err("files.list", json!({"projectId": pid, "worktreePath": w.to_string_lossy()})).await;
    assert!(e["code"] == "forbidden" || e["code"] == "bad_request", "{e}");
    let bare = c.ok("projects.create", json!({"name": "No Root"})).await;
    let e = c.err("files.list", json!({"projectId": bare["project"]["id"]})).await;
    assert_eq!(e["code"], "bad_request");
    let e = c.err("files.list", json!({"projectId": "0190f5c2-0000-7000-8000-00000000ffff"})).await;
    assert_eq!(e["code"], "not_found");

    core.shutdown().await;
}
