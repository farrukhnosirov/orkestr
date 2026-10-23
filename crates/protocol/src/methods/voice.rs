//! `voice.*` — Voice Command Center, voice notifications and sound engine
//! settings (§33, §63 G, ADR-0017).
//!
//! Privacy model (§33, §56):
//! - Wake-word detection runs **on the device** (desktop webview AudioWorklet);
//!   no audio is sent anywhere before the wake word or push-to-talk fires.
//! - Speech-to-text engines other than `Disabled` process audio off-device
//!   (`WebSpeech` uses the browser/OS speech service, `OpenAiWhisper` the
//!   user's OpenAI provider). They need an explicit, per-engine consent
//!   record that only `voice.setConsent` can write; `voice.setConfig` never
//!   changes consent and rejects an engine without it.
//! - Cloud text-to-speech (`TtsEngine::OpenAi`) sends text (never audio) and
//!   needs its own consent.
//! - `voice.recordCommand` logs the transcript and the parsed intent as a
//!   `VoiceCommand` event. Audio is never stored.
//!
//! The configuration is one JSON document stored by the core; every field
//! has a default, so older/newer documents load without migrations.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use std::collections::BTreeMap;

use orkestr_domain::{ProjectId, ProviderId, Timestamp, VoiceProfile};

use super::method;

/// Speech-to-text engine (after push-to-talk or the wake word).
///
/// ADR-0046: engines run in the core (`orkestr-voice`) except `WebSpeech`,
/// which lives in the webview. Local engines (`WhisperCpp`, `Vosk`,
/// `SherpaOnnx`, `WindowsSpeech`) keep audio on the device and need no
/// consent; cloud engines need a per-engine consent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SttEngine {
    /// No speech recognition: the voice HUD accepts typed commands only.
    #[default]
    Disabled,
    /// The webview's Web Speech API (audio is processed by the browser/OS
    /// speech service, usually online). Needs consent.
    WebSpeech,
    /// OpenAI transcription (`whisper-1`, `gpt-4o-transcribe`, …) through the
    /// voice API key or the user's OpenAI provider. Needs consent.
    OpenAiWhisper,
    /// whisper.cpp (local, multilingual; ADR-0046).
    WhisperCpp,
    /// Vosk / Kaldi (local, small fast models).
    Vosk,
    /// sherpa-onnx (local; zipformer / GigaAM ONNX models).
    SherpaOnnx,
    /// Windows built-in speech recognition (`System.Speech`; local).
    WindowsSpeech,
    /// Deepgram (cloud). Needs consent and an API key.
    Deepgram,
    /// Azure AI Speech (cloud). Needs consent, an API key and a region.
    AzureSpeech,
    /// Google Cloud Speech-to-Text (cloud). Needs consent and an API key.
    GoogleCloud,
    /// Yandex SpeechKit (cloud). Needs consent and an API key.
    YandexSpeechKit,
}

impl SttEngine {
    /// Every engine, in display order.
    pub const ALL: [SttEngine; 11] = [
        SttEngine::Disabled,
        SttEngine::WhisperCpp,
        SttEngine::Vosk,
        SttEngine::SherpaOnnx,
        SttEngine::WindowsSpeech,
        SttEngine::WebSpeech,
        SttEngine::OpenAiWhisper,
        SttEngine::Deepgram,
        SttEngine::AzureSpeech,
        SttEngine::GoogleCloud,
        SttEngine::YandexSpeechKit,
    ];

    /// Whether audio leaves the device with this engine (consent required).
    #[must_use]
    pub fn needs_consent(self) -> bool {
        !matches!(
            self,
            SttEngine::Disabled
                | SttEngine::WhisperCpp
                | SttEngine::Vosk
                | SttEngine::SherpaOnnx
                | SttEngine::WindowsSpeech
        )
    }

    /// Whether the core runs this engine (`voice.transcribe`); `WebSpeech`
    /// runs in the webview and `Disabled` runs nowhere.
    #[must_use]
    pub fn runs_in_core(self) -> bool {
        !matches!(self, SttEngine::Disabled | SttEngine::WebSpeech)
    }

    /// The API key this engine needs, if any.
    #[must_use]
    pub fn key(self) -> Option<VoiceKeyId> {
        match self {
            SttEngine::OpenAiWhisper => Some(VoiceKeyId::OpenAi),
            SttEngine::Deepgram => Some(VoiceKeyId::Deepgram),
            SttEngine::AzureSpeech => Some(VoiceKeyId::AzureSpeech),
            SttEngine::GoogleCloud => Some(VoiceKeyId::GoogleCloud),
            SttEngine::YandexSpeechKit => Some(VoiceKeyId::YandexSpeechKit),
            _ => None,
        }
    }
}

/// Text-to-speech engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TtsEngine {
    /// The OS voices through the webview's `speechSynthesis` (offline).
    #[default]
    System,
    /// OpenAI text-to-speech (opt-in; sends text).
    OpenAi,
    /// Piper (local neural voices; ADR-0046).
    Piper,
    /// Silero TTS (local; needs a Python + PyTorch runtime).
    Silero,
    /// Windows OneCore voices rendered by the core (`Windows.Media.SpeechSynthesis`).
    WindowsOneCore,
    /// Azure AI Speech neural voices (cloud).
    AzureSpeech,
    /// Google Cloud Text-to-Speech (cloud).
    GoogleCloud,
    /// Yandex SpeechKit (cloud).
    YandexSpeechKit,
    /// ElevenLabs (cloud).
    ElevenLabs,
}

impl TtsEngine {
    /// Every engine, in display order.
    pub const ALL: [TtsEngine; 9] = [
        TtsEngine::System,
        TtsEngine::Piper,
        TtsEngine::WindowsOneCore,
        TtsEngine::Silero,
        TtsEngine::OpenAi,
        TtsEngine::AzureSpeech,
        TtsEngine::GoogleCloud,
        TtsEngine::YandexSpeechKit,
        TtsEngine::ElevenLabs,
    ];

    /// Whether text leaves the device (consent required).
    #[must_use]
    pub fn is_cloud(self) -> bool {
        matches!(
            self,
            TtsEngine::OpenAi
                | TtsEngine::AzureSpeech
                | TtsEngine::GoogleCloud
                | TtsEngine::YandexSpeechKit
                | TtsEngine::ElevenLabs
        )
    }

    /// Whether the core renders audio for this engine (`voice.synthesize`);
    /// `System` is played by the webview.
    #[must_use]
    pub fn runs_in_core(self) -> bool {
        self != TtsEngine::System
    }

    /// The API key this engine needs, if any.
    #[must_use]
    pub fn key(self) -> Option<VoiceKeyId> {
        match self {
            TtsEngine::OpenAi => Some(VoiceKeyId::OpenAi),
            TtsEngine::AzureSpeech => Some(VoiceKeyId::AzureSpeech),
            TtsEngine::GoogleCloud => Some(VoiceKeyId::GoogleCloud),
            TtsEngine::YandexSpeechKit => Some(VoiceKeyId::YandexSpeechKit),
            TtsEngine::ElevenLabs => Some(VoiceKeyId::ElevenLabs),
            _ => None,
        }
    }

    /// The `VoiceProfile.provider` string of this engine.
    #[must_use]
    pub fn profile_provider(self) -> &'static str {
        match self {
            TtsEngine::System => "system",
            TtsEngine::OpenAi => "openai",
            TtsEngine::Piper => "piper",
            TtsEngine::Silero => "silero",
            TtsEngine::WindowsOneCore => "windows",
            TtsEngine::AzureSpeech => "azure",
            TtsEngine::GoogleCloud => "google",
            TtsEngine::YandexSpeechKit => "yandex",
            TtsEngine::ElevenLabs => "elevenlabs",
        }
    }

    /// Parses a `VoiceProfile.provider` string.
    #[must_use]
    pub fn from_profile_provider(s: &str) -> Option<TtsEngine> {
        TtsEngine::ALL.into_iter().find(|e| e.profile_provider() == s)
    }
}

/// A paid engine's API key slot in the secret vault (ADR-0046). Keys are
/// write-only over RPC: `voice.setEngineKey` stores or clears them, and
/// `voice.listEngines` reports only whether one is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceKeyId {
    /// OpenAI (STT + TTS). Without it the user's OpenAI provider is used.
    OpenAi,
    /// Deepgram (STT).
    Deepgram,
    /// Azure AI Speech (STT + TTS).
    AzureSpeech,
    /// Google Cloud (STT + TTS).
    GoogleCloud,
    /// Yandex SpeechKit (STT + TTS).
    YandexSpeechKit,
    /// ElevenLabs (TTS).
    ElevenLabs,
}

impl VoiceKeyId {
    /// Every key slot.
    pub const ALL: [VoiceKeyId; 6] = [
        VoiceKeyId::OpenAi,
        VoiceKeyId::Deepgram,
        VoiceKeyId::AzureSpeech,
        VoiceKeyId::GoogleCloud,
        VoiceKeyId::YandexSpeechKit,
        VoiceKeyId::ElevenLabs,
    ];

    /// Name of the global secret holding the key.
    #[must_use]
    pub fn secret_name(self) -> &'static str {
        match self {
            VoiceKeyId::OpenAi => "voice.openai.apiKey",
            VoiceKeyId::Deepgram => "voice.deepgram.apiKey",
            VoiceKeyId::AzureSpeech => "voice.azure.apiKey",
            VoiceKeyId::GoogleCloud => "voice.google.apiKey",
            VoiceKeyId::YandexSpeechKit => "voice.yandex.apiKey",
            VoiceKeyId::ElevenLabs => "voice.elevenlabs.apiKey",
        }
    }
}

/// A consent for one cloud text-to-speech engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TtsConsent {
    /// Engine.
    pub engine: TtsEngine,
    /// When it was given.
    pub granted_at: Timestamp,
}

/// Per-engine options (ADR-0046). Every field has a default; model and
/// voice ids refer to packages of the voice catalog (`voice.listModels`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VoiceEngineSettings {
    /// whisper.cpp model package id.
    pub whisper_model: String,
    /// Run whisper.cpp on the GPU (needs the CUDA runtime package).
    pub whisper_gpu: bool,
    /// Vosk model package id.
    pub vosk_model: String,
    /// sherpa-onnx model package id.
    pub sherpa_model: String,
    /// Piper voice package id.
    pub piper_voice: String,
    /// Windows OneCore voice display name (`None` = best match for the language).
    pub windows_voice: Option<String>,
    /// Silero speaker (`xenia`, `aidar`, `baya`, `kseniya`, `eugene`).
    pub silero_speaker: String,
    /// OpenAI transcription model.
    pub openai_stt_model: String,
    /// OpenAI speech model.
    pub openai_tts_model: String,
    /// Deepgram model.
    pub deepgram_model: String,
    /// Azure region, e.g. `westeurope`.
    pub azure_region: String,
    /// Azure neural voice.
    pub azure_voice: String,
    /// Google voice name (empty = the service default for the language).
    pub google_voice: String,
    /// Yandex Cloud folder id (needed for user-account keys; optional for service-account API keys).
    pub yandex_folder_id: String,
    /// Yandex voice.
    pub yandex_voice: String,
    /// ElevenLabs voice id.
    pub elevenlabs_voice: String,
    /// ElevenLabs model.
    pub elevenlabs_model: String,
}

impl Default for VoiceEngineSettings {
    fn default() -> Self {
        Self {
            whisper_model: "whisper-base".into(),
            whisper_gpu: false,
            vosk_model: "vosk-small-ru".into(),
            sherpa_model: "sherpa-zipformer-small-ru-int8".into(),
            piper_voice: "piper-ru-irina".into(),
            windows_voice: None,
            silero_speaker: "xenia".into(),
            openai_stt_model: "gpt-4o-transcribe".into(),
            openai_tts_model: "gpt-4o-mini-tts".into(),
            deepgram_model: "nova-2".into(),
            azure_region: String::new(),
            azure_voice: "ru-RU-SvetlanaNeural".into(),
            google_voice: String::new(),
            yandex_folder_id: String::new(),
            yandex_voice: "alena".into(),
            elevenlabs_voice: "21m00Tcm4TlvDq8ikWAM".into(),
            elevenlabs_model: "eleven_multilingual_v2".into(),
        }
    }
}

/// Whether `s` is a short id (`[A-Za-z0-9._-]`, 1..=`max`).
#[must_use]
pub fn is_voice_id(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl VoiceEngineSettings {
    /// Validates every field.
    pub fn validate(&self) -> Result<(), VoiceConfigError> {
        let ids = [
            ("engines.whisperModel", &self.whisper_model),
            ("engines.voskModel", &self.vosk_model),
            ("engines.sherpaModel", &self.sherpa_model),
            ("engines.piperVoice", &self.piper_voice),
            ("engines.sileroSpeaker", &self.silero_speaker),
            ("engines.openaiSttModel", &self.openai_stt_model),
            ("engines.openaiTtsModel", &self.openai_tts_model),
            ("engines.deepgramModel", &self.deepgram_model),
            ("engines.azureVoice", &self.azure_voice),
            ("engines.yandexVoice", &self.yandex_voice),
            ("engines.elevenlabsVoice", &self.elevenlabs_voice),
            ("engines.elevenlabsModel", &self.elevenlabs_model),
        ];
        for (field, v) in ids {
            if !is_voice_id(v, 64) {
                return Err(invalid(field, "1..=64 chars of [A-Za-z0-9._-]"));
            }
        }
        for (field, v) in
            [("engines.googleVoice", &self.google_voice), ("engines.yandexFolderId", &self.yandex_folder_id)]
        {
            if !v.is_empty() && !is_voice_id(v, 64) {
                return Err(invalid(field, "at most 64 chars of [A-Za-z0-9._-]"));
            }
        }
        let r = &self.azure_region;
        if !r.is_empty() && !(r.len() <= 32 && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())) {
            return Err(invalid("engines.azureRegion", "a region name like `westeurope`"));
        }
        if self.windows_voice.as_deref().is_some_and(|v| v.trim().is_empty() || !printable(v, 100)) {
            return Err(invalid("engines.windowsVoice", "1..=100 printable characters"));
        }
        Ok(())
    }
}

/// A recorded consent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SttConsent {
    /// Engine the consent was given for (switching engines needs a new consent).
    pub engine: SttEngine,
    /// When it was given.
    pub granted_at: Timestamp,
}

/// Wake-word settings (local detection only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct WakeWordConfig {
    /// Listen for the wake word (off by default, ADR-0017).
    pub enabled: bool,
    /// The phrase the user enrolled (display + matching hint), e.g. `Jarvis` / `Оркестр`.
    pub phrase: String,
    /// Sensitivity 0..=100 (higher = more accepts, more false accepts).
    pub sensitivity: u8,
    /// Average CPU budget of the detector in percent of one core (1..=10).
    pub cpu_budget_pct: u8,
    /// Pause listening in Battery Saver mode (push-to-talk keeps working).
    pub pause_on_battery_saver: bool,
}

impl Default for WakeWordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            phrase: "Jarvis".into(),
            sensitivity: 50,
            cpu_budget_pct: 2,
            pause_on_battery_saver: true,
        }
    }
}

/// Quiet hours (local time, `HH:MM`, may wrap past midnight).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct QuietHours {
    /// Enabled.
    pub enabled: bool,
    /// Start, `HH:MM`.
    pub start: String,
    /// End, `HH:MM`.
    pub end: String,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self { enabled: false, start: "22:00".into(), end: "08:00".into() }
    }
}

/// Spoken notifications for key events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
#[allow(clippy::struct_excessive_bools)]
pub struct VoiceNotificationConfig {
    /// Speak notifications at all.
    pub enabled: bool,
    /// Quiet hours.
    pub quiet_hours: QuietHours,
    /// A task completed.
    pub task_completed: bool,
    /// A task failed / an agent hit an error.
    pub task_failed: bool,
    /// An approval is needed.
    pub approval_needed: bool,
    /// A build failed.
    pub build_failed: bool,
    /// A budget threshold was crossed.
    pub budget_threshold: bool,
    /// A deployment finished.
    pub deploy_finished: bool,
}

impl Default for VoiceNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quiet_hours: QuietHours::default(),
            task_completed: true,
            task_failed: true,
            approval_needed: true,
            build_failed: true,
            budget_threshold: true,
            deploy_finished: true,
        }
    }
}

/// Sound preset of the procedural sound engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SoundPreset {
    /// Follow the active theme.
    #[default]
    Auto,
    /// Soft glassy chimes.
    Nebula,
    /// Bright bell tones.
    Crystal,
    /// Synthetic pulses.
    Pulse,
    /// Short, quiet clicks.
    Minimal,
}

/// Sound engine settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SoundConfig {
    /// Play interface sounds.
    pub enabled: bool,
    /// Master volume 0..=100.
    pub volume_pct: u8,
    /// Preset.
    pub preset: SoundPreset,
    /// Mute while Do Not Disturb is on (critical alerts still play).
    pub mute_in_dnd: bool,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self { enabled: true, volume_pct: 55, preset: SoundPreset::Auto, mute_in_dnd: true }
    }
}

/// The whole voice configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
#[allow(clippy::struct_excessive_bools)]
pub struct VoiceConfig {
    /// Voice commands on/off (push-to-talk, HUD, wake word).
    pub enabled: bool,
    /// Speech-to-text engine.
    pub stt_engine: SttEngine,
    /// Consent for off-device speech recognition (server-owned).
    pub stt_consent: Option<SttConsent>,
    /// Text-to-speech engine.
    pub tts_engine: TtsEngine,
    /// Consent for cloud text-to-speech (server-owned).
    pub tts_consent_at: Option<Timestamp>,
    /// OpenAI provider used for cloud STT/TTS (default: the first OpenAI provider with a key).
    pub cloud_provider_id: Option<ProviderId>,
    /// Recognition / reply language: `auto` (UI language) or a BCP-47 tag.
    pub language: String,
    /// Preferred input device id (`None` = system default).
    pub input_device_id: Option<String>,
    /// Push-to-talk hotkey (hold), e.g. `Ctrl+Space`.
    pub push_to_talk_hotkey: String,
    /// Speak short replies to voice commands.
    pub spoken_replies: bool,
    /// System (`speechSynthesis`) voice name for the assistant (`None` = best match for the language).
    pub system_voice: Option<String>,
    /// OpenAI TTS voice for the assistant.
    pub cloud_voice: String,
    /// Speaking rate, percent (50..=200).
    pub rate_pct: u16,
    /// Pitch, percent (50..=200).
    pub pitch_pct: u16,
    /// Wake word.
    pub wake: WakeWordConfig,
    /// Spoken notifications.
    pub notifications: VoiceNotificationConfig,
    /// Sound engine.
    pub sound: SoundConfig,
    /// Voice per agent: template id → voice profile. Overrides the
    /// template's own `voice` (built-in templates are read-only, so the
    /// user's choice lives here). Phase 3, additive.
    pub agent_voices: BTreeMap<String, VoiceProfile>,
    /// STT engines tried, in order, when the selected one fails (ADR-0046).
    pub stt_fallback: Vec<SttEngine>,
    /// TTS engines tried, in order, when the selected one fails. The client
    /// falls back to the system voice last.
    pub tts_fallback: Vec<TtsEngine>,
    /// Per-language STT default: primary language subtag (`ru`) → engine.
    pub stt_by_language: BTreeMap<String, SttEngine>,
    /// Per-language TTS default: primary language subtag (`ru`) → engine.
    pub tts_by_language: BTreeMap<String, TtsEngine>,
    /// Consents for off-device STT engines (server-owned; `sttConsent` is the legacy single slot).
    pub stt_consents: Vec<SttConsent>,
    /// Consents for cloud TTS engines (server-owned; `ttsConsentAt` is the legacy OpenAI slot).
    pub tts_consents: Vec<TtsConsent>,
    /// Per-engine options.
    pub engines: VoiceEngineSettings,
}

/// Max entries in `VoiceConfig::agent_voices`.
pub const MAX_AGENT_VOICES: usize = 256;
/// Max length of a fallback chain.
pub const MAX_FALLBACK: usize = 8;
/// Max entries of a per-language engine map.
pub const MAX_LANGUAGE_DEFAULTS: usize = 16;

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stt_engine: SttEngine::Disabled,
            stt_consent: None,
            tts_engine: TtsEngine::System,
            tts_consent_at: None,
            cloud_provider_id: None,
            language: "auto".into(),
            input_device_id: None,
            push_to_talk_hotkey: "Ctrl+Space".into(),
            spoken_replies: true,
            system_voice: None,
            cloud_voice: "alloy".into(),
            rate_pct: 100,
            pitch_pct: 100,
            wake: WakeWordConfig::default(),
            notifications: VoiceNotificationConfig::default(),
            sound: SoundConfig::default(),
            agent_voices: BTreeMap::new(),
            stt_fallback: Vec::new(),
            tts_fallback: Vec::new(),
            stt_by_language: BTreeMap::new(),
            tts_by_language: BTreeMap::new(),
            stt_consents: Vec::new(),
            tts_consents: Vec::new(),
            engines: VoiceEngineSettings::default(),
        }
    }
}

/// Primary language subtag of a BCP-47 tag (`ru-RU` → `ru`), lower-case.
#[must_use]
pub fn primary_language(tag: &str) -> String {
    tag.split(['-', '_']).next().unwrap_or(tag).to_ascii_lowercase()
}

/// Validation failure of a voice config.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct VoiceConfigError {
    /// camelCase path.
    pub field: &'static str,
    /// What is wrong.
    pub message: String,
}

fn invalid(field: &'static str, message: impl Into<String>) -> VoiceConfigError {
    VoiceConfigError { field, message: message.into() }
}

/// Whether `s` is `HH:MM` (00..=23, 00..=59).
#[must_use]
pub fn is_clock_time(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' || !b.iter().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit()) {
        return false;
    }
    let h = u32::from(b[0] - b'0') * 10 + u32::from(b[1] - b'0');
    let m = u32::from(b[3] - b'0') * 10 + u32::from(b[4] - b'0');
    h < 24 && m < 60
}

fn is_language(s: &str) -> bool {
    if s == "auto" {
        return true;
    }
    let mut parts = s.split('-');
    let primary_ok =
        parts.next().is_some_and(|p| (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic()));
    primary_ok && parts.all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn printable(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

/// Validates a voice profile used for an agent (`provider` is a
/// [`TtsEngine::profile_provider`] string: `system`, `openai`, `piper`, …).
pub fn validate_profile(p: &VoiceProfile) -> Result<(), VoiceConfigError> {
    if TtsEngine::from_profile_provider(&p.provider).is_none() {
        return Err(invalid("agentVoices.provider", "expected an engine like `system`, `piper` or `openai`"));
    }
    if !printable(&p.voice, 200) {
        return Err(invalid("agentVoices.voice", "at most 200 printable characters"));
    }
    if !is_language(&p.language) {
        return Err(invalid("agentVoices.language", "expected `auto` or a language tag like `ru` or `en-US`"));
    }
    if p.style.as_deref().is_some_and(|s| !printable(s, 200)) {
        return Err(invalid("agentVoices.style", "at most 200 printable characters"));
    }
    if p.rate_pct.is_some_and(|r| !(50..=200).contains(&r)) || p.pitch_pct.is_some_and(|r| !(50..=200).contains(&r)) {
        return Err(invalid("agentVoices.ratePct", "rate and pitch must be between 50 and 200"));
    }
    Ok(())
}

impl VoiceConfig {
    /// Validates every field (consent fields are not checked here — the core owns them).
    pub fn validate(&self) -> Result<(), VoiceConfigError> {
        if !is_language(&self.language) {
            return Err(invalid("language", "expected `auto` or a language tag like `ru` or `en-US`"));
        }
        if let Some(d) = &self.input_device_id {
            if !printable(d, 256) {
                return Err(invalid("inputDeviceId", "at most 256 printable characters"));
            }
        }
        let hk = &self.push_to_talk_hotkey;
        if hk.is_empty() || !printable(hk, 40) {
            return Err(invalid("pushToTalkHotkey", "1..=40 printable characters"));
        }
        if let Some(v) = &self.system_voice {
            if !printable(v, 200) {
                return Err(invalid("systemVoice", "at most 200 printable characters"));
            }
        }
        let cv = &self.cloud_voice;
        if cv.is_empty() || cv.len() > 40 || !cv.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(invalid("cloudVoice", "1..=40 chars of [A-Za-z0-9_-]"));
        }
        if !(50..=200).contains(&self.rate_pct) {
            return Err(invalid("ratePct", "must be between 50 and 200"));
        }
        if !(50..=200).contains(&self.pitch_pct) {
            return Err(invalid("pitchPct", "must be between 50 and 200"));
        }
        let w = &self.wake;
        if w.phrase.trim().is_empty() || !printable(&w.phrase, 40) {
            return Err(invalid("wake.phrase", "1..=40 printable characters"));
        }
        if w.sensitivity > 100 {
            return Err(invalid("wake.sensitivity", "must be between 0 and 100"));
        }
        if !(1..=10).contains(&w.cpu_budget_pct) {
            return Err(invalid("wake.cpuBudgetPct", "must be between 1 and 10"));
        }
        let q = &self.notifications.quiet_hours;
        if !is_clock_time(&q.start) {
            return Err(invalid("notifications.quietHours.start", "expected HH:MM"));
        }
        if !is_clock_time(&q.end) {
            return Err(invalid("notifications.quietHours.end", "expected HH:MM"));
        }
        if self.sound.volume_pct > 100 {
            return Err(invalid("sound.volumePct", "must be between 0 and 100"));
        }
        if self.agent_voices.len() > MAX_AGENT_VOICES {
            return Err(invalid("agentVoices", format!("at most {MAX_AGENT_VOICES} entries")));
        }
        for (template_id, p) in &self.agent_voices {
            if template_id.is_empty() || !printable(template_id, 64) {
                return Err(invalid("agentVoices", "template ids are 1..=64 printable characters"));
            }
            validate_profile(p)?;
        }
        if self.stt_fallback.len() > MAX_FALLBACK || self.tts_fallback.len() > MAX_FALLBACK {
            return Err(invalid("sttFallback", format!("at most {MAX_FALLBACK} fallback engines")));
        }
        if self.stt_by_language.len() > MAX_LANGUAGE_DEFAULTS || self.tts_by_language.len() > MAX_LANGUAGE_DEFAULTS {
            return Err(invalid("sttByLanguage", format!("at most {MAX_LANGUAGE_DEFAULTS} languages")));
        }
        for lang in self.stt_by_language.keys().chain(self.tts_by_language.keys()) {
            if lang == "auto" || lang.contains('-') || !is_language(lang) || lang != &lang.to_ascii_lowercase() {
                return Err(invalid("sttByLanguage", "keys are lower-case primary language subtags like `ru`"));
            }
        }
        if self.stt_consents.len() > 32 || self.tts_consents.len() > 32 {
            return Err(invalid("sttConsents", "too many consent records"));
        }
        self.engines.validate()
    }

    /// Whether `engine` may receive audio (local engines always may).
    #[must_use]
    pub fn stt_engine_consented(&self, engine: SttEngine) -> bool {
        !engine.needs_consent()
            || self.stt_consent.as_ref().is_some_and(|c| c.engine == engine)
            || self.stt_consents.iter().any(|c| c.engine == engine)
    }

    /// Whether `engine` may receive text (local engines always may; the
    /// legacy `ttsConsentAt` slot covers OpenAI).
    #[must_use]
    pub fn tts_engine_consented(&self, engine: TtsEngine) -> bool {
        !engine.is_cloud()
            || (engine == TtsEngine::OpenAi && self.tts_consent_at.is_some())
            || self.tts_consents.iter().any(|c| c.engine == engine)
    }

    /// Every STT engine the configuration can route audio to.
    #[must_use]
    pub fn stt_engines_in_use(&self) -> Vec<SttEngine> {
        let mut v = vec![self.stt_engine];
        v.extend(self.stt_by_language.values().copied());
        v.extend(self.stt_fallback.iter().copied());
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Every TTS engine the configuration can route text to.
    #[must_use]
    pub fn tts_engines_in_use(&self) -> Vec<TtsEngine> {
        let mut v = vec![self.tts_engine];
        v.extend(self.tts_by_language.values().copied());
        v.extend(self.tts_fallback.iter().copied());
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The STT engine for `language` (per-language default, else the selected one).
    #[must_use]
    pub fn stt_engine_for(&self, language: Option<&str>) -> SttEngine {
        language.map(primary_language).and_then(|l| self.stt_by_language.get(&l).copied()).unwrap_or(self.stt_engine)
    }

    /// The TTS engine for `language` (per-language default, else the selected one).
    #[must_use]
    pub fn tts_engine_for(&self, language: Option<&str>) -> TtsEngine {
        language.map(primary_language).and_then(|l| self.tts_by_language.get(&l).copied()).unwrap_or(self.tts_engine)
    }

    /// Engines the core tries for `language`, in order: the language
    /// default (or the selected engine), then the fallback chain. Engines the
    /// core cannot run (`Disabled`, `WebSpeech`) are skipped.
    #[must_use]
    pub fn stt_chain(&self, language: Option<&str>) -> Vec<SttEngine> {
        let mut out: Vec<SttEngine> = Vec::new();
        for e in std::iter::once(self.stt_engine_for(language)).chain(self.stt_fallback.iter().copied()) {
            if e.runs_in_core() && !out.contains(&e) {
                out.push(e);
            }
        }
        out
    }

    /// Engines the core tries for `language` (see [`Self::stt_chain`]); `System` is skipped.
    #[must_use]
    pub fn tts_chain(&self, language: Option<&str>) -> Vec<TtsEngine> {
        let mut out: Vec<TtsEngine> = Vec::new();
        for e in std::iter::once(self.tts_engine_for(language)).chain(self.tts_fallback.iter().copied()) {
            if e.runs_in_core() && !out.contains(&e) {
                out.push(e);
            }
        }
        out
    }

    /// Whether every STT engine in use (selected, per-language, fallback) has a matching consent.
    #[must_use]
    pub fn stt_consented(&self) -> bool {
        self.stt_engines_in_use().into_iter().all(|e| self.stt_engine_consented(e))
    }

    /// Whether every TTS engine in use has a matching consent.
    #[must_use]
    pub fn tts_all_consented(&self) -> bool {
        self.tts_engines_in_use().into_iter().all(|e| self.tts_engine_consented(e))
    }

    /// Whether cloud TTS is allowed.
    #[must_use]
    pub fn tts_consented(&self) -> bool {
        self.tts_consent_at.is_some()
    }
}

/// An OpenAI provider usable for cloud STT/TTS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceCloudProvider {
    /// Provider id.
    pub id: ProviderId,
    /// Display name.
    pub name: String,
    /// Whether an API key is stored.
    pub has_credential: bool,
}

/// Result of `voice.getConfig` / `voice.setConfig` / `voice.setConsent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceConfigResult {
    /// Current configuration.
    pub config: VoiceConfig,
    /// Providers that can serve cloud STT/TTS.
    pub cloud_providers: Vec<VoiceCloudProvider>,
}

/// Params of `voice.setConfig`. Consent fields in `config` are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetVoiceConfigParams {
    /// New configuration.
    pub config: VoiceConfig,
}

/// What a consent is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceConsentScope {
    /// Off-device speech recognition with `engine`.
    SpeechRecognition,
    /// Cloud text-to-speech.
    CloudSpeech,
}

/// Params of `voice.setConsent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetVoiceConsentParams {
    /// Scope.
    pub scope: VoiceConsentScope,
    /// STT engine (required for `SpeechRecognition` grants).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub engine: Option<SttEngine>,
    /// Grant (`true`) or revoke (`false`). Revoking switches back to a local engine.
    pub granted: bool,
    /// Cloud TTS engine for `CloudSpeech` (default `OpenAi`). Additive (ADR-0046).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tts_engine: Option<TtsEngine>,
    /// Grant without selecting the engine as the primary one (for fallback
    /// chains, per-language defaults and engine tests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep_selection: Option<bool>,
}

/// Max decoded audio size for `voice.transcribe` (8 MiB ≈ 4 min of 16 kHz mono PCM).
pub const MAX_TRANSCRIBE_BYTES: usize = 8 * 1024 * 1024;
/// Max text length for `voice.synthesize`.
pub const MAX_SYNTHESIZE_CHARS: usize = 1000;

/// Params of `voice.transcribe` (cloud STT; only with consent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscribeParams {
    /// Base64 audio (WAV recommended).
    pub audio_base64: String,
    /// MIME type, e.g. `audio/wav`.
    pub mime_type: String,
    /// Language hint (BCP-47 primary tag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Use exactly this engine (engine test); default: the configured chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub engine: Option<SttEngine>,
}

/// Result of `voice.transcribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscribeResult {
    /// Recognized text.
    pub text: String,
    /// Detected language when the service reports it.
    pub language: Option<String>,
    /// Engine that produced the text.
    #[serde(default)]
    pub engine: Option<SttEngine>,
    /// Wall time of the recognition in milliseconds.
    #[serde(default)]
    pub elapsed_ms: u64,
}

/// Params of `voice.synthesize` (cloud TTS; only with consent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SynthesizeParams {
    /// Text to speak (≤ 1000 chars).
    pub text: String,
    /// Voice id (`alloy`, …); default: the configured cloud voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub voice: Option<String>,
    /// Style instructions (`calm`, `cheerful`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub style: Option<String>,
    /// Use exactly this engine (engine test, agent voice); default: the configured chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub engine: Option<TtsEngine>,
    /// Language of the text (BCP-47); default: the configured language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
}

/// Result of `voice.synthesize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SynthesizeResult {
    /// Base64 audio.
    pub audio_base64: String,
    /// MIME type (`audio/mpeg`, `audio/wav`).
    pub mime_type: String,
    /// Engine that rendered the audio.
    #[serde(default)]
    pub engine: Option<TtsEngine>,
}

// --- engines + model manager (ADR-0046) ---------------------------------------

/// Readiness of an engine on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceEngineStatus {
    /// Usable now.
    Ready,
    /// A runtime or model package must be downloaded first.
    NeedsDownload,
    /// An API key (and, for some services, a region/folder) must be configured.
    NeedsKey,
    /// Not available on this machine (OS, missing language pack, missing runtime).
    Unavailable,
    /// Runs in the webview (WebSpeech, system voices); the core cannot check it.
    Client,
}

/// How an engine integration was verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceEngineVerification {
    /// End-to-end tests run against the real engine.
    Tested,
    /// Request/response shapes are covered by tests against a mock server;
    /// no live call without a user key.
    RequestShapeVerified,
    /// Not verified on this machine (missing runtime or language pack).
    Untested,
}

/// Coarse quality / speed grade for engine cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceGrade {
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Highest.
    VeryHigh,
}

/// A voice or model an engine can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceEngineOption {
    /// Id stored in the settings (package id, voice name, model name).
    pub id: String,
    /// Display label.
    pub label: String,
    /// Language tag (`ru-RU`; `*` = multilingual).
    pub language: String,
    /// Installed / usable now.
    pub available: bool,
}

/// One engine with its capabilities and readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[allow(clippy::struct_excessive_bools)]
pub struct VoiceEngineInfo {
    /// STT engine id (for STT entries).
    pub stt: Option<SttEngine>,
    /// TTS engine id (for TTS entries).
    pub tts: Option<TtsEngine>,
    /// Product name.
    pub name: String,
    /// Runs without network access.
    pub offline: bool,
    /// Sends audio/text to a cloud service.
    pub cloud: bool,
    /// Supports streaming (interim results / incremental audio) in principle.
    pub streaming: bool,
    /// Languages (primary subtags; `*` = multilingual).
    pub languages: Vec<String>,
    /// Supports Russian.
    pub russian: bool,
    /// Recognition / voice quality.
    pub quality: VoiceGrade,
    /// Speed (higher = lower latency).
    pub speed: VoiceGrade,
    /// Readiness.
    pub status: VoiceEngineStatus,
    /// Technical detail for `status` (English, e.g. "no ru-RU recognizer").
    pub detail: Option<String>,
    /// Packages this engine can use (runtime + models/voices).
    pub packages: Vec<String>,
    /// API key slot.
    pub key: Option<VoiceKeyId>,
    /// Whether that key is stored.
    pub key_configured: bool,
    /// Needs consent (cloud).
    pub needs_consent: bool,
    /// Consent recorded.
    pub consented: bool,
    /// Verification level.
    pub verification: VoiceEngineVerification,
    /// Selectable voices/models.
    pub options: Vec<VoiceEngineOption>,
}

/// Params of `voice.listEngines`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListVoiceEnginesParams {
    /// Re-run OS detection (Windows voices/recognizers) instead of using the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `voice.listEngines` / `voice.setEngineKey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceEnginesResult {
    /// Speech-to-text engines.
    pub stt: Vec<VoiceEngineInfo>,
    /// Text-to-speech engines.
    pub tts: Vec<VoiceEngineInfo>,
    /// Key slots that hold a key.
    pub configured_keys: Vec<VoiceKeyId>,
}

/// Params of `voice.setEngineKey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetVoiceEngineKeyParams {
    /// Key slot.
    pub key: VoiceKeyId,
    /// The key (stored in the secret vault); `None`/empty clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub api_key: Option<String>,
}

/// Kind of a downloadable voice package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoicePackageKind {
    /// Engine binaries.
    Runtime,
    /// A speech recognition model.
    SttModel,
    /// A synthesis voice.
    TtsVoice,
}

/// Install state of a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoicePackageState {
    /// Not on disk.
    NotInstalled,
    /// Waiting for another download.
    Queued,
    /// Downloading.
    Downloading,
    /// Checking SHA-256 / unpacking.
    Installing,
    /// Installed and verified.
    Installed,
    /// The last download failed (`error`).
    Failed,
}

/// A downloadable runtime / model / voice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoicePackageInfo {
    /// Package id (`whisper-base`, `piper-ru-irina`, …).
    pub id: String,
    /// Display title.
    pub title: String,
    /// Kind.
    pub kind: VoicePackageKind,
    /// STT engine it belongs to.
    pub stt: Option<SttEngine>,
    /// TTS engine it belongs to.
    pub tts: Option<TtsEngine>,
    /// Languages (`*` = multilingual).
    pub languages: Vec<String>,
    /// Download size in bytes.
    pub download_bytes: u64,
    /// Size on disk when installed.
    pub installed_bytes: u64,
    /// State.
    pub state: VoicePackageState,
    /// Downloaded bytes of the running download.
    pub progress_bytes: u64,
    /// Last error.
    pub error: Option<String>,
    /// Suggested for Russian out of the box.
    pub recommended: bool,
    /// Packages installed together with this one.
    pub depends_on: Vec<String>,
    /// Pinned SHA-256 of each downloaded file.
    pub sha256: Vec<String>,
    /// Available for this OS/CPU.
    pub supported: bool,
    /// License (SPDX-like).
    pub license: String,
    /// Download host.
    pub source: String,
}

/// Result of the `voice.*Model` methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModelsResult {
    /// Catalog with state.
    pub packages: Vec<VoicePackageInfo>,
    /// Bytes used under the voice directory.
    pub disk_usage_bytes: u64,
    /// The voice directory (`<ORKESTR_HOME>/voice/models`).
    pub root: String,
}

/// Params naming one package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoicePackageParams {
    /// Package id.
    pub id: String,
}

/// Result of `voice.verifyModel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceVerifyResult {
    /// All files match the manifest written at install time.
    pub ok: bool,
    /// Files checked.
    pub files: u32,
    /// Relative paths that are missing or changed.
    pub mismatches: Vec<String>,
}

/// How a voice command was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VoiceCommandSource {
    /// Push-to-talk hotkey or HUD mic button.
    PushToTalk,
    /// Local wake word.
    WakeWord,
    /// Typed into the voice HUD.
    Typed,
}

/// Params of `voice.recordCommand` (audit trail: transcript only, never audio).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordVoiceCommandParams {
    /// What the user said (≤ 500 chars).
    pub transcript: String,
    /// Parsed intent id (`stopProject`, `askManager`, …).
    pub intent: String,
    /// Outcome code (`ok`, `failed`, `cancelled`, `needsConfirmation`, `sentToManager`).
    pub outcome: String,
    /// Source.
    pub source: VoiceCommandSource,
    /// Project the command targeted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

method!(
    /// `voice.getConfig`.
    GetConfig, "voice.getConfig", super::Empty, VoiceConfigResult
);
method!(
    /// `voice.setConfig`.
    SetConfig, "voice.setConfig", SetVoiceConfigParams, VoiceConfigResult
);
method!(
    /// `voice.setConsent`.
    SetConsent, "voice.setConsent", SetVoiceConsentParams, VoiceConfigResult
);
method!(
    /// `voice.transcribe`.
    Transcribe, "voice.transcribe", TranscribeParams, TranscribeResult
);
method!(
    /// `voice.synthesize`.
    Synthesize, "voice.synthesize", SynthesizeParams, SynthesizeResult
);
method!(
    /// `voice.recordCommand`.
    RecordCommand, "voice.recordCommand", RecordVoiceCommandParams, super::Empty
);
method!(
    /// `voice.listEngines` (ADR-0046).
    ListEngines, "voice.listEngines", ListVoiceEnginesParams, VoiceEnginesResult
);
method!(
    /// `voice.setEngineKey`: stores/clears a paid engine's API key in the secret vault.
    SetEngineKey, "voice.setEngineKey", SetVoiceEngineKeyParams, VoiceEnginesResult
);
method!(
    /// `voice.listModels`.
    ListModels, "voice.listModels", super::Empty, VoiceModelsResult
);
method!(
    /// `voice.downloadModel`: starts a download (with dependencies).
    DownloadModel, "voice.downloadModel", VoicePackageParams, VoiceModelsResult
);
method!(
    /// `voice.cancelDownload`.
    CancelDownload, "voice.cancelDownload", VoicePackageParams, VoiceModelsResult
);
method!(
    /// `voice.removeModel`.
    RemoveModel, "voice.removeModel", VoicePackageParams, VoiceModelsResult
);
method!(
    /// `voice.verifyModel`: re-hashes installed files against the install manifest.
    VerifyModel, "voice.verifyModel", VoicePackageParams, VoiceVerifyResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_private_and_valid() {
        let c = VoiceConfig::default();
        c.validate().unwrap();
        assert_eq!(c.stt_engine, SttEngine::Disabled, "no audio leaves the device by default");
        assert!(!c.wake.enabled, "wake word is off by default (ADR-0017)");
        assert!(c.stt_consented());
        assert!(!c.tts_consented());
    }

    #[test]
    fn missing_and_unknown_fields_default() {
        let c: VoiceConfig =
            serde_json::from_str(r#"{"wake": {"enabled": true}, "futureField": 1, "sound": {"volumePct": 10}}"#)
                .unwrap();
        assert!(c.wake.enabled);
        assert_eq!(c.wake.phrase, "Jarvis");
        assert_eq!(c.sound.volume_pct, 10);
        assert!(c.sound.enabled);
        assert_eq!(c.push_to_talk_hotkey, "Ctrl+Space");
    }

    #[test]
    fn consent_must_match_engine() {
        let now = chrono::Utc::now();
        let mut c = VoiceConfig { stt_engine: SttEngine::OpenAiWhisper, ..VoiceConfig::default() };
        assert!(!c.stt_consented());
        c.stt_consent = Some(SttConsent { engine: SttEngine::WebSpeech, granted_at: now });
        assert!(!c.stt_consented(), "consent for another engine does not count");
        c.stt_consent = Some(SttConsent { engine: SttEngine::OpenAiWhisper, granted_at: now });
        assert!(c.stt_consented());
    }

    #[test]
    fn validation_rejects_bad_values() {
        assert!(is_clock_time("00:00") && is_clock_time("23:59"));
        assert!(
            !is_clock_time("24:00") && !is_clock_time("7:00") && !is_clock_time("12:60") && !is_clock_time("ab:cd")
        );
        let bad = |f: &dyn Fn(&mut VoiceConfig)| {
            let mut c = VoiceConfig::default();
            f(&mut c);
            c.validate().is_err()
        };
        assert!(bad(&|c| c.language = "Russian".into()));
        assert!(bad(&|c| c.rate_pct = 10));
        assert!(bad(&|c| c.pitch_pct = 500));
        assert!(bad(&|c| c.wake.sensitivity = 101));
        assert!(bad(&|c| c.wake.cpu_budget_pct = 0));
        assert!(bad(&|c| c.wake.phrase = "  ".into()));
        assert!(bad(&|c| c.notifications.quiet_hours.end = "25:00".into()));
        assert!(bad(&|c| c.sound.volume_pct = 200));
        assert!(bad(&|c| c.cloud_voice = "a b".into()));
        assert!(bad(&|c| c.push_to_talk_hotkey = String::new()));
        let profile = |provider: &str, rate: Option<u16>| VoiceProfile {
            provider: provider.into(),
            voice: "Microsoft Irina".into(),
            language: "ru-RU".into(),
            style: None,
            rate_pct: rate,
            pitch_pct: None,
        };
        assert!(bad(&|c| {
            c.agent_voices.insert("t1".into(), profile("bogus", None));
        }));
        assert!(bad(&|c| {
            c.agent_voices.insert("t1".into(), profile("system", Some(20)));
        }));
        assert!(bad(&|c| {
            c.agent_voices.insert(String::new(), profile("system", None));
        }));
        let mut ok = VoiceConfig::default();
        ok.agent_voices.insert("t1".into(), profile("system", Some(120)));
        ok.agent_voices.insert("t2".into(), profile("openai", None));
        ok.language = "ru".into();
        ok.wake.phrase = "Оркестр".into();
        ok.validate().unwrap();
    }
}
