//! Request-shape tests of the paid engines against a local mock server
//! ("request-shape verified", ADR-0046): path, query, auth header, content
//! type and body layout per vendor, plus response parsing.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Response};
use orkestr_protocol::methods::voice::{SttEngine, TtsEngine, VoiceEngineSettings};
use orkestr_voice::audio::Pcm;
use orkestr_voice::cloud::Endpoints;
use orkestr_voice::{VoiceError, VoiceService};
use parking_lot::Mutex;

#[derive(Debug, Clone)]
struct Seen {
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn mock() -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
        let log = Arc::clone(&log);
        async move {
            let path = uri.path().to_owned();
            log.lock().push(Seen { method, uri: uri.to_string(), headers, body: body.to_vec() });
            let json = |v: serde_json::Value| ([("content-type", "application/json")], v.to_string()).into_response();
            let audio = |mime: &'static str| ([("content-type", mime)], vec![1u8, 2, 3]).into_response();
            let r: Response = match path.as_str() {
                "/v1/audio/transcriptions" => json(serde_json::json!({"text": " привет "})),
                "/v1/audio/speech" => audio("audio/mpeg"),
                "/v1/listen" => json(serde_json::json!({"results": {"channels": [{"alternatives": [{"transcript": "привет deepgram"}]}]}})),
                "/speech/recognition/conversation/cognitiveservices/v1" => {
                    json(serde_json::json!({"RecognitionStatus": "Success", "DisplayText": "Привет, Azure."}))
                }
                "/cognitiveservices/v1" => audio("audio/wav"),
                "/v1/speech:recognize" => json(serde_json::json!({"results": [{"alternatives": [{"transcript": "привет"}]}, {"alternatives": [{"transcript": "google"}]}]})),
                "/v1/text:synthesize" => json(serde_json::json!({"audioContent": "UklGRg=="})),
                "/speech/v1/stt:recognize" => json(serde_json::json!({"result": "привет яндекс"})),
                "/speech/v1/tts:synthesize" => audio("audio/mpeg"),
                p if p.starts_with("/v1/text-to-speech/") => audio("audio/mpeg"),
                _ => (axum::http::StatusCode::UNAUTHORIZED, "nope").into_response(),
            };
            r
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn header(s: &Seen, name: &str) -> String {
    s.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

#[tokio::test]
async fn cloud_request_shapes() {
    let (base, seen) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = VoiceService::new(dir.path().into()).with_endpoints(Endpoints {
        openai: format!("{base}/v1"),
        deepgram: base.clone(),
        azure_stt: base.clone(),
        azure_tts: base.clone(),
        google_stt: base.clone(),
        google_tts: base.clone(),
        yandex_stt: base.clone(),
        yandex_tts: base.clone(),
        elevenlabs: base.clone(),
    });
    let s = VoiceEngineSettings {
        azure_region: "westeurope".into(),
        yandex_folder_id: "b1gfolder".into(),
        ..VoiceEngineSettings::default()
    };
    let pcm = Pcm { samples: vec![100, -100, 200, -200] };
    let take = || seen.lock().pop().unwrap();

    // Keys are required.
    let e = svc.transcribe(SttEngine::Deepgram, &s, None, &pcm, Some("ru")).await.unwrap_err();
    assert!(matches!(e, VoiceError::NeedsKey(_)), "{e:?}");

    // OpenAI STT: multipart with model/language and a WAV file part.
    let t = svc.transcribe(SttEngine::OpenAiWhisper, &s, Some("sk-o"), &pcm, Some("ru-RU")).await.unwrap();
    assert_eq!(t, "привет");
    let r = take();
    assert_eq!(r.method, Method::POST);
    assert_eq!(header(&r, "authorization"), "Bearer sk-o");
    assert!(header(&r, "content-type").starts_with("multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&r.body);
    assert!(body.contains("name=\"model\"\r\n\r\ngpt-4o-transcribe\r\n"), "{body}");
    assert!(body.contains("name=\"language\"\r\n\r\nru\r\n"));
    assert!(body.contains("filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF"));

    // OpenAI TTS: JSON body.
    let c = svc.synthesize(TtsEngine::OpenAi, &s, Some("sk-o"), Some("nova"), "ru-RU", 100, "Готово").await.unwrap();
    assert_eq!((c.bytes.as_slice(), c.mime_type.as_str()), (&[1u8, 2, 3][..], "audio/mpeg"));
    let r = take();
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["model"], "gpt-4o-mini-tts");
    assert_eq!(v["voice"], "nova");
    assert_eq!(v["input"], "Готово");

    // Deepgram: Token auth, raw WAV, model/language query.
    let t = svc.transcribe(SttEngine::Deepgram, &s, Some("dg"), &pcm, Some("ru")).await.unwrap();
    assert_eq!(t, "привет deepgram");
    let r = take();
    assert_eq!(header(&r, "authorization"), "Token dg");
    assert_eq!(header(&r, "content-type"), "audio/wav");
    assert!(r.uri.contains("model=nova-2") && r.uri.contains("language=ru") && r.uri.contains("smart_format=true"));
    assert!(r.body.starts_with(b"RIFF"));

    // Azure STT: subscription key header, locale query, PCM WAV content type.
    let t = svc.transcribe(SttEngine::AzureSpeech, &s, Some("az"), &pcm, Some("ru")).await.unwrap();
    assert_eq!(t, "Привет, Azure.");
    let r = take();
    assert_eq!(header(&r, "ocp-apim-subscription-key"), "az");
    assert!(r.uri.contains("language=ru-RU") && r.uri.contains("format=simple"), "{}", r.uri);
    assert_eq!(header(&r, "content-type"), "audio/wav; codecs=audio/pcm; samplerate=16000");

    // Azure TTS: SSML with an escaped text and the chosen voice.
    svc.synthesize(TtsEngine::AzureSpeech, &s, Some("az"), None, "ru-RU", 100, "A & B").await.unwrap();
    let r = take();
    assert_eq!(header(&r, "content-type"), "application/ssml+xml");
    assert_eq!(header(&r, "x-microsoft-outputformat"), "riff-24khz-16bit-mono-pcm");
    let ssml = String::from_utf8_lossy(&r.body);
    assert!(ssml.contains("<voice name='ru-RU-SvetlanaNeural'>A &amp; B</voice>"), "{ssml}");
    // …and needs a region.
    let no_region = VoiceEngineSettings::default();
    let e = svc.synthesize(TtsEngine::AzureSpeech, &no_region, Some("az"), None, "ru-RU", 100, "x").await.unwrap_err();
    assert!(matches!(e, VoiceError::NeedsKey(_)));

    // Google STT: key in a header (never the URL), LINEAR16 config, base64 PCM.
    let t = svc.transcribe(SttEngine::GoogleCloud, &s, Some("gk"), &pcm, Some("ru")).await.unwrap();
    assert_eq!(t, "привет google");
    let r = take();
    assert_eq!(header(&r, "x-goog-api-key"), "gk");
    assert!(!r.uri.contains("gk"));
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["config"]["encoding"], "LINEAR16");
    assert_eq!(v["config"]["sampleRateHertz"], 16000);
    assert_eq!(v["config"]["languageCode"], "ru-RU");
    assert_eq!(v["audio"]["content"], "ZACc/8gAOP8=");

    // Google TTS: base64 audioContent → WAV.
    let c = svc.synthesize(TtsEngine::GoogleCloud, &s, Some("gk"), None, "ru-RU", 100, "Да").await.unwrap();
    assert_eq!((c.bytes.as_slice(), c.mime_type.as_str()), (&b"RIFF"[..], "audio/wav"));
    let v: serde_json::Value = serde_json::from_slice(&take().body).unwrap();
    assert_eq!(v["audioConfig"]["audioEncoding"], "LINEAR16");
    assert_eq!(v["voice"]["languageCode"], "ru-RU");

    // Yandex STT: Api-Key auth, lpcm query with folder id, raw PCM body.
    let t = svc.transcribe(SttEngine::YandexSpeechKit, &s, Some("yk"), &pcm, Some("ru")).await.unwrap();
    assert_eq!(t, "привет яндекс");
    let r = take();
    assert_eq!(header(&r, "authorization"), "Api-Key yk");
    assert!(
        r.uri.contains("format=lpcm")
            && r.uri.contains("sampleRateHertz=16000")
            && r.uri.contains("folderId=b1gfolder")
    );
    assert_eq!(r.body, pcm.to_le_bytes());

    // Yandex TTS: form body.
    svc.synthesize(TtsEngine::YandexSpeechKit, &s, Some("yk"), None, "ru-RU", 100, "Привет").await.unwrap();
    let r = take();
    assert_eq!(header(&r, "content-type"), "application/x-www-form-urlencoded");
    let form = String::from_utf8_lossy(&r.body);
    assert!(form.contains("voice=alena") && form.contains("format=mp3") && form.contains("lang=ru-RU"), "{form}");

    // ElevenLabs: xi-api-key, voice in the path, model in the body.
    svc.synthesize(TtsEngine::ElevenLabs, &s, Some("el"), None, "ru-RU", 100, "Привет").await.unwrap();
    let r = take();
    assert_eq!(header(&r, "xi-api-key"), "el");
    assert!(r.uri.starts_with("/v1/text-to-speech/21m00Tcm4TlvDq8ikWAM?output_format=mp3_44100_128"), "{}", r.uri);
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["model_id"], "eleven_multilingual_v2");
}

#[tokio::test]
async fn failed_download_leaves_nothing_behind() {
    let (base, _) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    let models = orkestr_voice::models::ModelManager::new(dir.path().join("models"), reqwest::Client::new())
        .with_url_override(Arc::new(move |_| format!("{base}/v1/audio/speech")));
    // Pretend the runtime is installed so only the voice files are fetched.
    let rt = dir.path().join("models/piper-runtime");
    std::fs::create_dir_all(&rt).unwrap();
    std::fs::write(rt.join(orkestr_voice::models::MANIFEST), "{\"files\":{}}").unwrap();
    let err = models.install_blocking("piper-ru-irina").await.unwrap_err();
    assert!(err.to_string().contains("checksum mismatch"), "{err}");
    assert!(!models.is_installed("piper-ru-irina"));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("models/.tmp")).unwrap().flatten().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let list = models.list();
    let p = list.packages.iter().find(|p| p.id == "piper-ru-irina").unwrap();
    assert_eq!(p.state, orkestr_protocol::methods::voice::VoicePackageState::Failed);
    assert!(models.remove("piper-runtime").is_ok());
    assert!(models.download("nope").is_err());
}
