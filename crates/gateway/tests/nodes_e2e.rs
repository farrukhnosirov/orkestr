//! Phase 6 (WP-A) end to end on loopback: one gateway, a coordinator core and
//! a worker core of the same account. Node enrollment via a `kind: "node"`
//! pairing (wrong code rejected, SAS equal on both sides, unconfirmed SAS not
//! approvable), core-to-core E2E link with a core bearer token (relay sees no
//! plaintext), `nodes.list` snapshots, pull heartbeats, drain, revocation by
//! the worker (refused at the next heartbeat) and by the coordinator.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_gateway::{relay::Relay, spawn, GatewayConfig};
use orkestr_security::webauthn::{testing::SoftAuthenticator, RelyingParty};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Local {
    ws: Ws,
    n: u32,
}

impl Local {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
        let mut me = Self { ws, n: 0 };
        let res = me.next_res().await;
        assert_eq!(res["ok"], true, "{res}");
        me
    }

    async fn next_res(&mut self) -> Value {
        loop {
            let m =
                tokio::time::timeout(Duration::from_secs(60), self.ws.next()).await.expect("timeout").unwrap().unwrap();
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] == "res" {
                    return v;
                }
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.n += 1;
        let id = format!("l{}", self.n);
        self.ws
            .send(Message::Text(json!({"type":"req","id":id,"method":method,"params":params}).to_string().into()))
            .await
            .unwrap();
        let res = self.next_res().await;
        assert_eq!(res["id"], id.as_str());
        res
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let res = self.call(method, params).await;
        assert_eq!(res["ok"], true, "{method}: {res}");
        res["result"].clone()
    }
}

async fn session_token(http: &reqwest::Client, base: &str) -> String {
    let state: Value = http.get(format!("{base}/v1/auth/state")).send().await.unwrap().json().await.unwrap();
    let rp = RelyingParty {
        rp_id: state["rpId"].as_str().unwrap().into(),
        origins: vec![state["origins"][0].as_str().unwrap().into()],
    };
    let mut passkey = SoftAuthenticator::new(rp);
    let begin: Value = http
        .post(format!("{base}/v1/auth/register/begin"))
        .json(&json!({"name":"owner"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let challenge = orkestr_security::webauthn::unb64(begin["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let fin: Value = http
        .post(format!("{base}/v1/auth/register/finish"))
        .json(&json!({"ceremonyId": begin["ceremonyId"], "credential": passkey.register(&challenge)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    fin["token"].as_str().expect("session token").to_owned()
}

async fn enrolled_core(http: &reqwest::Client, base: &str, token: &str) -> (orkestr_core::CoreHandle, Local, String, tempfile::TempDir) {
    let enroll: Value = http
        .post(format!("{base}/v1/cores/enrollment-token"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(orkestr_core::CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let mut local = Local::connect(&core).await;
    let status = local.ok("remote.enable", json!({"gatewayUrl": base, "enrollmentToken": enroll["token"]})).await;
    assert_eq!(status["connected"], true, "{status}");
    let core_id = status["coreId"].as_str().unwrap().to_owned();
    (core, local, core_id, dir)
}

async fn wait_status(c: &mut Local, node_id: &str, want: &str) -> Value {
    for _ in 0..100 {
        let list = c.ok("nodes.list", json!({})).await;
        if let Some(s) = list["snapshots"].as_array().unwrap().iter().find(|s| s["node"]["id"] == node_id) {
            if s["status"] == want {
                return s.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node {node_id} never became {want}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn node_enrollment_heartbeat_revoke_end_to_end() {
    // fast heartbeats for the whole process (both cores read it at start)
    std::env::set_var("ORKESTR_NODE_HEARTBEAT_MS", "300");
    let gw_dir = tempfile::tempdir().unwrap();
    let cfg = GatewayConfig::from_args([
        "--data-dir".to_owned(),
        gw_dir.path().display().to_string(),
        "--bind".into(),
        "127.0.0.1:0".into(),
    ])
    .unwrap();
    let gw = spawn(cfg, Relay::with_capture()).await.unwrap();
    let base = format!("http://{}", gw.addr);
    let http = reqwest::Client::new();
    let token = session_token(&http, &base).await;
    let (coord, mut c, _coord_core_id, _cd) = enrolled_core(&http, &base, &token).await;
    let (worker, mut w, worker_core_id, _wd) = enrolled_core(&http, &base, &token).await;

    // the worker offers a node pairing with trust `standard`
    let offer = w.ok("devices.startPairing", json!({"kind": "node", "trustLevel": "standard"})).await;
    let pairing_id = offer["pairingId"].as_str().unwrap().to_owned();
    let code = offer["code"].as_str().unwrap().to_owned();

    // wrong code → a real error, no enrollment
    let bad = c.call("nodes.enroll", json!({"coreId": worker_core_id, "pairingId": pairing_id, "code": "WRONGCOD"})).await;
    assert_eq!(bad["ok"], false, "{bad}");
    assert_eq!(bad["error"]["code"], "forbidden", "{bad}");

    // right code → pending approval with the SAS
    let params = json!({"coreId": worker_core_id, "pairingId": pairing_id, "code": code, "name": "gpu"});
    let pending = c.call("nodes.enroll", params.clone()).await;
    assert_eq!(pending["error"]["data"]["pendingApproval"], true, "{pending}");
    let coord_sas = pending["error"]["data"]["sas"].as_str().unwrap().to_owned();
    let list = w.ok("devices.list", json!({})).await;
    let p = &list["pending"][0];
    assert_eq!(p["sas"], coord_sas.as_str(), "SAS equal on both sides");
    assert_eq!(p["kind"], "node");
    // the SAS must be confirmed
    let r = w.call("devices.approve", json!({"pairingId": pairing_id, "sasConfirmed": false, "role": "operator"})).await;
    assert_eq!(r["ok"], false, "{r}");

    // enroll concurrently with the worker's approval
    let enroll = tokio::spawn(async move {
        let r = c.call("nodes.enroll", params).await;
        (c, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let dev = w
        .ok(
            "devices.approve",
            json!({"pairingId": pairing_id, "sasConfirmed": true, "role": "operator", "remoteTerminal": false,
                "kind": "node", "trustLevel": "standard"}),
        )
        .await;
    assert_eq!(dev["trustLevel"], "standard", "{dev}");
    let coordinator_device = dev["deviceId"].as_str().unwrap().to_owned();
    let (mut c, res) = enroll.await.unwrap();
    assert_eq!(res["ok"], true, "{res}");
    let enr = &res["result"]["enrollment"];
    assert_eq!(res["result"]["sas"], coord_sas.as_str());
    assert_eq!(enr["trustLevel"], "standard");
    assert_eq!(enr["coreId"], worker_core_id.as_str());
    assert!(enr["sig"].as_str().is_some_and(|s| !s.is_empty()));
    let node_id = enr["nodeId"].as_str().unwrap().to_owned();
    assert_eq!(node_id, worker.services.node_id.to_string());

    // registry: coordinator first, the worker with its resources
    let list = c.ok("nodes.list", json!({})).await;
    let snaps = list["snapshots"].as_array().unwrap();
    assert_eq!(snaps.len(), 2, "{list}");
    assert_eq!(snaps[0]["isCoordinator"], true);
    assert_eq!(snaps[0]["node"]["id"], coord.services.node_id.to_string());
    assert_eq!(snaps[1]["node"]["id"], node_id.as_str());
    assert_eq!(snaps[1]["node"]["name"], "gpu");
    assert_eq!(snaps[1]["trustLevel"], "standard");
    assert_eq!(snaps[1]["isCoordinator"], false);
    assert!(snaps[1]["node"]["resources"]["cpuCores"].as_u64().unwrap() >= 1);
    // NodeRegistered was published
    let q = orkestr_events::ReplayQuery { from_seq: 0, to_seq: None, limit: 10_000, topics: None, newest_first: false };
    let ev = coord
        .services
        .events
        .replay(&q)
        .await
        .unwrap()
        .events
        .into_iter()
        .any(|e| matches!(&e.kind, orkestr_protocol::event::EventKind::NodeRegistered(p) if p.node.node.id.to_string() == node_id));
    assert!(ev, "NodeRegistered event");

    // pull heartbeats advance lastHeartbeatAt
    let first = snaps[1]["lastHeartbeatAt"].as_str().unwrap().to_owned();
    let mut advanced = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let l = c.ok("nodes.list", json!({})).await;
        if l["snapshots"][1]["lastHeartbeatAt"].as_str().unwrap() != first {
            advanced = true;
            break;
        }
    }
    assert!(advanced, "heartbeat recorded");

    // the relay saw only ciphertext
    {
        let cap = gw.state.relay.capture.as_ref().unwrap().lock();
        assert!(!cap.is_empty());
        for b in cap.iter() {
            let s = String::from_utf8_lossy(b);
            assert!(!s.contains("node.heartbeat") && !s.contains("node.hello"), "plaintext leaked: {s}");
        }
    }
    let audit: Value =
        http.get(format!("{base}/v1/audit")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let actions: Vec<&str> = audit["entries"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    assert!(actions.contains(&"relay.coreConnect"), "{actions:?}");
    assert!(actions.contains(&"pairing.coreClaim"), "{actions:?}");

    // drain is sticky across heartbeats; undrain returns it
    let d = c.ok("nodes.drain", json!({"nodeId": node_id, "drain": true})).await;
    assert_eq!(d["node"]["status"], "Draining");
    tokio::time::sleep(Duration::from_millis(800)).await;
    wait_status(&mut c, &node_id, "Draining").await;
    c.ok("nodes.drain", json!({"nodeId": node_id, "drain": false})).await;
    wait_status(&mut c, &node_id, "Online").await;
    // policy: never above what the worker granted
    let r = c.call("nodes.setPolicy", json!({"nodeId": node_id, "trustLevel": "trusted"})).await;
    assert_eq!(r["error"]["code"], "bad_request", "{r}");
    let r = c.ok("nodes.setPolicy", json!({"nodeId": node_id, "trustLevel": "restricted", "maxConcurrentJobs": 1})).await;
    assert_eq!(r["node"]["trustLevel"], "restricted");
    assert_eq!(r["node"]["slots"]["max"], 1);

    // the worker revokes the coordinator → refused at the next heartbeat
    w.ok("devices.revoke", json!({"deviceId": coordinator_device})).await;
    let s = wait_status(&mut c, &node_id, "Revoked").await;
    assert_eq!(s["status"], "Revoked");
    let enrs = coord.services.node_hub.registry().all_stored().await.unwrap();
    assert!(enrs[0].enrollment.revoked_at.is_some());
    assert!(coord.services.node_hub.verify_enrollment(&enrs[0].enrollment), "re-signed on revoke");
    let link = coord.services.node_hub.link(node_id.parse().unwrap()).await;
    assert!(matches!(link, Err(orkestr_node::NodeError::Revoked(_))), "revoked node refused");
    // coordinator-side revoke is idempotent; drain of a revoked node fails
    c.ok("nodes.revoke", json!({"nodeId": node_id})).await;
    let r = c.call("nodes.drain", json!({"nodeId": node_id, "drain": true})).await;
    assert_eq!(r["ok"], false);

    coord.shutdown().await;
    worker.shutdown().await;
}
