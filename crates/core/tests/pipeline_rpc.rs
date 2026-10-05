//! `pipeline.*` over the real WebSocket: configuration (defaults, per-kind
//! overrides, validation, reset), an empty pipeline view of a task, the
//! listing, and `pipeline.start` refusing a task whose agent did not finish.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    next: u64,
}

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
        let mut c = Self { ws, next: 0 };
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

    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        self.ws
            .send(Message::Text(
                json!({"type": "req", "id": id, "method": method, "params": params}).to_string().into(),
            ))
            .await
            .unwrap();
        loop {
            let v = self.recv().await;
            if v["type"] == "res" && v["id"] == id.as_str() {
                return v;
            }
        }
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
}

#[tokio::test]
async fn pipeline_config_and_views_over_rpc() {
    let data = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(data.path())).await.unwrap();
    let mut c = Client::connect(&core).await;
    let project = c.call("projects.create", json!({"name": "Pipeline"})).await;
    let pid = project["project"]["id"].as_str().unwrap().to_owned();

    let cfg = c.call("pipeline.getConfig", json!({"projectId": pid})).await;
    assert_eq!(cfg["enabled"], true);
    assert_eq!(cfg["customized"], false);
    assert_eq!(cfg["stages"], json!(["Author", "Review", "Build", "Test", "Security", "Merge"]));
    assert_eq!(cfg["healing"]["maxAttempts"], 3);
    assert_eq!(cfg["merge"], "PerPolicy");

    let set = c
        .call(
            "pipeline.setConfig",
            json!({
                "projectId": pid,
                "stages": ["Test", "Review", "Review"],
                "overrides": [{"kind": "Docs", "stages": ["Review"]}],
                "healing": {"maxAttempts": 2, "maxWallMins": 15, "stopOnRepeat": true, "escalateModel": false},
                "merge": "Propose",
            }),
        )
        .await;
    assert_eq!(set["stages"], json!(["Author", "Review", "Test"]), "canonical order, author always on");
    assert_eq!(set["overrides"][0]["stages"], json!(["Author", "Review"]));
    assert_eq!(set["healing"]["maxWallMins"], 15);
    assert_eq!(set["customized"], true);
    assert_eq!(
        c.error_code(
            "pipeline.setConfig",
            json!({"projectId": pid, "healing": {"maxAttempts": 1, "maxWallMins": 0, "stopOnRepeat": true, "escalateModel": true}}),
        )
        .await,
        "bad_request"
    );
    assert_eq!(c.call("pipeline.getConfig", json!({"projectId": pid})).await["merge"], "Propose");
    let reset = c.call("pipeline.setConfig", json!({"projectId": pid, "reset": true})).await;
    assert_eq!(reset["customized"], false);

    let task = c.call("tasks.create", json!({"projectId": pid, "title": "Implement the greeter"})).await;
    let tid = task["task"]["id"].as_str().unwrap().to_owned();
    let view = c.call("pipeline.get", json!({"taskId": tid})).await;
    assert_eq!(view["pipeline"], Value::Null);
    assert_eq!(view["live"], false);
    assert_eq!(c.call("pipeline.list", json!({"projectId": pid})).await["pipelines"], json!([]));
    assert_eq!(c.error_code("pipeline.start", json!({"taskId": tid})).await, "conflict", "the task never ran");
    assert_eq!(
        c.error_code("pipeline.getConfig", json!({"projectId": "01a10c4a-a403-77bb-a4c2-7170ae990184"})).await,
        "not_found"
    );
    core.shutdown().await;
}
