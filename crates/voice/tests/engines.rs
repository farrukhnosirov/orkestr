//! End-to-end tests of the offline engines on a machine with installed
//! packages. They need `ORKESTR_VOICE_TEST_DIR=<ORKESTR_HOME>/voice` (with
//! `whisper-base`, `vosk-small-ru` and the Piper ru voices installed, e.g. by
//! `cargo run -p orkestr-voice --example voice_install`); without it they
//! print a note and pass.

use orkestr_protocol::methods::voice::{SttEngine, TtsEngine, VoiceConfig, VoiceEngineSettings};
use orkestr_voice::audio::{self, to_pcm16k, wav_rms};
use orkestr_voice::VoiceService;

const BUNDLED: &[u8] = include_bytes!("data/ru_command.wav");
const PHRASE: &str = "Оркестр, открой проект и запусти тесты.";
const KEYWORDS: &[&str] = &["откр", "проект", "запуст", "тест"];

fn service() -> Option<VoiceService> {
    match std::env::var_os("ORKESTR_VOICE_TEST_DIR") {
        Some(d) => Some(VoiceService::new(d.into())),
        None => {
            eprintln!("ORKESTR_VOICE_TEST_DIR is not set; skipping engine end-to-end test");
            None
        }
    }
}

fn has_keywords(text: &str, words: &[&str]) -> bool {
    let t = text.to_lowercase();
    words.iter().filter(|w| t.contains(*w)).count() >= 2
}

async fn stt(svc: &VoiceService, e: SttEngine, wav: &[u8]) -> String {
    let pcm = to_pcm16k(wav).unwrap();
    svc.transcribe(e, &VoiceEngineSettings::default(), None, &pcm, Some("ru")).await.unwrap()
}

#[tokio::test]
async fn bundled_russian_wav_is_recognized_by_whisper_and_vosk() {
    let Some(svc) = service() else { return };
    let w = stt(&svc, SttEngine::WhisperCpp, BUNDLED).await;
    assert!(has_keywords(&w, KEYWORDS), "whisper: {w}");
    let v = stt(&svc, SttEngine::Vosk, BUNDLED).await;
    assert!(has_keywords(&v, KEYWORDS), "vosk: {v}");
    if svc.models().is_installed("sherpa-zipformer-small-ru-int8") {
        let s = VoiceEngineSettings {
            sherpa_model: "sherpa-zipformer-small-ru-int8".into(),
            ..VoiceEngineSettings::default()
        };
        let pcm = to_pcm16k(BUNDLED).unwrap();
        let t = svc.transcribe(SttEngine::SherpaOnnx, &s, None, &pcm, Some("ru")).await.unwrap();
        assert!(has_keywords(&t, KEYWORDS), "sherpa: {t}");
    }
}

#[tokio::test]
async fn every_piper_voice_speaks_and_whisper_hears_it() {
    let Some(svc) = service() else { return };
    let s = VoiceEngineSettings::default();
    for voice in ["piper-ru-irina", "piper-ru-denis", "piper-ru-dmitri", "piper-ru-ruslan"] {
        let clip = svc.synthesize(TtsEngine::Piper, &s, None, Some(voice), "ru-RU", 100, PHRASE).await.unwrap();
        assert_eq!(clip.mime_type, "audio/wav");
        assert!(wav_rms(&clip.bytes).unwrap() > 0.01, "{voice}: silent");
        let heard = stt(&svc, SttEngine::WhisperCpp, &clip.bytes).await;
        assert!(has_keywords(&heard, KEYWORDS), "{voice} → whisper: {heard}");
    }
}

#[tokio::test]
async fn windows_voices_and_recognizer() {
    if !cfg!(windows) {
        return;
    }
    let Some(svc) = service() else { return };
    let win = svc.windows(true).await;
    let s = VoiceEngineSettings::default();
    if win.voices.iter().any(|v| v.language.starts_with("ru")) {
        let clip = svc.synthesize(TtsEngine::WindowsOneCore, &s, None, None, "ru-RU", 100, PHRASE).await.unwrap();
        assert!(wav_rms(&clip.bytes).unwrap() > 0.01, "OneCore voice is silent");
        let heard = stt(&svc, SttEngine::WhisperCpp, &clip.bytes).await;
        assert!(has_keywords(&heard, KEYWORDS), "OneCore → whisper: {heard}");
    }
    if win.recognizers.iter().any(|c| c == "en-US") && win.voices.iter().any(|v| v.language == "en-US") {
        let clip = svc
            .synthesize(TtsEngine::WindowsOneCore, &s, None, None, "en-US", 90, "Open the project and run the tests.")
            .await
            .unwrap();
        let pcm = to_pcm16k(&clip.bytes).unwrap();
        let heard = svc.transcribe(SttEngine::WindowsSpeech, &s, None, &pcm, Some("en-US")).await.unwrap();
        let t = heard.to_lowercase();
        assert!(t.contains("project") || t.contains("test"), "Windows recognizer: {heard}");
    }
    // Russian dictation needs a ru-RU recognizer; without it the error is explicit.
    if !win.recognizers.iter().any(|c| c.starts_with("ru")) {
        let pcm = to_pcm16k(BUNDLED).unwrap();
        let err = svc.transcribe(SttEngine::WindowsSpeech, &s, None, &pcm, Some("ru")).await.unwrap_err();
        assert!(err.to_string().contains("no Windows speech recognizer"), "{err}");
    }
}

#[tokio::test]
async fn engine_listing_reflects_installed_packages() {
    let Some(svc) = service() else { return };
    let r = svc.engines(&VoiceConfig::default(), &[], false).await;
    let whisper = r.stt.iter().find(|e| e.stt == Some(SttEngine::WhisperCpp)).unwrap();
    assert_eq!(whisper.status, orkestr_protocol::methods::voice::VoiceEngineStatus::Ready, "{whisper:?}");
    let piper = r.tts.iter().find(|e| e.tts == Some(TtsEngine::Piper)).unwrap();
    assert!(piper.options.iter().filter(|o| o.available).count() >= 4);
    let deepgram = r.stt.iter().find(|e| e.stt == Some(SttEngine::Deepgram)).unwrap();
    assert_eq!(deepgram.status, orkestr_protocol::methods::voice::VoiceEngineStatus::NeedsKey);
    for id in ["whisper-base", "vosk-small-ru", "piper-ru-irina"] {
        let v = svc.models().verify(id).await.unwrap();
        assert!(v.ok && v.files > 0, "{id}: {v:?}");
    }
    assert_eq!(audio::SAMPLE_RATE, 16_000);
}
