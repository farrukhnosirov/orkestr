//! `jarvis.*` over a real WebSocket (ADR-0049): settings, local template
//! answers with the form of address, AI conversation through the offline
//! Mock provider (streamed `JarvisMessage` deltas), rolling memory, record
//! and forget.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

struct Client {
    ws: Ws,
    next: u64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
        assert_eq!(recv(&mut ws).await["ok"], true);
        Self { ws, next: 0, events: Vec::new() }
    }

    async fn raw(&mut self, frame: Value, id: &str) -> Value {
        self.ws.send(Message::Text(frame.to_string().into())).await.unwrap();
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => {}
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.raw(json!({"type": "req", "id": id, "method": method, "params": params}), &id).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method}: {res}");
        res["result"].clone()
    }

    async fn err_code(&mut self, method: &str, params: Value) -> String {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} should fail: {res}");
        res["error"]["code"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn jarvis_settings_templates_chat_memory() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Defaults + bank size.
    let r = c.ok("jarvis.getConfig", json!({})).await;
    assert_eq!(r["config"]["address"], "Sir");
    assert_eq!(r["config"]["persona"], true);
    assert!(r["bank"]["questions"].as_u64().unwrap() >= 10_000, "{r}");
    let mut config = r["config"].clone();

    // Template answers: instant, with the form of address; unrelated text falls through.
    let m = c.ok("jarvis.match", json!({"text": "Привет, Джарвис!", "lang": "ru"})).await;
    assert_eq!(m["matched"]["intent"], "greet.hello", "{m}");
    assert!(m["matched"]["text"].as_str().unwrap().contains("сэр"), "{m}");
    let m = c.ok("jarvis.match", json!({"text": "который час", "lang": "ru"})).await;
    assert_eq!(m["matched"]["intent"], "time.now");
    let m = c.ok("jarvis.match", json!({"text": "объясни, как работает протокол TCP", "lang": "ru"})).await;
    assert_eq!(m["matched"], Value::Null, "{m}");
    let m = c.ok("jarvis.match", json!({"text": "режим разговора", "lang": "ru"})).await;
    assert_eq!(m["matched"]["action"], "mode.chat");
    let m = c
        .ok(
            "jarvis.match",
            json!({"text": "повтори что ты сказал", "lang": "ru", "lastReply": "Открываю YouTube, сэр."}),
        )
        .await;
    assert!(m["matched"]["text"].as_str().unwrap().contains("Открываю YouTube"), "{m}");
    assert_eq!(c.ok("jarvis.match", json!({"text": "привет", "lang": "en"})).await["matched"], Value::Null);

    // Settings: boss; validation; templates off.
    config["address"] = json!("Boss");
    c.ok("jarvis.setConfig", json!({"config": config.clone()})).await;
    let m = c.ok("jarvis.match", json!({"text": "спасибо", "lang": "ru"})).await;
    assert!(m["matched"]["text"].as_str().unwrap().contains("босс"), "{m}");
    let mut bad = config.clone();
    bad["address"] = json!("Name");
    bad["userName"] = json!("");
    assert_eq!(c.err_code("jarvis.setConfig", json!({"config": bad})).await, "bad_request");
    let mut off = config.clone();
    off["localTemplates"] = json!(false);
    c.ok("jarvis.setConfig", json!({"config": off})).await;
    assert_eq!(c.ok("jarvis.match", json!({"text": "привет", "lang": "ru"})).await["matched"], Value::Null);

    // The offline Mock chosen explicitly answers (never a real CLI / API in
    // tests: routing points at the Mock too); deltas stream; memory persists.
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let mock = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    let model = json!({"providerId": mock, "model": "mock-smart"});
    c.ok(
        "providers.setRouting",
        json!({"config": {"mode": "Manual", "rules": [], "defaultFallback": [model], "preferLocal": true, "maxRateLimitWaitSecs": 0}}),
    )
    .await;
    config["model"] = model;
    config["localTemplates"] = json!(true);
    c.ok("jarvis.setConfig", json!({"config": config.clone()})).await;
    let sub = c.raw(json!({"type": "sub", "id": "s1", "topics": ["global"]}), "s1").await;
    assert_eq!(sub["ok"], true);
    c.ok("jarvis.record", json!({"user": "привет", "reply": "Здравствуйте, босс.", "source": "template"})).await;
    let r = c
        .ok(
            "jarvis.chat",
            json!({"text": "Расскажи коротко о чёрных дырах", "lang": "ru", "messageId": "m-1", "spoken": true}),
        )
        .await;
    assert_eq!(r["messageId"], "m-1");
    assert!(!r["text"].as_str().unwrap().is_empty(), "{r}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let done = c.events.iter().any(|e| {
            e["event"]["kind"] == "JarvisMessage"
                && e["event"]["payload"]["messageId"] == "m-1"
                && e["event"]["payload"]["done"] == true
        });
        if done {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no JarvisMessage done event");
        let v = recv(&mut c.ws).await;
        if v["type"] == "event" {
            c.events.push(v);
        }
    }
    let streamed: String = c
        .events
        .iter()
        .filter(|e| e["event"]["kind"] == "JarvisMessage" && e["event"]["payload"]["messageId"] == "m-1")
        .map(|e| e["event"]["payload"]["delta"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(!streamed.is_empty(), "deltas streamed");

    let h = c.ok("jarvis.history", json!({})).await;
    let msgs = h["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4, "{h}");
    assert_eq!(msgs[1]["source"], "template");
    assert_eq!(msgs[3]["source"], "ai");
    assert_eq!(c.ok("jarvis.history", json!({"limit": 1})).await["messages"].as_array().unwrap().len(), 1);

    // «забудь разговор»
    c.ok("jarvis.forget", json!({})).await;
    assert!(c.ok("jarvis.history", json!({})).await["messages"].as_array().unwrap().is_empty());
    assert_eq!(c.err_code("jarvis.chat", json!({"text": "  ", "lang": "ru"})).await, "bad_request");
    c.ok("jarvis.cancel", json!({"messageId": "nothing"})).await;
    core.shutdown().await;
}
