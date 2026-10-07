//! rust-analyzer from the Open VSX platform build, driven through the LSP
//! supervisor exactly as the Code panel does it: initialize, didOpen, then
//! diagnostics and hover over the real JSON-RPC channel (network: ignored by default).
//!
//! `cargo test -p orkestr-extensions --test lsp_live -- --ignored --nocapture`

use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::ProjectId;
use orkestr_extensions::lsp::recipes;
use orkestr_extensions::lsp::{LaunchSpec, LspConfig, LspEvent, LspManager};
use orkestr_extensions::{ExtensionService, NoEvents, OpenVsxClient};
use orkestr_protocol::methods::lsp::LspState;
use orkestr_storage::Db;
use serde_json::{json, Value};

async fn next(rx: &mut tokio::sync::mpsc::Receiver<LspEvent>, secs: u64) -> Option<Value> {
    match tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await.ok()?? {
        LspEvent::Message { message, .. } => serde_json::from_str(&message).ok(),
        LspEvent::Status(i) => Some(json!({ "status": format!("{:?}", i.state) })),
    }
}

#[tokio::test]
#[ignore = "needs network access to open-vsx.org"]
async fn rust_analyzer_from_open_vsx_answers_lsp_requests() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    db.migrate(orkestr_extensions::extensions_migrations()).await.unwrap();
    let s = ExtensionService::open(
        db,
        dir.path().join("extensions"),
        OpenVsxClient::new(None).unwrap(),
        Arc::new(NoEvents),
    )
    .await
    .unwrap();
    s.install("rust-lang", "rust-analyzer", None).await.unwrap();
    let dirs = s.enabled_dirs().await.unwrap();
    let ra = recipes::resolve(recipes::recipe("rust-analyzer").unwrap(), &dirs, &|_| None).expect("bundled server");
    orkestr_platform::fs::make_executable(&ra.program).unwrap();

    let proj = dir.path().join("hello");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
        .unwrap();
    let text = "mod missing_module;\n\nfn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n\nfn main() {\n    println!(\"{}\", greet(\"world\"));\n}\n";
    std::fs::write(proj.join("src/main.rs"), text).unwrap();
    let norm = proj.display().to_string().replace('\\', "/");
    let root = format!("file:///{}", norm.trim_start_matches('/'));
    let file = format!("{root}/src/main.rs");

    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let m = LspManager::new(tx, LspConfig::default(), Arc::new(orkestr_platform::NativeProcessControl));
    let info = m
        .start(LaunchSpec {
            server_id: "rust-analyzer".into(),
            project_id: ProjectId::new(),
            root_uri: root.clone(),
            cwd: proj.clone(),
            program: ra.program.clone(),
            args: ra.args.clone(),
            stop: None,
        })
        .unwrap();
    let sid = info.session_id;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.unwrap().unwrap();
        if let LspEvent::Status(i) = ev {
            assert_ne!(i.state, LspState::Failed, "{:?}", i.error);
            if i.state == LspState::Running {
                break;
            }
        }
    }
    let send = |v: Value| m.send(sid, v.to_string()).unwrap();
    send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "processId": null, "rootUri": root, "workspaceFolders": [{"uri": root, "name": "hello"}],
        "capabilities": {"textDocument": {"hover": {"contentFormat": ["markdown"]}, "publishDiagnostics": {}}}
    }}));
    let mut caps = Value::Null;
    let mut diagnostics: Vec<Value> = Vec::new();
    let mut hover = Value::Null;
    let mut hover_pending = false;
    let mut next_id = 2;
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline && (hover.is_null() || diagnostics.is_empty()) {
        if caps.is_object() && hover.is_null() && !hover_pending {
            next_id += 1;
            send(json!({"jsonrpc":"2.0","id":next_id,"method":"textDocument/hover","params":{
                "textDocument": {"uri": file}, "position": {"line": 7, "character": 21}}}));
            hover_pending = true;
        }
        let Some(msg) = next(&mut rx, 5).await else { continue };
        if msg.get("id") == Some(&json!(1)) {
            caps = msg["result"]["capabilities"].clone();
            send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
            send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
                "uri": file, "languageId": "rust", "version": 1, "text": text}}}));
        } else if msg.get("method").is_some() && msg.get("id").is_some() {
            let result = if msg["method"] == "workspace/configuration" {
                Value::Array(
                    msg["params"]["items"]
                        .as_array()
                        .map(|a| a.iter().map(|_| Value::Null).collect())
                        .unwrap_or_default(),
                )
            } else {
                Value::Null
            };
            send(json!({"jsonrpc":"2.0","id": msg["id"], "result": result}));
        } else if msg["method"] == "textDocument/publishDiagnostics" {
            let list = msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default();
            if msg["params"]["uri"].as_str().is_some_and(|u| u.ends_with("main.rs")) && !list.is_empty() {
                diagnostics = list;
            }
        } else if msg.get("id").and_then(Value::as_i64).is_some_and(|i| i >= 3) {
            hover_pending = false;
            if msg["result"].is_object() {
                hover = msg["result"].clone();
            } else {
                // Still indexing: ask again shortly.
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    println!(
        "capabilities: hover={} completion={} definition={} references={} rename={} formatting={} signatureHelp={}",
        caps["hoverProvider"],
        caps["completionProvider"].is_object(),
        caps["definitionProvider"],
        caps["referencesProvider"],
        caps["renameProvider"].is_object() || caps["renameProvider"] == true,
        caps["documentFormattingProvider"],
        caps["signatureHelpProvider"].is_object()
    );
    println!("diagnostics: {:?}", diagnostics.iter().map(|d| d["message"].as_str().unwrap_or("")).collect::<Vec<_>>());
    println!("hover: {}", hover["contents"]["value"].as_str().unwrap_or("").replace('\n', " "));
    assert!(!diagnostics.is_empty(), "rust-analyzer published diagnostics");
    assert!(hover["contents"]["value"].as_str().unwrap_or("").contains("fn greet"), "hover shows the signature");
    m.shutdown(Duration::from_secs(10)).await;
    assert!(m.list().is_empty(), "session stopped and removed");
}
