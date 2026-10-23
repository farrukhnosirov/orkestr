//! # orkestr-voice
//!
//! Switchable speech engines for Orkestr (ADR-0046, §33).
//!
//! - **Engines** ([`VoiceService::transcribe`], [`VoiceService::synthesize`]):
//!   one entry point per direction, dispatching on the protocol's
//!   [`SttEngine`] / [`TtsEngine`]. Offline: whisper.cpp (CLI), Vosk
//!   (`libvosk` via `orkestr-platform::vosk`), sherpa-onnx (CLI), Windows
//!   `System.Speech`; Piper (CLI), Windows OneCore voices (WinRT via
//!   PowerShell), Silero (optional Python + PyTorch). Cloud: OpenAI,
//!   Deepgram, Azure, Google, Yandex SpeechKit, ElevenLabs ([`cloud`]).
//! - **Capabilities and readiness** ([`VoiceService::engines`]).
//! - **Model manager** ([`models::ModelManager`]): pinned, checksum-verified
//!   downloads under `<ORKESTR_HOME>/voice/models`.
//!
//! Consent, routing (per-language defaults, fallback chains) and API key
//! storage are the core's job; this crate only runs engines. Audio is never
//! persisted: temp files under `voice/models/.tmp` are deleted right after
//! each call.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audio;
pub mod catalog;
pub mod cloud;
pub mod local;
pub mod models;
pub mod proc;

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_protocol::methods::voice::{
    SttEngine, TtsEngine, VoiceConfig, VoiceEngineInfo, VoiceEngineOption, VoiceEngineSettings, VoiceEngineStatus,
    VoiceEngineVerification, VoiceEnginesResult, VoiceGrade, VoiceKeyId,
};
use parking_lot::Mutex;

use crate::audio::Pcm;

/// Engine errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VoiceError {
    /// A package must be downloaded first.
    #[error("voice package `{0}` is not installed (Settings → Voice → Models)")]
    NotInstalled(String),
    /// A key / region is missing or was rejected.
    #[error("{0}")]
    NeedsKey(String),
    /// Not available on this machine.
    #[error("{0}")]
    Unavailable(String),
    /// Invalid input.
    #[error("{0}")]
    BadRequest(String),
    /// A network / service error.
    #[error("{0}")]
    Network(String),
    /// The engine failed.
    #[error("{0}")]
    Failed(String),
}

/// Encoded audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// Bytes (WAV / MP3).
    pub bytes: Vec<u8>,
    /// MIME type.
    pub mime_type: String,
}

/// Engine voices / models shown in settings for the cloud engines.
const AZURE_VOICES: &[(&str, &str)] = &[
    ("ru-RU-SvetlanaNeural", "ru-RU"),
    ("ru-RU-DmitryNeural", "ru-RU"),
    ("ru-RU-DariyaNeural", "ru-RU"),
    ("en-US-JennyNeural", "en-US"),
    ("en-US-GuyNeural", "en-US"),
];
const YANDEX_VOICES: &[(&str, &str)] = &[
    ("alena", "ru-RU"),
    ("filipp", "ru-RU"),
    ("ermil", "ru-RU"),
    ("jane", "ru-RU"),
    ("omazh", "ru-RU"),
    ("zahar", "ru-RU"),
];
const OPENAI_VOICES: &[&str] = &["alloy", "ash", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"];

/// The speech engine service.
pub struct VoiceService {
    models: models::ModelManager,
    cloud: cloud::Cloud,
    vosk: Arc<local::VoskCache>,
    windows: Mutex<Option<local::WindowsSpeech>>,
}

impl std::fmt::Debug for VoiceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceService").field("models", &self.models).finish_non_exhaustive()
    }
}

impl VoiceService {
    /// A service storing packages under `voice_dir/models` (`voice_dir` = `<ORKESTR_HOME>/voice`).
    #[must_use]
    pub fn new(voice_dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("orkestr-voice/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self {
            models: models::ModelManager::new(voice_dir.join("models"), http.clone()),
            cloud: cloud::Cloud { http, endpoints: cloud::Endpoints::default() },
            vosk: Arc::default(),
            windows: Mutex::new(None),
        }
    }

    /// Overrides cloud endpoints (tests).
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: cloud::Endpoints) -> Self {
        self.cloud.endpoints = endpoints;
        self
    }

    /// The model manager.
    #[must_use]
    pub fn models(&self) -> &models::ModelManager {
        &self.models
    }

    /// Windows recognizers/voices (cached; `refresh` re-detects).
    pub async fn windows(&self, refresh: bool) -> local::WindowsSpeech {
        if !refresh {
            if let Some(w) = self.windows.lock().clone() {
                return w;
            }
        }
        let w = local::detect_windows().await;
        *self.windows.lock() = Some(w.clone());
        w
    }

    /// Recognizes `pcm` with exactly `engine`. `key` is the engine's API key
    /// (cloud engines). `raw` is the original upload (forwarded to OpenAI as-is).
    pub async fn transcribe(
        &self,
        engine: SttEngine,
        s: &VoiceEngineSettings,
        key: Option<&str>,
        pcm: &Pcm,
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        let need_key = || {
            key.filter(|k| !k.is_empty()).ok_or_else(|| VoiceError::NeedsKey(format!("{engine:?} needs an API key")))
        };
        match engine {
            SttEngine::Disabled => Err(VoiceError::BadRequest("speech recognition is disabled".into())),
            SttEngine::WebSpeech => Err(VoiceError::BadRequest("Web Speech runs in the app window".into())),
            SttEngine::WhisperCpp => local::whisper(&self.models, s, pcm, language).await,
            SttEngine::Vosk => local::vosk(&self.vosk, &self.models, s, pcm).await,
            SttEngine::SherpaOnnx => local::sherpa(&self.models, s, pcm).await,
            SttEngine::WindowsSpeech => {
                let w = self.windows(false).await;
                local::windows_stt(&self.models, &w, pcm, language).await
            }
            SttEngine::OpenAiWhisper => self.cloud.openai_stt(need_key()?, s, &pcm.to_wav(), language).await,
            SttEngine::Deepgram => self.cloud.deepgram_stt(need_key()?, s, &pcm.to_wav(), language).await,
            SttEngine::AzureSpeech => self.cloud.azure_stt(need_key()?, s, &pcm.to_wav(), language).await,
            SttEngine::GoogleCloud => self.cloud.google_stt(need_key()?, pcm, language).await,
            SttEngine::YandexSpeechKit => self.cloud.yandex_stt(need_key()?, s, pcm, language).await,
        }
    }

    /// Synthesizes `text` with exactly `engine`. `voice` overrides the
    /// engine's configured voice (agent voices, tests).
    #[allow(clippy::too_many_arguments)]
    pub async fn synthesize(
        &self,
        engine: TtsEngine,
        s: &VoiceEngineSettings,
        key: Option<&str>,
        voice: Option<&str>,
        language: &str,
        rate_pct: u16,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        let need_key = || {
            key.filter(|k| !k.is_empty()).ok_or_else(|| VoiceError::NeedsKey(format!("{engine:?} needs an API key")))
        };
        let voice = voice.map(str::trim).filter(|v| !v.is_empty());
        let lang = Some(language);
        match engine {
            TtsEngine::System => Err(VoiceError::BadRequest("system voices are played by the app window".into())),
            TtsEngine::Piper => local::piper(&self.models, voice.unwrap_or(&s.piper_voice), rate_pct, text).await,
            TtsEngine::WindowsOneCore => {
                local::windows_tts(&self.models, voice.or(s.windows_voice.as_deref()), language, rate_pct, text).await
            }
            TtsEngine::Silero => local::silero(&self.models, voice.unwrap_or(&s.silero_speaker), text).await,
            TtsEngine::OpenAi => self.cloud.openai_tts(need_key()?, s, voice.unwrap_or("alloy"), text).await,
            TtsEngine::AzureSpeech => {
                self.cloud.azure_tts(need_key()?, s, voice.unwrap_or(&s.azure_voice), lang, text).await
            }
            TtsEngine::GoogleCloud => self.cloud.google_tts(need_key()?, s, lang, text).await,
            TtsEngine::YandexSpeechKit => {
                self.cloud.yandex_tts(need_key()?, s, voice.unwrap_or(&s.yandex_voice), lang, text).await
            }
            TtsEngine::ElevenLabs => {
                self.cloud.elevenlabs_tts(need_key()?, s, voice.unwrap_or(&s.elevenlabs_voice), text).await
            }
        }
    }

    fn model_options(&self, packages: &[&catalog::Package]) -> Vec<VoiceEngineOption> {
        packages
            .iter()
            .filter(|p| p.kind != orkestr_protocol::methods::voice::VoicePackageKind::Runtime)
            .map(|p| VoiceEngineOption {
                id: p.id.to_owned(),
                label: p.title.to_owned(),
                language: p.languages.first().copied().unwrap_or("*").to_owned(),
                available: self.models.is_installed(p.id),
            })
            .collect()
    }

    fn local_status(&self, runtime: &str, selected: &str) -> (VoiceEngineStatus, Option<String>) {
        let rt = catalog::package(runtime);
        if rt.is_some_and(|p| !p.supported()) {
            return (VoiceEngineStatus::Unavailable, Some("the prebuilt runtime is for Windows x64".into()));
        }
        if !self.models.is_installed(runtime) {
            return (VoiceEngineStatus::NeedsDownload, Some(format!("download `{runtime}`")));
        }
        if !self.models.is_installed(selected) {
            return (VoiceEngineStatus::NeedsDownload, Some(format!("download `{selected}`")));
        }
        (VoiceEngineStatus::Ready, None)
    }

    /// Capabilities and readiness of every engine.
    pub async fn engines(&self, cfg: &VoiceConfig, keys: &[VoiceKeyId], refresh: bool) -> VoiceEnginesResult {
        let s = &cfg.engines;
        let win = self.windows(refresh).await;
        let has_key = |k: VoiceKeyId| keys.contains(&k);
        let cloud_status = |k: VoiceKeyId, extra_ok: bool, extra: &str| {
            if !has_key(k) {
                (VoiceEngineStatus::NeedsKey, Some("paste an API key".to_owned()))
            } else if !extra_ok {
                (VoiceEngineStatus::NeedsKey, Some(extra.to_owned()))
            } else {
                (VoiceEngineStatus::Ready, None)
            }
        };
        let names = |ps: &[&catalog::Package]| ps.iter().map(|p| p.id.to_owned()).collect::<Vec<_>>();
        let langs = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut stt = Vec::new();
        for e in SttEngine::ALL {
            let pk = catalog::stt_packages(e);
            let (name, offline, streaming, languages, quality, speed, (status, detail), verification, options) = match e
            {
                SttEngine::Disabled => continue,
                SttEngine::WhisperCpp => (
                    "whisper.cpp",
                    true,
                    false,
                    langs(&["*"]),
                    VoiceGrade::High,
                    VoiceGrade::Medium,
                    self.local_status("whisper-runtime", &s.whisper_model),
                    VoiceEngineVerification::Tested,
                    self.model_options(&pk),
                ),
                SttEngine::Vosk => (
                    "Vosk",
                    true,
                    true,
                    langs(&["ru", "en", "*"]),
                    VoiceGrade::Medium,
                    VoiceGrade::VeryHigh,
                    self.local_status("vosk-runtime", &s.vosk_model),
                    VoiceEngineVerification::Tested,
                    self.model_options(&pk),
                ),
                SttEngine::SherpaOnnx => (
                    "sherpa-onnx",
                    true,
                    true,
                    langs(&["ru", "*"]),
                    VoiceGrade::Medium,
                    VoiceGrade::VeryHigh,
                    self.local_status("sherpa-runtime", &s.sherpa_model),
                    if self.models.is_installed("sherpa-runtime") {
                        VoiceEngineVerification::Tested
                    } else {
                        VoiceEngineVerification::Untested
                    },
                    self.model_options(&pk),
                ),
                SttEngine::WindowsSpeech => {
                    let ru = local::windows_culture(&win.recognizers, Some("ru")).is_some();
                    let st = if !cfg!(windows) || win.recognizers.is_empty() {
                        (VoiceEngineStatus::Unavailable, Some("no Windows speech recognizer is installed".into()))
                    } else if !ru {
                        (
                            VoiceEngineStatus::Ready,
                            Some(format!(
                                "no ru-RU recognizer (installed: {}); add the Russian speech pack in Windows Settings",
                                win.recognizers.join(", ")
                            )),
                        )
                    } else {
                        (VoiceEngineStatus::Ready, None)
                    };
                    (
                        "Windows Speech Recognition",
                        true,
                        false,
                        win.recognizers.iter().map(|c| orkestr_protocol::methods::voice::primary_language(c)).collect(),
                        VoiceGrade::Low,
                        VoiceGrade::High,
                        st,
                        VoiceEngineVerification::Tested,
                        win.recognizers
                            .iter()
                            .map(|c| VoiceEngineOption {
                                id: c.clone(),
                                label: c.clone(),
                                language: c.clone(),
                                available: true,
                            })
                            .collect(),
                    )
                }
                SttEngine::WebSpeech => (
                    "Web Speech (WebView2)",
                    false,
                    true,
                    langs(&["*"]),
                    VoiceGrade::Medium,
                    VoiceGrade::High,
                    (VoiceEngineStatus::Client, None),
                    VoiceEngineVerification::Untested,
                    Vec::new(),
                ),
                SttEngine::OpenAiWhisper => (
                    "OpenAI (gpt-4o-transcribe / whisper-1)",
                    false,
                    false,
                    langs(&["*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::Medium,
                    cloud_status(VoiceKeyId::OpenAi, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"]
                        .iter()
                        .map(|m| VoiceEngineOption {
                            id: (*m).into(),
                            label: (*m).into(),
                            language: "*".into(),
                            available: true,
                        })
                        .collect(),
                ),
                SttEngine::Deepgram => (
                    "Deepgram",
                    false,
                    true,
                    langs(&["*"]),
                    VoiceGrade::High,
                    VoiceGrade::VeryHigh,
                    cloud_status(VoiceKeyId::Deepgram, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    ["nova-2", "nova-3"]
                        .iter()
                        .map(|m| VoiceEngineOption {
                            id: (*m).into(),
                            label: (*m).into(),
                            language: "*".into(),
                            available: true,
                        })
                        .collect(),
                ),
                SttEngine::AzureSpeech => (
                    "Azure AI Speech",
                    false,
                    true,
                    langs(&["*"]),
                    VoiceGrade::High,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::AzureSpeech, !s.azure_region.is_empty(), "set the Azure region"),
                    VoiceEngineVerification::RequestShapeVerified,
                    Vec::new(),
                ),
                SttEngine::GoogleCloud => (
                    "Google Cloud Speech-to-Text",
                    false,
                    true,
                    langs(&["*"]),
                    VoiceGrade::High,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::GoogleCloud, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    Vec::new(),
                ),
                SttEngine::YandexSpeechKit => (
                    "Yandex SpeechKit",
                    false,
                    true,
                    langs(&["ru", "en", "*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::YandexSpeechKit, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    Vec::new(),
                ),
            };
            let russian =
                e != SttEngine::WindowsSpeech || local::windows_culture(&win.recognizers, Some("ru")).is_some();
            stt.push(VoiceEngineInfo {
                stt: Some(e),
                tts: None,
                name: name.to_owned(),
                offline,
                cloud: e.needs_consent(),
                streaming,
                languages,
                russian,
                quality,
                speed,
                status,
                detail,
                packages: names(&pk),
                key: e.key(),
                key_configured: e.key().is_some_and(has_key),
                needs_consent: e.needs_consent(),
                consented: cfg.stt_engine_consented(e),
                verification,
                options,
            });
        }
        let mut tts = Vec::new();
        for e in TtsEngine::ALL {
            let pk = catalog::tts_packages(e);
            let simple = |ids: &[(&str, &str)]| {
                ids.iter()
                    .map(|(id, l)| VoiceEngineOption {
                        id: (*id).into(),
                        label: (*id).into(),
                        language: (*l).into(),
                        available: true,
                    })
                    .collect::<Vec<_>>()
            };
            let (name, offline, languages, quality, speed, (status, detail), verification, options) = match e {
                TtsEngine::System => (
                    "System voices (WebView2)",
                    true,
                    langs(&["*"]),
                    VoiceGrade::Medium,
                    VoiceGrade::VeryHigh,
                    (VoiceEngineStatus::Client, None),
                    VoiceEngineVerification::Untested,
                    Vec::new(),
                ),
                TtsEngine::Piper => (
                    "Piper",
                    true,
                    langs(&["ru", "en", "*"]),
                    VoiceGrade::High,
                    VoiceGrade::VeryHigh,
                    self.local_status("piper-runtime", &s.piper_voice),
                    VoiceEngineVerification::Tested,
                    self.model_options(&pk),
                ),
                TtsEngine::WindowsOneCore => (
                    "Windows OneCore voices",
                    true,
                    win.voices
                        .iter()
                        .map(|v| orkestr_protocol::methods::voice::primary_language(&v.language))
                        .collect(),
                    VoiceGrade::Medium,
                    VoiceGrade::VeryHigh,
                    if win.voices.is_empty() {
                        (VoiceEngineStatus::Unavailable, Some("no Windows voices found".into()))
                    } else {
                        (VoiceEngineStatus::Ready, None)
                    },
                    VoiceEngineVerification::Tested,
                    win.voices
                        .iter()
                        .map(|v| VoiceEngineOption {
                            id: v.name.clone(),
                            label: format!("{} ({})", v.name, v.language),
                            language: v.language.clone(),
                            available: true,
                        })
                        .collect(),
                ),
                TtsEngine::Silero => (
                    "Silero TTS",
                    true,
                    langs(&["ru"]),
                    VoiceGrade::High,
                    VoiceGrade::Medium,
                    if local::silero_python().is_some() {
                        (VoiceEngineStatus::Ready, None)
                    } else {
                        (
                            VoiceEngineStatus::Unavailable,
                            Some(format!("needs Python + PyTorch (~1 GB); set {}", local::SILERO_PYTHON_ENV)),
                        )
                    },
                    VoiceEngineVerification::Untested,
                    simple(&[
                        ("xenia", "ru-RU"),
                        ("baya", "ru-RU"),
                        ("kseniya", "ru-RU"),
                        ("aidar", "ru-RU"),
                        ("eugene", "ru-RU"),
                    ]),
                ),
                TtsEngine::OpenAi => (
                    "OpenAI TTS",
                    false,
                    langs(&["*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::Medium,
                    cloud_status(VoiceKeyId::OpenAi, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    OPENAI_VOICES
                        .iter()
                        .map(|v| VoiceEngineOption {
                            id: (*v).into(),
                            label: (*v).into(),
                            language: "*".into(),
                            available: true,
                        })
                        .collect(),
                ),
                TtsEngine::AzureSpeech => (
                    "Azure AI Speech",
                    false,
                    langs(&["*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::AzureSpeech, !s.azure_region.is_empty(), "set the Azure region"),
                    VoiceEngineVerification::RequestShapeVerified,
                    simple(AZURE_VOICES),
                ),
                TtsEngine::GoogleCloud => (
                    "Google Cloud Text-to-Speech",
                    false,
                    langs(&["*"]),
                    VoiceGrade::High,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::GoogleCloud, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    simple(&[
                        ("ru-RU-Wavenet-A", "ru-RU"),
                        ("ru-RU-Wavenet-B", "ru-RU"),
                        ("ru-RU-Standard-A", "ru-RU"),
                    ]),
                ),
                TtsEngine::YandexSpeechKit => (
                    "Yandex SpeechKit",
                    false,
                    langs(&["ru", "en", "*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::High,
                    cloud_status(VoiceKeyId::YandexSpeechKit, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    simple(YANDEX_VOICES),
                ),
                TtsEngine::ElevenLabs => (
                    "ElevenLabs",
                    false,
                    langs(&["*"]),
                    VoiceGrade::VeryHigh,
                    VoiceGrade::Medium,
                    cloud_status(VoiceKeyId::ElevenLabs, true, ""),
                    VoiceEngineVerification::RequestShapeVerified,
                    Vec::new(),
                ),
            };
            let russian = e != TtsEngine::WindowsOneCore || win.voices.iter().any(|v| v.language.starts_with("ru"));
            tts.push(VoiceEngineInfo {
                stt: None,
                tts: Some(e),
                name: name.to_owned(),
                offline,
                cloud: e.is_cloud(),
                streaming: false,
                languages,
                russian,
                quality,
                speed,
                status,
                detail,
                packages: names(&pk),
                key: e.key(),
                key_configured: e.key().is_some_and(has_key),
                needs_consent: e.is_cloud(),
                consented: cfg.tts_engine_consented(e),
                verification,
                options,
            });
        }
        VoiceEnginesResult { stt, tts, configured_keys: keys.to_vec() }
    }
}
