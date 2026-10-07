//! `voice.*` over a real WebSocket: defaults, validation, server-owned consent,
//! cloud STT/TTS through an OpenAI provider pointed at a local mock, and the
//! `VoiceCommand` audit event (§33, ADR-0017).

use std::time::Duration;

use axum::body::Bytes;
use axum::http::Uri;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use orkestr_core::CoreConfig;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    next: u64,
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

impl Client {
    async fn connect(core: &orkestr_core::CoreHandle) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(core.url.as_str()).await.unwrap();
        ws.send(Message::Text(json!({"type": "auth", "token": core.token}).to_string().into())).await.unwrap();
        assert_eq!(recv(&mut ws).await["ok"], true);
        Self { ws, next: 0 }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = format!("r{}", self.next);
        let frame = json!({"type": "req", "id": id, "method": method, "params": params});
        self.ws.send(Message::Text(frame.to_string().into())).await.unwrap();
        loop {
            let v = recv(&mut self.ws).await;
            match v["type"].as_str() {
                Some("res") if v["id"] == id => return v,
                _ => {}
            }
        }
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

async fn mock_openai() -> String {
    async fn handler(uri: Uri, body: Bytes) -> axum::response::Response {
        match uri.path() {
            "/v1/audio/transcriptions" => {
                assert!(String::from_utf8_lossy(&body).contains("RIFF"), "the WAV bytes are forwarded");
                ([("content-type", "application/json")], json!({"text": "что происходит со Schoolify"}).to_string())
                    .into_response()
            }
            "/v1/audio/speech" => ([("content-type", "audio/mpeg")], vec![1_u8, 2, 3]).into_response(),
            _ => (axum::http::StatusCode::NOT_FOUND, "").into_response(),
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().fallback(handler)).await.unwrap();
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn voice_config_consent_cloud_speech_and_command_log() {
    let dir = tempfile::tempdir().unwrap();
    // In-memory secrets: `providers.create` below stores API keys, which must
    // never land in the developer's real OS keyring.
    let config = CoreConfig::for_tests(dir.path().join("data"));
    assert_eq!(config.secret_backend, orkestr_security::SecretBackendChoice::Memory);
    let core = orkestr_core::start(config).await.unwrap();
    let mut c = Client::connect(&core).await;

    // Defaults: private.
    let r = c.ok("voice.getConfig", json!({})).await;
    let mut config = r["config"].clone();
    assert_eq!(config["sttEngine"], "Disabled");
    assert_eq!(config["wake"]["enabled"], false);
    assert_eq!(config["sttConsent"], Value::Null);

    // Validation.
    let mut bad = config.clone();
    bad["ratePct"] = json!(5);
    assert_eq!(c.err_code("voice.setConfig", json!({"config": bad})).await, "bad_request");

    // Off-device STT without consent is forbidden; forged consent is ignored.
    let mut cloud = config.clone();
    cloud["sttEngine"] = json!("OpenAiWhisper");
    assert_eq!(c.err_code("voice.setConfig", json!({"config": cloud.clone()})).await, "forbidden");
    cloud["sttConsent"] = json!({"engine": "OpenAiWhisper", "grantedAt": "2026-01-01T00:00:00Z"});
    assert_eq!(c.err_code("voice.setConfig", json!({"config": cloud})).await, "forbidden");
    assert_eq!(
        c.err_code("voice.transcribe", json!({"audioBase64": "UklGRg==", "mimeType": "audio/wav"})).await,
        "forbidden"
    );

    // Local settings round-trip.
    config["wake"]["enabled"] = json!(true);
    config["wake"]["phrase"] = json!("Оркестр");
    config["notifications"]["quietHours"] = json!({"enabled": true, "start": "23:00", "end": "07:30"});
    let saved = c.ok("voice.setConfig", json!({"config": config})).await;
    assert_eq!(saved["config"]["wake"]["phrase"], "Оркестр");
    assert_eq!(c.ok("voice.getConfig", json!({})).await["config"]["notifications"]["quietHours"]["end"], "07:30");

    // Consent → engine switches; audited.
    let r = c
        .ok("voice.setConsent", json!({"scope": "SpeechRecognition", "engine": "OpenAiWhisper", "granted": true}))
        .await;
    assert_eq!(r["config"]["sttEngine"], "OpenAiWhisper");
    assert_eq!(r["config"]["sttConsent"]["engine"], "OpenAiWhisper");
    assert_eq!(r["config"]["wake"]["phrase"], "Оркестр", "consent keeps other settings");
    let audit = c.ok("audit.list", json!({"limit": 20})).await;
    assert!(audit.to_string().contains("voice.consentGranted"), "{audit}");

    // No OpenAI provider yet.
    assert_eq!(
        c.err_code("voice.transcribe", json!({"audioBase64": "UklGRg==", "mimeType": "audio/wav"})).await,
        "bad_request"
    );

    // Cloud STT through an OpenAI provider (local mock).
    let base = mock_openai().await;
    c.ok("providers.create", json!({"kind": "OpenAi", "name": "OpenAI", "baseUrl": base, "apiKey": "sk-test"})).await;
    let providers = c.ok("voice.getConfig", json!({})).await["cloudProviders"].clone();
    assert_eq!(providers.as_array().unwrap().len(), 1);
    let t = c
        .ok("voice.transcribe", json!({"audioBase64": "UklGRnh4eHg=", "mimeType": "audio/wav", "language": "ru"}))
        .await;
    assert_eq!(t["text"], "что происходит со Schoolify");
    assert_eq!(
        c.err_code("voice.transcribe", json!({"audioBase64": "@@@", "mimeType": "audio/wav"})).await,
        "bad_request"
    );

    // Cloud TTS needs its own consent.
    assert_eq!(c.err_code("voice.synthesize", json!({"text": "Готово"})).await, "forbidden");
    c.ok("voice.setConsent", json!({"scope": "CloudSpeech", "granted": true})).await;
    let s = c.ok("voice.synthesize", json!({"text": "Готово", "voice": "alloy"})).await;
    assert_eq!(s["audioBase64"], "AQID");
    assert_eq!(s["mimeType"], "audio/mpeg");

    // Audit r1: consent is bound to the provider that receives the data.
    let first = providers[0]["id"].clone();
    let second = c
        .ok("providers.create", json!({"kind": "OpenAiCompatible", "name": "Other", "baseUrl": base, "apiKey": "k"}))
        .await;
    let second_id = second["provider"]["provider"]["id"].clone();
    assert!(second_id.is_string(), "{second}");
    let mut cfg = c.ok("voice.getConfig", json!({})).await["config"].clone();
    cfg["cloudProviderId"] = second_id.clone();
    c.ok("voice.setConfig", json!({"config": cfg.clone()})).await;
    let audio = json!({"audioBase64": "UklGRnh4eHg=", "mimeType": "audio/wav"});
    assert_eq!(c.err_code("voice.transcribe", audio.clone()).await, "forbidden", "consent was for the first provider");
    assert_eq!(c.err_code("voice.synthesize", json!({"text": "x", "voice": "alloy"})).await, "forbidden");
    c.ok("voice.setConsent", json!({"scope": "SpeechRecognition", "engine": "OpenAiWhisper", "granted": true})).await;
    c.ok("voice.transcribe", audio.clone()).await;
    // A disabled provider never receives audio, and cannot be selected.
    c.ok("providers.update", json!({"providerId": second_id, "patch": {"enabled": false}})).await;
    assert_eq!(c.err_code("voice.transcribe", audio.clone()).await, "bad_request");
    cfg["cloudProviderId"] = first;
    c.ok("voice.setConfig", json!({"config": cfg.clone()})).await;
    cfg["cloudProviderId"] = second_id;
    assert_eq!(c.err_code("voice.setConfig", json!({"config": cfg})).await, "bad_request");

    // Revoking falls back to local engines.
    let r = c.ok("voice.setConsent", json!({"scope": "SpeechRecognition", "granted": false})).await;
    assert_eq!(r["config"]["sttEngine"], "Disabled");
    assert_eq!(r["config"]["sttConsent"], Value::Null);
    let r = c.ok("voice.setConsent", json!({"scope": "CloudSpeech", "granted": false})).await;
    assert_eq!(r["config"]["ttsEngine"], "System");

    // Command log: transcript + intent only.
    c.ok(
        "voice.recordCommand",
        json!({"transcript": "останови Schoolify", "intent": "stopProject", "outcome": "ok", "source": "WakeWord"}),
    )
    .await;
    assert_eq!(
        c.err_code(
            "voice.recordCommand",
            json!({"transcript": "x", "intent": "drop table;", "outcome": "ok", "source": "Typed"})
        )
        .await,
        "bad_request"
    );
    let replay = c.ok("events.replay", json!({"limit": 500})).await;
    let found = replay["events"].as_array().unwrap().iter().any(|e| {
        e["kind"] == "VoiceCommand" && e["payload"]["intent"] == "stopProject" && e["payload"]["source"] == "WakeWord"
    });
    assert!(found, "VoiceCommand event persisted");
}
