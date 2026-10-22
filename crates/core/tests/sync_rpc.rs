//! `sync.*` over real WebSocket RPC: two cores + an in-process sync server
//! (Seamless Continue, privacy policy, deferred `sync.rotateKeys`).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(30), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

async fn connect(core: &orkestr_core::CoreHandle) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
    send(&mut ws, json!({"type": "auth", "token": core.token})).await;
    assert_eq!(recv(&mut ws).await["ok"], true);
    ws
}

async fn call(ws: &mut Ws, method: &str, params: Value) -> Value {
    let id = format!("{method}-{}", rand_id());
    send(ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
    loop {
        let v = recv(ws).await;
        if v["type"] == "res" && v["id"] == id.as_str() {
            return v;
        }
    }
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

async fn ok(ws: &mut Ws, method: &str, params: Value) -> Value {
    let res = call(ws, method, params).await;
    assert_eq!(res["ok"], true, "{method}: {res}");
    res["result"].clone()
}

async fn sync_server(dir: &std::path::Path) -> String {
    let auth: orkestr_sync::server::AuthFn = Arc::new(|t: &str| (t == "tok").then(|| "acct".to_owned()));
    let state = orkestr_sync::SyncServerState::open(dir, auth).unwrap();
    let app = axum::Router::new().nest("/v1/sync", orkestr_sync::server_router(state));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

#[tokio::test]
async fn seamless_continue_between_two_cores() {
    let tmp = tempfile::tempdir().unwrap();
    let url = sync_server(&tmp.path().join("server")).await;
    let root_a = tmp.path().join("work-a");
    let root_b = tmp.path().join("work-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let core_a = orkestr_core::start(CoreConfig::with_data_dir(tmp.path().join("a"))).await.unwrap();
    let core_b = orkestr_core::start(CoreConfig::with_data_dir(tmp.path().join("b"))).await.unwrap();
    let mut a = connect(&core_a).await;
    let mut b = connect(&core_b).await;

    let pa = ok(&mut a, "projects.create", json!({"name": "Demo", "rootPath": root_a.to_string_lossy()})).await;
    let pa = pa["project"]["id"].as_str().unwrap().to_owned();
    let pb = ok(&mut b, "projects.create", json!({"name": "Demo", "rootPath": root_b.to_string_lossy()})).await;
    let pb = pb["project"]["id"].as_str().unwrap().to_owned();

    let st = ok(&mut a, "sync.status", json!({"projectId": pa})).await;
    assert_eq!(st["configured"], false);
    let st = ok(&mut a, "sync.configure", json!({"projectId": pa, "serverUrl": url, "accessToken": "tok"})).await;
    let key = st["generatedRemoteKey"].as_str().expect("generated key").to_owned();
    let st = ok(
        &mut b,
        "sync.configure",
        json!({"projectId": pb, "serverUrl": url, "accessToken": "tok", "remoteKey": key}),
    )
    .await;
    assert_eq!(st["configured"], true);
    assert!(st.get("generatedRemoteKey").is_none());

    ok(&mut a, "tasks.create", json!({"projectId": pa, "title": "Backend API"})).await;
    std::fs::write(root_a.join("main.rs"), "fn main() { /* wip */ }\n").unwrap();
    let ui = json!({"openEditors": [{"path": "main.rs", "cursor": 3}], "activePanel": "editor"});
    let res = ok(&mut a, "sync.continueOn", json!({"projectId": pa, "uiState": ui})).await;
    assert!(res["report"]["pushed"].as_u64().unwrap() > 0);
    assert!(res["summary"].as_str().unwrap().contains("Backend API"));

    let res = ok(&mut b, "sync.resume", json!({"projectId": pb})).await;
    assert_eq!(res["uiState"], ui);
    assert!(res["summary"].as_str().unwrap().contains("Backend API"), "{res}");
    assert_eq!(std::fs::read_to_string(root_b.join("main.rs")).unwrap(), "fn main() { /* wip */ }\n");

    // Conflicts: concurrent edits on both cores.
    std::fs::write(root_a.join("main.rs"), "A\n").unwrap();
    std::fs::write(root_b.join("main.rs"), "B\n").unwrap();
    ok(&mut a, "sync.now", json!({"projectId": pa})).await;
    let r = ok(&mut b, "sync.now", json!({"projectId": pb})).await;
    assert_eq!(r["conflicts"], 1);
    let list = ok(&mut b, "sync.listConflicts", json!({"projectId": pb})).await;
    let c = &list["conflicts"][0];
    assert_eq!(c["path"], "main.rs");
    let id = c["id"].as_str().unwrap().to_owned();
    let bad =
        call(&mut b, "sync.resolveConflict", json!({"projectId": pa, "conflictId": id, "resolution": "keepLocal"}))
            .await;
    assert_eq!(bad["error"]["code"], "not_found", "conflict ids are scoped to the project");
    ok(&mut b, "sync.resolveConflict", json!({"projectId": pb, "conflictId": id, "resolution": "keepLocal"})).await;
    assert_eq!(ok(&mut b, "sync.status", json!({"projectId": pb})).await["conflicts"], 0);

    // Deferred: key rotation.
    let res = call(&mut a, "sync.rotateKeys", json!({"projectId": pa})).await;
    assert_eq!(res["ok"], false);

    // Privacy: No Remote Sync в‡’ forbidden.
    let secret = ok(
        &mut a,
        "projects.create",
        json!({"name": "Private", "rootPath": root_a.to_string_lossy(),
               "privacy": {"ai": {"mode": "CloudAllowed"}, "sync": "NoRemoteSync"}}),
    )
    .await;
    let sp = secret["project"]["id"].as_str().unwrap().to_owned();
    let res = call(&mut a, "sync.configure", json!({"projectId": sp, "serverUrl": url, "accessToken": "tok"})).await;
    assert_eq!(res["error"]["code"], "forbidden", "{res}");
    assert_eq!(res["error"]["data"]["syncError"], "policyDenied");
    let res = call(&mut a, "sync.now", json!({"projectId": sp})).await;
    assert_eq!(res["error"]["code"], "forbidden");

    core_a.shutdown().await;
    core_b.shutdown().await;
}
