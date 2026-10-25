//! ADR-0051 end-to-end: the first AI source by priority answers 429, the
//! Manager chat switches to the next source at once, names both sources and
//! logs the switch (`providers.sources`).

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
        Self { ws, next: 0, events: Vec::new() }
    }

    async fn raw(&mut self, frame: Value, id: &str) -> Value {
        send(&mut self.ws, frame).await;
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                Some("event") => self.events.push(v),
                _ => panic!("unexpected frame {v}"),
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

    async fn err(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} should fail: {res}");
        res["error"].clone()
    }
}

#[tokio::test]
async fn manager_chat_fails_over_to_the_next_source_by_priority() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();
    let mut c = Client::connect(&core).await;
    let project = c
        .ok("projects.create", json!({"name": "Failover", "rootPath": root.to_string_lossy(), "approvalMode": "Ask"}))
        .await["project"]
        .clone();
    let pid = project["id"].as_str().unwrap().to_owned();
    let providers = c.ok("providers.list", json!({})).await["providers"].as_array().unwrap().clone();
    let backup = providers.iter().find(|p| p["provider"]["kind"] == "Mock").unwrap()["provider"]["id"].clone();
    // The "subscription" that is rate limited (a second offline Mock instance).
    let sub =
        c.ok("providers.create", json!({"kind": "Mock", "name": "Sub A"})).await["provider"]["provider"]["id"].clone();
    c.ok("providers.setManager", json!({"config": {"model": {"providerId": sub, "model": "mock-rate-limit"}}})).await;
    c.ok(
        "providers.setRouting",
        json!({"config": {
            "mode": "Manual", "rules": [], "defaultFallback": [], "maxRateLimitWaitSecs": 0,
            "sourcePriority": [{"providerId": sub}, {"providerId": backup}]
        }}),
    )
    .await;
    let bad = json!({"config": {"purposeSources": [{"purpose": "nope", "sources": []}]}});
    assert_eq!(c.err("providers.setRouting", bad).await["code"], "bad_request");
    let sources = c.ok("providers.sources", json!({})).await;
    assert_eq!(sources["sources"][0]["providerId"], sub);
    assert_eq!(sources["sources"][1]["providerId"], backup);
    assert!(sources["purposes"].as_array().unwrap().iter().any(|p| p == "jarvis"));
    let s1 = c.raw(json!({"type": "sub", "id": "s1", "topics": [format!("project:{pid}"), "global"]}), "s1").await;
    assert_eq!(s1["ok"], true);

    let sent = c.ok("manager.chat", json!({"projectId": pid, "text": "Say hello"})).await;
    let reply_id = sent["replyMessageId"].as_str().unwrap().to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let (mut reply, mut fallback) = (None, None);
    while reply.is_none() || fallback.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "reply {reply:?} fallback {fallback:?}");
        let v = recv(&mut c.ws).await;
        if v["type"] != "event" {
            continue;
        }
        let ev = &v["event"];
        if ev["kind"] == "ManagerMessageAdded" && ev["payload"]["message"]["id"] == reply_id.as_str() {
            reply = Some(ev["payload"]["message"].clone());
        }
        if ev["kind"] == "ProviderFallback" {
            fallback = Some(ev["payload"].clone());
        }
    }
    let reply = reply.unwrap();
    assert_eq!(reply["role"], "Manager", "{reply}");
    let fb = fallback.unwrap();
    assert_eq!(fb["reason"], "rate_limited");
    assert_eq!(fb["fromName"], "Sub A");
    assert_eq!(fb["to"]["providerId"], backup);

    let log = c.ok("providers.sources", json!({})).await["log"].clone();
    assert_eq!(log[0]["fromName"], "Sub A");
    assert_eq!(log[0]["reason"], "rate_limited");
    core.shutdown().await;
}
