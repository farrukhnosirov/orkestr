//! `marketplace.*` over a real WebSocket: official template install shows up
//! in `templates.list`; a directory registry by an unknown publisher needs
//! `marketplace.trustKey`; local publish + install; uninstall.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use orkestr_marketplace::index::{IndexDocument, IndexEntry, IndexKey};
use orkestr_marketplace::service::write_index;
use orkestr_marketplace::signing::{package_message, sha256_hex, KeyPair};
use orkestr_protocol::methods::marketplace::PackageKind;
use orkestr_security::SecretBackendChoice;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    events: Vec<Value>,
    n: u32,
}

impl Client {
    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(20), self.ws.next())
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

    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        let mut c = Self { ws, events: Vec::new(), n: 0 };
        c.send(json!({"type": "auth", "token": core.token})).await;
        assert_eq!(c.recv().await["ok"], true);
        c
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.n += 1;
        let id = format!("r{}", self.n);
        self.send(json!({"type": "req", "id": id, "method": method, "params": params})).await;
        loop {
            let v = self.recv().await;
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

    async fn err(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], false, "{method} should fail: {res}");
        res["error"].clone()
    }
}

fn template(name: &str) -> Vec<u8> {
    let mut v: Value = serde_json::from_str(include_str!("../../agent-runtime/templates/reviewer.json")).unwrap();
    v["origin"] = Value::Null;
    v["template"]["name"] = json!(name);
    serde_json::to_vec_pretty(&v).unwrap()
}

#[tokio::test]
async fn marketplace_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::with_data_dir(dir.path());
    cfg.secret_backend = SecretBackendChoice::VaultFileKey;
    let core = orkestr_core::start(cfg).await.unwrap();
    let mut c = Client::connect(&core).await;
    let mut sub = core.services.events.subscribe_all();

    // Official catalogue: verified with the embedded key, installs without a prompt.
    let sources = c.ok("marketplace.sources", json!({})).await;
    assert_eq!(sources["sources"][0]["id"], "official");
    let found = c.ok("marketplace.search", json!({"kind": "AgentTemplate", "query": "strict reviewer"})).await;
    assert_eq!(found["entries"][0]["id"], "orkestr.strict-code-reviewer", "{found}");
    let res = c.ok("marketplace.install", json!({"id": "orkestr.strict-code-reviewer"})).await;
    let tid = res["templateIds"][0].as_str().unwrap().to_owned();
    let list = c.ok("templates.list", json!({})).await;
    let t = list["templates"].as_array().unwrap().iter().find(|t| t["id"] == tid.as_str()).expect("imported");
    assert_eq!(t["name"], "Strict Code Reviewer");
    let installed = c.ok("marketplace.installed", json!({})).await;
    assert_eq!(installed["packages"][0]["version"], "1.0.0");

    // Directory registry of an unknown publisher: trust on first use.
    let reg_dir = dir.path().join("third-party");
    std::fs::create_dir_all(reg_dir.join("packages")).unwrap();
    let key = KeyPair::generate();
    let bytes = template("Third Party Reviewer");
    let sha256 = sha256_hex(&bytes);
    std::fs::write(reg_dir.join("packages/p.json"), &bytes).unwrap();
    let mut doc = IndexDocument::empty("third");
    doc.keys.push(IndexKey { key_id: key.key_id(), publisher: "third".into(), public_key: key.public_key() });
    doc.entries.push(IndexEntry {
        id: "third.reviewer".into(),
        kind: PackageKind::AgentTemplate,
        name: "Third Party Reviewer".into(),
        version: "1.0.0".into(),
        publisher: "third".into(),
        description: String::new(),
        tags: vec![],
        permissions: vec![],
        key_id: key.key_id(),
        signature: key.sign(&package_message(&sha256).unwrap()),
        sha256,
        url: "packages/p.json".into(),
        revoked: false,
    });
    write_index(&reg_dir, &doc, &key).await.unwrap();
    let src = c.ok("marketplace.addSource", json!({"kind": "Directory", "location": reg_dir.to_string_lossy()})).await;
    assert!(src["source"]["lastError"].is_null(), "{src}");
    let e = c.err("marketplace.install", json!({"id": "third.reviewer"})).await;
    assert_eq!(e["code"], "forbidden", "{e}");
    let details = c.ok("marketplace.get", json!({"id": "third.reviewer"})).await;
    assert_eq!(details["key"]["trust"], "Untrusted");
    let fp = details["key"]["fingerprint"].as_str().unwrap().to_owned();
    c.ok("marketplace.trustKey", json!({"keyId": key.key_id(), "fingerprint": fp})).await;
    c.ok("marketplace.install", json!({"id": "third.reviewer"})).await;

    // Local publishing into a directory source with the vault-held key.
    let mine = dir.path().join("mine");
    let src = c.ok("marketplace.addSource", json!({"kind": "Directory", "location": mine.to_string_lossy()})).await;
    let file = dir.path().join("mine.agent.json");
    std::fs::write(&file, template("My Reviewer")).unwrap();
    let published = c
        .ok(
            "marketplace.publish",
            json!({"path": file.to_string_lossy(), "kind": "AgentTemplate", "sourceId": src["source"]["id"], "publisher": "me"}),
        )
        .await;
    assert_eq!(published["entry"]["id"], "me.my-reviewer");
    let keys = c.ok("marketplace.keys", json!({})).await;
    assert_eq!(keys["localPublisher"]["trust"], "Trusted");
    c.ok("marketplace.install", json!({"id": "me.my-reviewer"})).await;
    c.ok("marketplace.uninstall", json!({"id": "me.my-reviewer"})).await;
    let installed = c.ok("marketplace.installed", json!({})).await;
    assert_eq!(installed["packages"].as_array().unwrap().len(), 2);

    // MarketplaceUpdated events were published.
    let mut seen = false;
    while let Ok(Some(r)) = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
        if let orkestr_events::Received::Event(ev) = r {
            seen |= matches!(ev.kind, orkestr_protocol::EventKind::MarketplaceUpdated(_));
        }
    }
    assert!(seen, "MarketplaceUpdated published");
}

/// Trust is shared with the plugin host: a widget plugin signed by a
/// third-party marketplace key installs (without dev mode) only after
/// `marketplace.trustKey`, its widget is served by `widgets.bundle` with the
/// widget CSP, and `marketplace.untrustKey` removes the plugin-host trust.
#[tokio::test]
async fn marketplace_plugin_trust_reaches_the_plugin_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::with_data_dir(dir.path());
    cfg.secret_backend = SecretBackendChoice::VaultFileKey;
    let core = orkestr_core::start(cfg).await.unwrap();
    let mut c = Client::connect(&core).await;

    let key = KeyPair::generate();
    let plugin_key = orkestr_plugins::signing::PublisherKeyPair::from_seed_b64(&key.seed_b64()).unwrap();
    assert_eq!(plugin_key.key_id(), key.key_id(), "one key id format for both trust stores");
    let fixture = orkestr_plugins::fixtures::all(&plugin_key)
        .unwrap()
        .into_iter()
        .find(|f| f.file == "hello-widget.orkp")
        .unwrap();
    let reg_dir = dir.path().join("widgets-registry");
    std::fs::create_dir_all(reg_dir.join("packages")).unwrap();
    std::fs::write(reg_dir.join("packages/w.orkp"), &fixture.bytes).unwrap();
    let sha256 = sha256_hex(&fixture.bytes);
    let mut doc = IndexDocument::empty("widgets");
    doc.keys.push(IndexKey { key_id: key.key_id(), publisher: "acme".into(), public_key: key.public_key() });
    doc.entries.push(IndexEntry {
        id: fixture.package_id.clone(),
        kind: PackageKind::Widget,
        name: "Hello Widget".into(),
        version: "0.1.0".into(),
        publisher: "acme".into(),
        description: String::new(),
        tags: vec![],
        permissions: vec![],
        key_id: key.key_id(),
        signature: key.sign(&package_message(&sha256).unwrap()),
        sha256,
        url: "packages/w.orkp".into(),
        revoked: false,
    });
    write_index(&reg_dir, &doc, &key).await.unwrap();
    c.ok("marketplace.addSource", json!({"kind": "Directory", "location": reg_dir.to_string_lossy()})).await;

    let e = c.err("marketplace.install", json!({"id": fixture.package_id})).await;
    assert_eq!(e["code"], "forbidden", "{e}");
    let fp = c.ok("marketplace.get", json!({"id": fixture.package_id})).await["key"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    c.ok("marketplace.trustKey", json!({"keyId": key.key_id(), "fingerprint": fp})).await;
    let res = c.ok("marketplace.install", json!({"id": fixture.package_id})).await;
    let plugin_id = res["plugin"]["plugin"]["id"].as_str().expect("plugin installed").to_owned();
    let trusted = core.services.plugins.trust_store().get(&key.key_id()).await.unwrap().expect("pinned");
    assert_eq!(trusted.source, "marketplace");

    c.ok("plugins.enable", json!({"pluginId": plugin_id})).await;
    let widget_id = format!("{}/hello-widget", fixture.package_id);
    let list = c.ok("widgets.list", json!({})).await;
    assert!(list["widgets"].as_array().unwrap().iter().any(|w| w["id"] == widget_id.as_str()), "{list}");
    let bundle = c.ok("widgets.bundle", json!({"widgetId": widget_id})).await;
    assert!(bundle["html"].as_str().unwrap().contains("<"), "{bundle}");
    assert_eq!(bundle["csp"], orkestr_core::rpc::widgets::WIDGET_CSP);

    c.ok("marketplace.untrustKey", json!({"keyId": key.key_id()})).await;
    assert!(core.services.plugins.trust_store().get(&key.key_id()).await.unwrap().is_none());
}
