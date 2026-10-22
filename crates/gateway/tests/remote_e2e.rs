//! End-to-end Phase 4 remote access on loopback: gateway + core + a Rust
//! device client (passkey sign-in, enrollment, pairing with codeMac/SAS,
//! `e2e-v1` RPC through the opaque relay, policy, step-up, rate limit,
//! revocation and kill switch).

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::remote::{e2e, pairing};
use orkestr_gateway::{relay::Relay, spawn, GatewayConfig};
use orkestr_security::webauthn::{testing::SoftAuthenticator, RelyingParty};
use p256::ecdsa::SigningKey;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ---------------------------------------------------------------- local core RPC

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
                tokio::time::timeout(Duration::from_secs(15), self.ws.next()).await.expect("timeout").unwrap().unwrap();
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

// ---------------------------------------------------------------- device client

struct Device {
    session: e2e::Session,
    ws: Ws,
    n: u32,
}

async fn device_connect(
    gw: &str,
    token: &str,
    core_id: &str,
    device_id: &str,
    dk: &SigningKey,
    core_pub: &[u8],
) -> Result<Device, String> {
    let url = format!("ws://{gw}/v1/connect?coreId={core_id}&token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.map_err(|e| e.to_string())?;
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    let (hs, hello) = e2e::DeviceHandshake::start(core_id, device_id, now);
    ws.send(Message::Text(String::from_utf8(hello.to_bytes()).unwrap().into())).await.unwrap();
    let server_hello = recv_bin(&mut ws).await.ok_or("closed before serverHello")?;
    let frame = e2e::HandshakeFrame::from_bytes(&server_hello).map_err(|e| e.to_string())?;
    if let e2e::HandshakeFrame::Error { code } = &frame {
        return Err(code.clone());
    }
    let core_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(core_pub).unwrap();
    let (finish, session) = hs.finish(&frame, dk, &core_key).map_err(|e| e.to_string())?;
    ws.send(Message::Binary(finish.to_bytes().into())).await.unwrap();
    Ok(Device { session, ws, n: 0 })
}

async fn recv_bin(ws: &mut Ws) -> Option<Vec<u8>> {
    loop {
        match tokio::time::timeout(Duration::from_secs(15), ws.next()).await.ok()?? {
            Ok(Message::Binary(b)) => return Some(b.to_vec()),
            Ok(Message::Text(t)) => return Some(t.as_str().as_bytes().to_vec()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

impl Device {
    async fn send(&mut self, v: &Value) {
        let rec = self.session.sealer.seal(v.to_string().as_bytes()).unwrap();
        self.ws.send(Message::Binary(rec.into())).await.unwrap();
    }

    async fn next_res(&mut self) -> Option<Value> {
        loop {
            let rec = recv_bin(&mut self.ws).await?;
            let pt = self.session.opener.open(&rec).ok()?;
            let v: Value = serde_json::from_slice(&pt).unwrap();
            if v["type"] == "res" {
                return Some(v);
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.n += 1;
        let id = format!("d{}", self.n);
        self.send(&json!({"type":"req","id":id,"method":method,"params":params})).await;
        let res = self.next_res().await.expect("response");
        assert_eq!(res["id"], id.as_str());
        res
    }
}

// ---------------------------------------------------------------- the scenario

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn remote_access_end_to_end() {
    let gw_dir = tempfile::tempdir().unwrap();
    let cfg = GatewayConfig::from_args([
        "--data-dir".to_owned(),
        gw_dir.path().display().to_string(),
        "--bind".into(),
        "127.0.0.1:0".into(),
    ])
    .unwrap();
    let gw = spawn(cfg, Relay::with_capture()).await.unwrap();
    let gw_addr = gw.addr.to_string();
    let base = format!("http://{gw_addr}");
    let http = reqwest::Client::new();

    // 1. Passkey registration (first account) → session.
    let state: Value = http.get(format!("{base}/v1/auth/state")).send().await.unwrap().json().await.unwrap();
    assert_eq!(state["hasAccount"], false);
    let rp = RelyingParty {
        rp_id: state["rpId"].as_str().unwrap().into(),
        origins: vec![state["origins"][0].as_str().unwrap().into()],
    };
    let mut passkey = SoftAuthenticator::new(rp.clone());
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
    let fin = http
        .post(format!("{base}/v1/auth/register/finish"))
        .json(&json!({"ceremonyId": begin["ceremonyId"], "credential": passkey.register(&challenge)}))
        .send()
        .await
        .unwrap();
    assert!(fin.headers().get("set-cookie").is_some());
    let _: Value = fin.json().await.unwrap();
    // passkey login
    let begin: Value = http.post(format!("{base}/v1/auth/login/begin")).send().await.unwrap().json().await.unwrap();
    let challenge = orkestr_security::webauthn::unb64(begin["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let login: Value = http
        .post(format!("{base}/v1/auth/login/finish"))
        .json(&json!({"ceremonyId": begin["ceremonyId"], "credential": passkey.assert(&challenge)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_owned();
    // wrong TOTP is rejected
    let bad = http.post(format!("{base}/v1/auth/totp/login")).json(&json!({"code":"000000"})).send().await.unwrap();
    assert_eq!(bad.status(), 401);

    // 2. Core enrollment.
    let enroll: Value = http
        .post(format!("{base}/v1/cores/enrollment-token"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let core_dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(orkestr_core::CoreConfig::with_data_dir(core_dir.path())).await.unwrap();
    let mut local = Local::connect(&core).await;
    let status = local.ok("remote.enable", json!({"gatewayUrl": base, "enrollmentToken": enroll["token"]})).await;
    assert_eq!(status["connected"], true, "{status}");
    let core_id = status["coreId"].as_str().unwrap().to_owned();

    // 3. Pairing (codeMac + SAS).
    let offer = local.ok("devices.startPairing", json!({})).await;
    let pairing_id = offer["pairingId"].as_str().unwrap();
    let dk = SigningKey::random(&mut rand_core06::OsRng);
    let dk_pub = e2e::public_raw(&p256::PublicKey::from(dk.verifying_key()));
    let mac = pairing::code_mac(offer["code"].as_str().unwrap(), &dk_pub, pairing_id);
    let claim = http
        .post(format!("{base}/v1/pairings/{pairing_id}/claim"))
        .bearer_auth(&token)
        .json(&json!({"coreId": core_id, "devicePubKey": e2e::b64(&dk_pub), "deviceName": "test phone", "platform": "test", "codeMac": e2e::b64(&mac)}))
        .send()
        .await
        .unwrap();
    assert_eq!(claim.status(), 200);
    let claim: Value = claim.json().await.unwrap();
    let device_id = claim["deviceId"].as_str().unwrap().to_owned();
    let core_pub = e2e::unb64(claim["corePublicKey"].as_str().unwrap()).unwrap();
    assert_eq!(pairing::fingerprint(&core_pub), offer["coreKeyFingerprint"].as_str().unwrap(), "pinned core key");
    let device_sas = pairing::sas(&core_pub, &dk_pub, pairing_id);
    let list = local.ok("devices.list", json!({})).await;
    assert_eq!(list["pending"][0]["sas"], device_sas.as_str());
    local.ok("devices.approve", json!({"pairingId": pairing_id, "sasConfirmed": true, "role": "operator"})).await;
    let pst: Value = http
        .get(format!("{base}/v1/pairings/{pairing_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pst["status"], "approved");
    let msg = pairing::completion_message(pairing_id, &device_id, &dk_pub, orkestr_protocol::policy::Role::Operator);
    e2e::verify(
        &p256::ecdsa::VerifyingKey::from_sec1_bytes(&core_pub).unwrap(),
        &msg,
        pst["result"]["sig"].as_str().unwrap(),
    )
    .unwrap();

    // 4. E2E RPC through the relay.
    let mut dev = device_connect(&gw_addr, &token, &core_id, &device_id, &dk, &core_pub).await.unwrap();
    dev.send(&json!({"type":"auth","id":"a","token":"ignored-on-remote"})).await;
    assert_eq!(dev.next_res().await.unwrap()["ok"], true);
    let res = dev.call("projects.list", json!({})).await;
    assert_eq!(res["ok"], true, "{res}");
    // the relay saw only ciphertext + handshake JSON
    {
        let cap = gw.state.relay.capture.as_ref().unwrap().lock();
        assert!(!cap.is_empty());
        for b in cap.iter() {
            let s = String::from_utf8_lossy(b);
            assert!(!s.contains("projects.list") && !s.contains("ignored-on-remote"), "plaintext leaked: {s}");
        }
    }

    // 4b. Web Push: the core learned the gateway VAPID key from the tunnel
    // `welcome`; the device subscribes over E2E.
    let vapid: Value = http.get(format!("{base}/v1/push/vapid")).send().await.unwrap().json().await.unwrap();
    assert!(vapid["publicKey"].as_str().is_some_and(|k| !k.is_empty()), "{vapid}");
    let key = local.ok("push.vapidKey", json!({})).await;
    assert_eq!(key["publicKey"], vapid["publicKey"]);
    let ua = p256::SecretKey::random(&mut rand_core06::OsRng);
    let endpoint = "http://127.0.0.1:9/push/revoked-device";
    let r = dev
        .call(
            "push.subscribe",
            json!({"endpoint": endpoint, "p256dh": e2e::b64(&e2e::public_raw(&ua.public_key())), "auth": e2e::b64(&[7u8; 16])}),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    let subs_file = core_dir.path().join("push").join("subscriptions.json");
    assert!(std::fs::read_to_string(&subs_file).unwrap().contains(endpoint));

    // 5. Policy: local-only and admin-only are forbidden for an operator.
    for m in ["remote.enable", "devices.startPairing", "secrets.list"] {
        let r = dev.call(m, json!({})).await;
        assert_eq!(r["error"]["code"], "forbidden", "{m}: {r}");
    }
    let r = dev.call("devices.revoke", json!({"deviceId": device_id})).await;
    assert_eq!(r["error"]["code"], "forbidden");
    let r = dev.call("terminal.open", json!({})).await;
    assert_eq!(r["error"]["code"], "forbidden", "terminal needs remoteTerminal: {r}");

    // 6. Step-up: register a core-held credential, then a dangerous call.
    let mut stepup = SoftAuthenticator::new(rp.clone());
    let r = dev.call("devices.registerStepUp", json!({"credential": stepup.register(b"in-e2e")})).await;
    assert_eq!(r["ok"], true, "{r}");
    let params = json!({"projectId": "00000000-0000-0000-0000-000000000000", "paths": ["x"]});
    let r = dev.call("vcs.discard", params.clone()).await;
    assert_eq!(r["error"]["data"]["stepUpRequired"], true, "{r}");
    let challenge = e2e::unb64(r["error"]["data"]["challenge"].as_str().unwrap()).unwrap();
    let assertion = stepup.assert(&challenge);
    let mut with = params.clone();
    with["stepUp"] = serde_json::to_value(&assertion).unwrap();
    let r = dev.call("vcs.discard", with.clone()).await;
    assert_ne!(r["error"]["code"], "forbidden", "step-up accepted, handler ran: {r}");
    let r = dev.call("vcs.discard", with).await;
    assert_eq!(r["error"]["code"], "forbidden", "replayed assertion rejected: {r}");

    // 7. Rate limit (Read: 50 burst).
    let mut limited = false;
    for _ in 0..60 {
        let r = dev.call("remote.status", json!({})).await;
        if r["error"]["code"] == "rate_limited" {
            assert!(r["error"]["data"]["retryAfterMs"].as_u64().unwrap() > 0);
            limited = true;
            break;
        }
    }
    assert!(limited, "burst must be rate limited");

    // 8. Revoke ⇒ stream closed, handshake refused.
    local.ok("devices.revoke", json!({"deviceId": device_id})).await;
    assert!(dev.next_res().await.is_none(), "revoked session must be closed");
    let err = device_connect(&gw_addr, &token, &core_id, &device_id, &dk, &core_pub).await.err();
    assert_eq!(err.as_deref(), Some("revoked"));
    // ... and its push subscriptions are dropped.
    let mut dropped = false;
    for _ in 0..40 {
        if !std::fs::read_to_string(&subs_file).unwrap_or_default().contains(endpoint) {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dropped, "revoked device keeps push subscriptions");

    // 9. Kill switch: tunnel dropped, no redial.
    let st = local.ok("remote.disable", json!({})).await;
    assert_eq!(st["enabled"], false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!gw.state.relay.is_online(&core_id));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!gw.state.relay.is_online(&core_id), "no redial after remote.disable");

    // 10. Audit on both sides.
    let audit: Value =
        http.get(format!("{base}/v1/audit")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let actions: Vec<&str> = audit["entries"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    for a in [
        "auth.register",
        "auth.login",
        "cores.enrolled",
        "tunnel.connected",
        "pairing.claim",
        "devices.authorized",
        "devices.revoked",
    ] {
        assert!(actions.contains(&a), "gateway audit misses {a}: {actions:?}");
    }
    let core_audit = local.ok("audit.list", json!({})).await;
    let text = core_audit.to_string();
    for a in ["remote.handshake", "remote.call", "devices.approve", "devices.revoke", "remote.disable"] {
        assert!(text.contains(a), "core audit misses {a}");
    }
    core.shutdown().await;
}
