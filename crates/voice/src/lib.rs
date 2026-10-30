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
pub mod barge;
pub mod catalog;
pub mod cloud;
pub mod gate;
pub mod local;
pub mod models;
pub mod proc;
pub mod whisper_server;

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_protocol::methods::voice::{
    NoiseRejection, SttEngine, SttRecommendation, SttRejectReason, TtsEngine, VoiceConfig, VoiceEngineInfo,
    VoiceEngineOption, VoiceEngineSettings, VoiceEngineStatus, VoiceEngineVerification, VoiceEnginesResult, VoiceGrade,
    VoiceKeyId,
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
    whisper: whisper_server::WhisperServers,
    gpus: Mutex<Option<Vec<orkestr_platform::GpuDevice>>>,
    windows: Mutex<Option<local::WindowsSpeech>>,
    barge: Mutex<BargeSessions>,
}

/// Open barge-in sessions (ADR-0054).
#[derive(Default)]
struct BargeSessions {
    next: u64,
    open: std::collections::HashMap<u64, BargeSession>,
}

struct BargeSession {
    spotter: Arc<Mutex<barge::BargeSpotter>>,
    at: std::time::Instant,
    engines: VoiceEngineSettings,
    language: Option<String>,
}

/// Max concurrent barge-in sessions (one per window that speaks; stale ones are dropped).
const MAX_BARGE_SESSIONS: usize = 4;
/// A session not fed for this long is closed.
const BARGE_IDLE: std::time::Duration = std::time::Duration::from_secs(60);
/// The Vosk model with a grammar-capable (dynamic) graph used for stop phrases.
pub const BARGE_VOSK_MODEL: &str = "vosk-small-ru";

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
        let whisper = whisper_server::WhisperServers::new(&voice_dir.join("models").join(".tmp"));
        Self {
            whisper,
            gpus: Mutex::new(None),
            models: models::ModelManager::new(voice_dir.join("models"), http.clone()),
            cloud: cloud::Cloud { http, endpoints: cloud::Endpoints::default() },
            vosk: Arc::default(),
            windows: Mutex::new(None),
            barge: Mutex::new(BargeSessions::default()),
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
            SttEngine::WhisperCpp => {
                let segs =
                    local::whisper(&self.models, &self.whisper, s, pcm, language, &local::WhisperCall::default())
                        .await?;
                Ok(segs.iter().map(|x| x.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" "))
            }
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

    /// Recognizes `pcm` behind the noise gates (ADR-0057): speech activity
    /// first (silence / steady noise never reach the engine), whisper.cpp with
    /// the vocabulary prompt + Silero VAD + decoder confidence, then the
    /// hallucination filter. A rejection is `Ok` with an empty text.
    pub async fn transcribe_checked(
        &self,
        engine: SttEngine,
        s: &VoiceEngineSettings,
        key: Option<&str>,
        pcm: &Pcm,
        language: Option<&str>,
        hints: &SttHints,
    ) -> Result<Transcript, VoiceError> {
        let level = s.noise_rejection;
        if level != NoiseRejection::Off {
            let a = gate::speech_activity(&pcm.samples);
            let strict_ok = level != NoiseRejection::Strict || a.snr_db >= gate::STRICT_SNR_DB;
            if !a.has_speech() || !strict_ok {
                tracing::debug!(?a, "voice: no speech in the recording");
                return Ok(Transcript::rejected(SttRejectReason::NoSpeech));
            }
        }
        let (text, confidence) = if engine == SttEngine::WhisperCpp {
            let prompt = gate::initial_prompt(language, &hints.wake_phrases);
            let call = local::WhisperCall { prompt: Some(&prompt), vad: level != NoiseRejection::Off };
            let segs = local::whisper(&self.models, &self.whisper, s, pcm, language, &call).await?;
            match gate::confidence(&segs) {
                Some((t, c)) if !gate::copies_prompt(&t, &prompt) => (t, Some(c)),
                Some(_) => return Ok(Transcript::rejected(SttRejectReason::Hallucination)),
                None => return Ok(Transcript::rejected(SttRejectReason::NoSpeech)),
            }
        } else {
            (self.transcribe(engine, s, key, pcm, language).await?, None)
        };
        let text = match gate::check_text(&text) {
            Ok(t) => t,
            Err(r) => {
                tracing::debug!(reason = ?r, "voice: transcript rejected");
                return Ok(Transcript::rejected(r));
            }
        };
        if let Some(c) = confidence {
            let min = match level {
                NoiseRejection::Off => 0.0,
                NoiseRejection::Normal => gate::REJECT_LOGPROB.exp(),
                NoiseRejection::Strict => gate::STRICT_LOGPROB.exp(),
            };
            if c < min {
                tracing::debug!(confidence = c, "voice: transcript too uncertain");
                return Ok(Transcript::rejected(SttRejectReason::LowConfidence));
            }
        }
        Ok(Transcript { text, confidence, rejected: None })
    }

    /// Loads the whisper model ahead of the first command (resident server).
    pub async fn prewarm(&self, s: &VoiceEngineSettings) {
        let Some((runtime, gpu)) = local::whisper_runtime(&self.models, s.whisper_gpu) else { return };
        let Some(model) = self.models.installed_dir(&s.whisper_model).and_then(|d| {
            std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "bin"))
        }) else {
            return;
        };
        let key = whisper_server::ServerKey { runtime, model, vad: local::whisper_vad_model(&self.models), gpu };
        if self.whisper.is_running(&key).await {
            return;
        }
        if let Err(e) = self.whisper.ensure(&key).await {
            tracing::warn!(error = %e, "could not preload the whisper model");
            return;
        }
        // One tiny decode: the first CUDA run compiles kernels (up to ~30 s on
        // a fresh driver cache) — pay it here, not on the user's first command.
        let lang = s.whisper_model.clone();
        let quiet = audio::Pcm { samples: vec![0; audio::SAMPLE_RATE as usize / 2] };
        let req = whisper_server::Request { language: "ru", prompt: None, vad: false };
        let t = std::time::Instant::now();
        if let Err(e) = self.whisper.transcribe(&key, &quiet.to_wav(), &req).await {
            tracing::warn!(error = %e, "whisper warm-up failed");
        }
        tracing::info!(model = %lang, ms = t.elapsed().as_millis(), "whisper warmed up");
    }

    /// Stops the resident whisper server (engine switched away, shutdown).
    pub async fn stop_whisper(&self) {
        self.whisper.stop().await;
    }

    /// GPUs of this machine (detected once).
    async fn gpus(&self) -> Vec<orkestr_platform::GpuDevice> {
        if let Some(g) = self.gpus.lock().clone() {
            return g;
        }
        let g = tokio::task::spawn_blocking(|| orkestr_platform::detect_gpus(std::time::Duration::from_secs(8)))
            .await
            .unwrap_or_default();
        *self.gpus.lock() = Some(g.clone());
        g
    }

    /// The best local recognizer for this PC and what is missing for it (ADR-0057).
    pub async fn stt_recommendation(&self, cfg: &VoiceConfig) -> Option<SttRecommendation> {
        let gpus = self.gpus().await;
        let (ram, cores) = {
            use orkestr_platform::ResourceMonitor as _;
            let snap = orkestr_platform::SysinfoResourceMonitor::new(self.models.root().to_path_buf()).snapshot();
            (snap.memory_total_bytes, snap.cpu_cores)
        };
        let pick = recommend(&gpus, ram, cores, cfg!(windows) && cfg!(target_arch = "x86_64"));
        let mut packages: Vec<String> = Vec::new();
        let mut add = |id: &str| {
            let Some(p) = catalog::package(id) else { return };
            for d in p.depends.iter().copied().chain(std::iter::once(id)) {
                if !self.models.is_installed(d) && !packages.iter().any(|x| x == d) {
                    packages.push(d.to_owned());
                }
            }
        };
        add("whisper-runtime");
        if pick.gpu {
            add(catalog::WHISPER_CUDA);
        }
        add(pick.model);
        add(catalog::WHISPER_VAD);
        let download_bytes =
            packages.iter().filter_map(|id| catalog::package(id)).map(catalog::Package::download_bytes).sum();
        let active = cfg.stt_engine == SttEngine::WhisperCpp
            && cfg.engines.whisper_model == pick.model
            && cfg.engines.whisper_gpu == pick.gpu
            && packages.is_empty();
        Some(SttRecommendation {
            engine: SttEngine::WhisperCpp,
            model: pick.model.to_owned(),
            gpu: pick.gpu,
            packages,
            download_bytes,
            gpu_name: pick.gpu_name,
            active,
        })
    }

    /// Opens a barge-in session: a Vosk grammar recognizer when `vosk-small-ru`
    /// is installed, else voiced windows for an installed offline engine
    /// (whisper.cpp / sherpa-onnx). `Ok(None)` when nothing can spot phrases.
    pub async fn barge_open(
        &self,
        cfg: &orkestr_protocol::methods::voice::BargeInConfig,
        engines: &VoiceEngineSettings,
        language: Option<&str>,
    ) -> Result<Option<(u64, orkestr_protocol::methods::voice::BargeEngine)>, VoiceError> {
        let phrases = barge::StopPhrases::from_config(cfg);
        if phrases.is_empty() {
            return Ok(None);
        }
        let vosk = self.models.installed_dir("vosk-runtime").zip(self.models.installed_dir(BARGE_VOSK_MODEL));
        let rec = match vosk {
            Some((runtime, model_dir)) => {
                let cache = Arc::clone(&self.vosk);
                let grammar = phrases.grammar(!BARGE_VOSK_MODEL.ends_with("-ru"));
                let made = tokio::task::spawn_blocking(move || {
                    let model = local::vosk_model_blocking(&cache, &runtime, &model_dir)?;
                    let fail = |e: orkestr_platform::PlatformError| VoiceError::Failed(e.to_string());
                    let stream = orkestr_platform::vosk::VoskStream::new(&model, audio::SAMPLE_RATE, Some(&grammar))
                        .map_err(fail)?;
                    Ok::<_, VoiceError>(barge::Recognizer::Vosk(stream))
                })
                .await
                .map_err(|e| VoiceError::Failed(e.to_string()))?;
                match made {
                    Ok(rec) => Some(rec),
                    Err(e) => {
                        tracing::warn!(error = %e, "barge-in: Vosk grammar recognizer unavailable, using windows");
                        None
                    }
                }
            }
            None => None,
        };
        let rec = match rec {
            Some(r) => r,
            None if self.barge_window_engine(engines).is_some() => barge::Recognizer::Window,
            None => return Ok(None),
        };
        let spotter = barge::BargeSpotter::new(phrases, cfg.sensitivity, rec);
        let engine = spotter.engine();
        let mut g = self.barge.lock();
        let now = std::time::Instant::now();
        g.open.retain(|_, b| now.duration_since(b.at) < BARGE_IDLE);
        while g.open.len() >= MAX_BARGE_SESSIONS {
            let Some(oldest) = g.open.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| *k) else { break };
            g.open.remove(&oldest);
        }
        g.next += 1;
        let id = g.next;
        g.open.insert(
            id,
            BargeSession {
                spotter: Arc::new(Mutex::new(spotter)),
                at: now,
                engines: engines.clone(),
                language: language.map(str::to_owned),
            },
        );
        Ok(Some((id, engine)))
    }

    /// The offline engine that transcribes fallback windows, if one is installed.
    fn barge_window_engine(&self, s: &VoiceEngineSettings) -> Option<SttEngine> {
        let m = &self.models;
        if m.is_installed("whisper-runtime") && m.is_installed(&s.whisper_model) {
            Some(SttEngine::WhisperCpp)
        } else if m.is_installed("sherpa-runtime") && m.is_installed(&s.sherpa_model) {
            Some(SttEngine::SherpaOnnx)
        } else {
            None
        }
    }

    /// Feeds a chunk to a barge-in session. `Err(BadRequest)` for an unknown / expired session.
    pub async fn barge_feed(
        &self,
        id: u64,
        pcm: Vec<i16>,
        spoken: Option<&str>,
    ) -> Result<Option<orkestr_protocol::methods::voice::BargeHit>, VoiceError> {
        let (session, engines, language) = {
            let mut g = self.barge.lock();
            let entry = g.open.get_mut(&id).ok_or_else(|| VoiceError::BadRequest("unknown barge-in session".into()))?;
            entry.at = std::time::Instant::now();
            (Arc::clone(&entry.spotter), entry.engines.clone(), entry.language.clone())
        };
        // Vosk decoding of ~100 ms takes a few ms: run it off the async workers.
        let spoken_owned = spoken.map(str::to_owned);
        let feeder = Arc::clone(&session);
        let out = tokio::task::spawn_blocking(move || feeder.lock().feed(&pcm, spoken_owned.as_deref()))
            .await
            .map_err(|e| VoiceError::Failed(e.to_string()))?;
        if out.hit.is_some() {
            return Ok(out.hit);
        }
        let Some(window) = out.window else { return Ok(None) };
        let Some(engine) = self.barge_window_engine(&engines) else { return Ok(None) };
        let text =
            self.transcribe(engine, &engines, None, &audio::Pcm { samples: window }, language.as_deref()).await?;
        let hit = session.lock().classify_text(&text, spoken);
        Ok(hit)
    }

    /// Closes a barge-in session (unknown ids are ignored).
    pub fn barge_close(&self, id: u64) {
        self.barge.lock().open.remove(&id);
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
        let stt_recommendation = self.stt_recommendation(cfg).await;
        VoiceEnginesResult { stt, tts, configured_keys: keys.to_vec(), stt_recommendation }
    }
}

/// Context for [`VoiceService::transcribe_checked`].
#[derive(Debug, Clone, Default)]
pub struct SttHints {
    /// Wake phrases (put first into whisper's vocabulary prompt).
    pub wake_phrases: Vec<String>,
}

/// A checked transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    /// Text (empty when rejected).
    pub text: String,
    /// Decoder confidence 0..1 (whisper.cpp only).
    pub confidence: Option<f32>,
    /// Why the audio was not accepted as speech.
    pub rejected: Option<SttRejectReason>,
}

impl Transcript {
    fn rejected(r: SttRejectReason) -> Self {
        Self { text: String::new(), confidence: None, rejected: Some(r) }
    }
}

/// A recommended whisper setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// Model package.
    pub model: &'static str,
    /// Use the CUDA runtime.
    pub gpu: bool,
    /// The GPU it is for.
    pub gpu_name: Option<String>,
}

/// Minimum VRAM for large-v3-turbo q5_0 on the GPU (model ≈ 0.6 GB + buffers ≈ 1 GB).
const TURBO_MIN_VRAM: u64 = 3 * 1024 * 1024 * 1024;

/// Picks the whisper setup for this hardware (ADR-0057): large-v3-turbo on an
/// NVIDIA GPU with ≥ 3 GB VRAM (CUDA build, Windows x64); on the CPU `small`
/// for 4+ cores with 8 GB RAM, `base` otherwise.
#[must_use]
pub fn recommend(gpus: &[orkestr_platform::GpuDevice], ram_bytes: u64, cores: u32, cuda_build: bool) -> Pick {
    const GB: u64 = 1024 * 1024 * 1024;
    let best = gpus
        .iter()
        .filter(|g| {
            g.vendor.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("nvidia"))
                || g.name.to_lowercase().contains("nvidia")
        })
        .max_by_key(|g| g.vram_bytes.unwrap_or(0));
    if let Some(g) = best.filter(|g| cuda_build && g.vram_bytes.unwrap_or(0) >= TURBO_MIN_VRAM) {
        #[allow(clippy::cast_precision_loss)]
        let vram = g.vram_bytes.unwrap_or(0) as f64 / GB as f64;
        return Pick { model: catalog::WHISPER_TURBO, gpu: true, gpu_name: Some(format!("{}, {vram:.0} GB", g.name)) };
    }
    let model = if cores >= 4 && ram_bytes >= 7 * GB { "whisper-small" } else { "whisper-base" };
    Pick { model, gpu: false, gpu_name: None }
}

#[cfg(test)]
mod recommend_tests {
    use super::*;

    fn gpu(name: &str, vram_gb: u64) -> orkestr_platform::GpuDevice {
        orkestr_platform::GpuDevice {
            name: name.into(),
            vendor: orkestr_platform::gpu::vendor_of(name),
            vram_bytes: Some(vram_gb * 1024 * 1024 * 1024),
            driver_version: None,
            source: "nvidia-smi",
        }
    }

    #[test]
    fn picks_turbo_on_a_capable_nvidia_gpu_and_falls_back_otherwise() {
        const GB: u64 = 1024 * 1024 * 1024;
        let p = recommend(&[gpu("NVIDIA GeForce RTX 2060 SUPER", 8)], 32 * GB, 20, true);
        assert_eq!((p.model, p.gpu), (catalog::WHISPER_TURBO, true));
        assert_eq!(p.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 2060 SUPER, 8 GB"));
        let p = recommend(&[gpu("NVIDIA GeForce GT 1030", 2)], 16 * GB, 8, true);
        assert_eq!((p.model, p.gpu), ("whisper-small", false));
        let p = recommend(&[gpu("AMD Radeon RX 6800", 16)], 16 * GB, 8, true);
        assert!(!p.gpu);
        let p = recommend(&[gpu("NVIDIA GeForce RTX 4090", 24)], 64 * GB, 16, false);
        assert!(!p.gpu, "no CUDA build for this OS/CPU");
        assert_eq!(recommend(&[], 4 * GB, 2, true).model, "whisper-base");
    }
}
