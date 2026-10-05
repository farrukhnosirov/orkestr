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

use orkestr_domain::{ProjectId, ProviderId, Timestamp};

use super::method;

/// Speech-to-text engine (after push-to-talk or the wake word).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SttEngine {
    /// No speech recognition: the voice HUD accepts typed commands only.
    #[default]
    Disabled,
    /// The webview's Web Speech API (audio is processed by the browser/OS
    /// speech service, usually online). Needs consent.
    WebSpeech,
    /// OpenAI Whisper / transcription API through the user's OpenAI provider. Needs consent.
    OpenAiWhisper,
}

impl SttEngine {
    /// Whether audio leaves the device with this engine (consent required).
    #[must_use]
    pub fn needs_consent(self) -> bool {
        !matches!(self, SttEngine::Disabled)
    }
}

/// Text-to-speech engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TtsEngine {
    /// The OS voices through `speechSynthesis` (offline).
    #[default]
    System,
    /// OpenAI text-to-speech through the user's OpenAI provider (opt-in; sends text).
    OpenAi,
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
        Self { enabled: false, phrase: "Jarvis".into(), sensitivity: 50, cpu_budget_pct: 2, pause_on_battery_saver: true }
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
}

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
        }
    }
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
        Ok(())
    }

    /// Whether the selected STT engine has a matching consent.
    #[must_use]
    pub fn stt_consented(&self) -> bool {
        !self.stt_engine.needs_consent() || self.stt_consent.as_ref().is_some_and(|c| c.engine == self.stt_engine)
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
}

/// Result of `voice.synthesize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SynthesizeResult {
    /// Base64 audio.
    pub audio_base64: String,
    /// MIME type (`audio/mpeg`).
    pub mime_type: String,
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
            serde_json::from_str(r#"{"wake": {"enabled": true}, "futureField": 1, "sound": {"volumePct": 10}}"#).unwrap();
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
        assert!(!is_clock_time("24:00") && !is_clock_time("7:00") && !is_clock_time("12:60") && !is_clock_time("ab:cd"));
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
        let mut ok = VoiceConfig::default();
        ok.language = "ru".into();
        ok.wake.phrase = "Оркестр".into();
        ok.validate().unwrap();
    }
}
