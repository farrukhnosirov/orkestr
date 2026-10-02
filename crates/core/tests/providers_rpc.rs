//! End-to-end `providers.*` over the real WebSocket: first-start Mock,
//! kinds, health, models, connection test, CRUD with an OpenAI-compatible mock
//! server, credentials (key never echoed), Manager + routing settings, events.

use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(20), ws.next()).await.expect("timeout").expect("closed").unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected message {other:?}"),
        }
    }
}

struct Client {
    ws: Ws,
    next: u64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        send(&mut ws, json!({"type": "auth", "token": core.token})).await;
        let res = recv(&mut ws).await;
        assert_eq!(res["ok"], true, "{res}");
        let mut c = Self { ws, next: 0, events: vec![] };
        send(&mut c.ws, json!({"type": "sub", "id": "s", "topics": ["global"]})).await;
        c.wait_res("s").await;
        c
    }

    async fn wait_res(&mut self, id: &str) -> Value {
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
            }
        }
    }

    /// Raw response frame.
    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        send(&mut self.ws, json!({"type": "req", "id": id, "method": method, "params": params})).await;
        self.wait_res(&id).await
    }

    /// Successful result (panics on error).
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
}

/// Minimal OpenAI-compatible server recording request headers.
async fn openai_compatible_server() -> (String, Arc<Mutex<Vec<HeaderMap>>>) {
    let seen: Arc<Mutex<Vec<HeaderMap>>> = Arc::default();
    let s2 = Arc::clone(&seen);
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |headers: HeaderMap| {
            let seen = Arc::clone(&s2);
            async move {
                seen.lock().push(headers);
                let body = concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":1}}\n\n",
                    "data: [DONE]\n\n"
                );
                ([("content-type", "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/v1"), seen)
}

#[tokio::test]
async fn providers_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    // Provider keys go through the security SecretService; keep tests off
    // the real OS keyring.
    let mut config = CoreConfig::with_data_dir(dir.path());
    config.secret_backend = orkestr_security::SecretBackendChoice::Memory;
    let core = orkestr_core::start(config).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Kinds.
    let kinds = c.call("providers.kinds", json!({})).await;
    let kinds = kinds["kinds"].as_array().unwrap().clone();
    assert_eq!(kinds.len(), 9);
    let mock_kind = kinds.iter().find(|k| k["kind"] == "Mock").unwrap();
    assert_eq!(mock_kind["displayName"], "Mock (offline demo)");
    let cc = kinds.iter().find(|k| k["kind"] == "ClaudeCode").unwrap();
    assert!(cc["detected"].is_boolean());
    assert!(cc["loginInstructions"].as_str().unwrap().contains("claude"));

    // First start registered the Mock provider.
    let list = c.call("providers.list", json!({})).await;
    let providers = list["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").expect("mock auto-registered").clone();
    assert_eq!(mock["provider"]["name"], "Mock (offline demo)");
    assert_eq!(mock["locality"], "Local");
    let mock_id = mock["provider"]["id"].as_str().unwrap().to_owned();

    let health = c.call("providers.health", json!({"providerId": mock_id})).await;
    assert_eq!(health["status"]["health"], "Healthy");
    assert_eq!(health["status"]["authState"], "Authenticated");

    let models = c.call("providers.models", json!({"providerId": mock_id})).await;
    assert!(models["models"].as_array().unwrap().iter().any(|m| m["id"] == "mock-smart"));

    let test = c.call("providers.test", json!({"providerId": mock_id})).await;
    assert_eq!(test["ok"], true, "{test}");
    assert!(test["sample"].as_str().unwrap().contains("Mock (offline demo)"));

    // OpenAI-compatible provider against a local mock server.
    let (base, seen) = openai_compatible_server().await;
    assert_eq!(
        c.error_code("providers.create", json!({"kind": "OpenAiCompatible", "name": "Gateway", "baseUrl": base, "headers": {"Authorization": "Bearer leak"}})).await,
        "bad_request",
        "credential headers must go through the vault"
    );
    assert_eq!(
        c.error_code("providers.create", json!({"kind": "OpenAiCompatible", "baseUrl": "ftp://nope"})).await,
        "bad_request"
    );
    let created = c.call("providers.create", json!({"kind": "OpenAiCompatible", "name": "Gateway", "baseUrl": base, "headers": {"X-Team": "core"}, "defaultModel": "local-model"})).await;
    let pid = created["provider"]["provider"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["provider"]["hasCredential"], false);
    assert_eq!(created["provider"]["locality"], "Local");
    assert_eq!(
        c.error_code("providers.create", json!({"kind": "OpenAiCompatible", "name": "gateway", "baseUrl": base})).await,
        "conflict"
    );

    let key = format!("orkestr-test-key-{}", uuid_like());
    let with_key = c.call_raw("providers.setCredential", json!({"providerId": pid, "apiKey": key})).await;
    assert_eq!(with_key["ok"], true, "{with_key}");
    assert_eq!(with_key["result"]["provider"]["hasCredential"], true);
    // The key lives in the security vault (reference only, value never listed).
    let secrets = c.call("secrets.list", json!({})).await;
    let refs = secrets["secrets"].as_array().expect("secrets list");
    assert!(refs.iter().any(|r| r["name"].as_str().is_some_and(|n| n.starts_with("provider."))), "{secrets}");
    assert!(!secrets.to_string().contains(&key), "secret value leaked into secrets.list");
    assert!(!with_key.to_string().contains(&key), "the key is never echoed");

    let test = c.call("providers.test", json!({"providerId": pid})).await;
    assert_eq!(test["ok"], true, "{test}");
    assert_eq!(test["sample"], "OK");
    assert_eq!(test["inputTokens"], 12);
    {
        let seen = seen.lock();
        let last = seen.last().expect("server called");
        assert_eq!(last["authorization"], format!("Bearer {key}"));
        assert_eq!(last["x-team"], "core");
    }
    let got = c.call("providers.get", json!({"providerId": pid})).await;
    assert_eq!(got["provider"]["status"]["authState"], "Authenticated");
    assert!(!got.to_string().contains(&key));

    let cleared = c.call("providers.clearCredential", json!({"providerId": pid})).await;
    assert_eq!(cleared["provider"]["hasCredential"], false);

    let updated = c
        .call("providers.update", json!({"providerId": pid, "patch": {"name": "Gateway (off)", "enabled": false}}))
        .await;
    assert_eq!(updated["provider"]["provider"]["name"], "Gateway (off)");
    assert_eq!(updated["provider"]["provider"]["enabled"], false);

    // Manager + routing settings.
    let mgr = c.call("providers.getManager", json!({})).await;
    assert_eq!(mgr["config"]["maxParallelAgents"], 4);
    let cfg = json!({"config": {
        "model": {"providerId": mock_id, "model": "mock-smart"},
        "reasoning": "High",
        "systemInstructions": "Prefer small PRs.",
        "maxParallelAgents": 3,
        "budgetUsd": 2.5,
        "timeLimitMins": 30,
        "contextPolicy": "Balanced",
        "fallbackChain": [{"providerId": pid, "model": "local-model"}],
        "approvalPolicy": "SemiAuto"
    }});
    let saved = c.call("providers.setManager", cfg.clone()).await;
    assert_eq!(saved, cfg);
    assert_eq!(c.call("providers.getManager", json!({})).await, cfg);
    assert_eq!(c.error_code("providers.setManager", json!({"config": {"maxParallelAgents": 0}})).await, "bad_request");

    let routing = json!({"config": {"mode": "Manual", "rules": [{"purpose": "Backend", "model": {"providerId": mock_id, "model": "mock-smart"}, "fallback": []}], "defaultFallback": [], "preferLocal": true, "maxRateLimitWaitSecs": 30}});
    assert_eq!(c.call("providers.setRouting", routing.clone()).await, routing);
    assert_eq!(c.call("providers.getRouting", json!({})).await, routing);
    let bogus = json!({"config": {"rules": [{"purpose": "Backend", "model": {"providerId": "0199a213-81c0-7800-8aa1-bbab2a035a53", "model": "x"}}]}});
    assert_eq!(c.error_code("providers.setRouting", bogus).await, "bad_request");

    // Delete prunes references.
    let del = c.call("providers.delete", json!({"providerId": pid})).await;
    assert_eq!(del["providerId"], pid);
    assert_eq!(c.error_code("providers.get", json!({"providerId": pid})).await, "not_found");
    let mgr = c.call("providers.getManager", json!({})).await;
    assert_eq!(mgr["config"]["fallbackChain"], json!([]));

    // ProviderStatusChanged events were published on the global topic.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !c.events.iter().any(|e| e["event"]["kind"] == "ProviderStatusChanged") {
        assert!(tokio::time::Instant::now() < deadline, "no ProviderStatusChanged event");
        let v = recv(&mut c.ws).await;
        c.events.push(v);
    }
    for e in &c.events {
        assert!(!e.to_string().contains(&key), "events never carry the key");
    }
    core.shutdown().await;
}

fn uuid_like() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}
