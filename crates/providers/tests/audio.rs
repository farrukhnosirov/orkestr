//! Speech APIs (§33): OpenAI-wire transcription and speech against a local
//! axum mock — request shape, auth, multipart body, error classification and
//! the `Unsupported` default of other adapters.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use orkestr_domain::{AuthMethod, Provider, ProviderId, ProviderKind};
use orkestr_providers::audio::{SpeechRequest, TranscriptionRequest};
use orkestr_providers::factory::build_adapter;
use orkestr_providers::{AdapterDeps, CredentialResolver, MemoryCredentials, ProviderAdapter, ProviderError};
use parking_lot::Mutex;
use serde_json::{json, Value};

/// A recorded request: path, headers, body.
type Seen = (String, HeaderMap, Vec<u8>);

#[derive(Clone, Default)]
struct Mock {
    seen: Arc<Mutex<Vec<Seen>>>,
    status: Arc<Mutex<u16>>,
}

async fn handler(State(m): State<Mock>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    m.seen.lock().push((uri.path().to_owned(), headers, body.to_vec()));
    let status = *m.status.lock();
    if status != 200 {
        return (StatusCode::from_u16(status).unwrap(), r#"{"error":{"message":"nope"}}"#).into_response();
    }
    match uri.path() {
        "/v1/audio/transcriptions" => {
            ([("content-type", "application/json")], json!({"text": " Jarvis, what is up? ", "language": "english"}).to_string())
                .into_response()
        }
        "/v1/audio/speech" => ([("content-type", "audio/mpeg")], vec![0xFF_u8, 0xFB, 0x90, 0x00]).into_response(),
        _ => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

async fn start() -> (Mock, String) {
    let m = Mock { status: Arc::new(Mutex::new(200)), ..Mock::default() };
    let app = Router::new().fallback(handler).with_state(m.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (m, format!("http://{addr}/v1"))
}

async fn adapter(kind: ProviderKind, base: &str, key: Option<&str>) -> Arc<dyn ProviderAdapter> {
    let creds = Arc::new(MemoryCredentials::new());
    let id = ProviderId::new();
    let secret = match key {
        Some(k) => Some(creds.store(id, "test", k).await.unwrap()),
        None => None,
    };
    let now = chrono::Utc::now();
    let p = Provider {
        id,
        kind,
        name: "speech".into(),
        enabled: true,
        base_url: Some(base.to_owned()),
        auth_method: AuthMethod::ApiKey,
        credential_ref_id: None,
        secret_ref_id: secret,
        is_local: false,
        default_model: None,
        settings: Value::Null,
        created_at: now,
        updated_at: now,
    };
    build_adapter(&p, &AdapterDeps::new(creds)).unwrap()
}

#[tokio::test]
async fn openai_transcription_sends_multipart_with_auth_and_language() {
    let (mock, base) = start().await;
    let a = adapter(ProviderKind::OpenAi, &base, Some("sk-test")).await;
    let out = a
        .transcribe(TranscriptionRequest {
            audio: b"RIFFxxxxWAVE".to_vec(),
            mime_type: "audio/wav".into(),
            language: Some("ru-RU".into()),
            model: None,
        })
        .await
        .unwrap();
    assert_eq!(out.text, "Jarvis, what is up?");
    assert_eq!(out.language.as_deref(), Some("english"));
    let (path, headers, body) = mock.seen.lock().last().cloned().unwrap();
    assert_eq!(path, "/v1/audio/transcriptions");
    assert_eq!(headers["authorization"], "Bearer sk-test");
    assert!(headers["content-type"].to_str().unwrap().starts_with("multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"));
    assert!(body.contains("name=\"language\"\r\n\r\nru\r\n"), "language hint is the primary tag");
    assert!(body.contains("filename=\"speech.wav\""));
    assert!(body.contains("RIFFxxxxWAVE"));
}

#[tokio::test]
async fn openai_speech_returns_audio_and_sends_style() {
    let (mock, base) = start().await;
    let a = adapter(ProviderKind::OpenAi, &base, Some("sk-test")).await;
    let out = a
        .synthesize_speech(SpeechRequest {
            text: "Build failed".into(),
            voice: "alloy".into(),
            style: Some("calm".into()),
            model: None,
        })
        .await
        .unwrap();
    assert_eq!(out.mime_type, "audio/mpeg");
    assert_eq!(out.bytes, vec![0xFF, 0xFB, 0x90, 0x00]);
    let (path, _, body) = mock.seen.lock().last().cloned().unwrap();
    assert_eq!(path, "/v1/audio/speech");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["input"], "Build failed");
    assert_eq!(v["voice"], "alloy");
    assert_eq!(v["instructions"], "calm");
    assert_eq!(v["response_format"], "mp3");
}

#[tokio::test]
async fn errors_are_classified_and_missing_keys_fail_before_any_request() {
    let (mock, base) = start().await;
    let no_key = adapter(ProviderKind::OpenAi, &base, None).await;
    let e = no_key
        .transcribe(TranscriptionRequest { audio: vec![1], mime_type: "audio/wav".into(), language: None, model: None })
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Auth(_)), "{e:?}");
    assert!(mock.seen.lock().is_empty(), "no audio is sent without a key");

    *mock.status.lock() = 401;
    let a = adapter(ProviderKind::OpenAi, &base, Some("sk-bad")).await;
    let e = a
        .synthesize_speech(SpeechRequest { text: "x".into(), voice: "alloy".into(), style: None, model: None })
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Auth(_)), "{e:?}");
}

#[tokio::test]
async fn other_adapters_do_not_support_speech() {
    let (_mock, base) = start().await;
    let a = adapter(ProviderKind::Anthropic, &base, Some("k")).await;
    let e = a
        .transcribe(TranscriptionRequest { audio: vec![1], mime_type: "audio/wav".into(), language: None, model: None })
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Unsupported(_)));
}
