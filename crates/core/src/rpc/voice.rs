//! `voice.*` handlers (§33 Voice Command Center, ADR-0017).
//!
//! - The voice configuration is one JSON document in `core_settings` under
//!   the key [`VOICE_KEY`] (the settings store ignores keys it does not know,
//!   so no migration is needed). Missing/invalid documents load as defaults.
//! - Consent (off-device STT, cloud TTS) is **server-owned**: only
//!   `voice.setConsent` writes it (and audits it); `voice.setConfig` keeps the
//!   stored consent and rejects an engine that has none (`forbidden`).
//! - `voice.transcribe` / `voice.synthesize` run only with the matching
//!   consent and go through the user's OpenAI(-compatible) provider. Audio is
//!   never persisted or logged; buffers are dropped when the call returns.
//! - `voice.recordCommand` publishes a `VoiceCommand` event (transcript +
//!   intent, never audio).

use base64::Engine as _;
use orkestr_domain::{AuditOutcome, ProviderKind};
use orkestr_protocol::event::{EventKind, VoiceCommandPayload};
use orkestr_protocol::methods::voice::{
    self as m, RecordVoiceCommandParams, SetVoiceConfigParams, SetVoiceConsentParams, SttConsent, SttEngine,
    SynthesizeParams, SynthesizeResult, TranscribeParams, TranscribeResult, TtsEngine, VoiceCloudProvider, VoiceConfig,
    VoiceConfigResult, VoiceConsentScope, MAX_SYNTHESIZE_CHARS, MAX_TRANSCRIBE_BYTES,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_providers::audio::{SpeechRequest, TranscriptionRequest};
use orkestr_providers::ProviderView;
use orkestr_security::NewAudit;
use orkestr_storage::rusqlite::{params, OptionalExtension};

use super::providers::provider_error_to_rpc;
use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};

/// `core_settings` key of the voice document.
pub const VOICE_KEY: &str = "voice";
/// `core_settings` key: the provider cloud speech recognition consent is bound to.
const STT_PROVIDER_KEY: &str = "voice.sttConsentProvider";
/// `core_settings` key: the provider cloud text-to-speech consent is bound to.
const TTS_PROVIDER_KEY: &str = "voice.ttsConsentProvider";

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::GetConfig, _, _>(|ctx: RpcContext, _: Empty| get_config(ctx))
        .method::<m::SetConfig, _, _>(set_config)
        .method::<m::SetConsent, _, _>(set_consent)
        .method::<m::Transcribe, _, _>(transcribe)
        .method::<m::Synthesize, _, _>(synthesize)
        .method::<m::RecordCommand, _, _>(record_command);
}

/// Serializes read-modify-write cycles of the voice document.
fn write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Parses a stored document; anything unreadable or invalid → defaults.
#[must_use]
pub fn parse_stored(raw: Option<&str>) -> VoiceConfig {
    let Some(raw) = raw else { return VoiceConfig::default() };
    match serde_json::from_str::<VoiceConfig>(raw) {
        Ok(c) if c.validate().is_ok() => c,
        Ok(_) | Err(_) => {
            tracing::warn!("ignoring invalid stored voice configuration");
            VoiceConfig::default()
        }
    }
}

async fn load(ctx: &RpcContext) -> Result<VoiceConfig, RpcError> {
    let raw: Option<String> = ctx
        .services
        .db
        .call(|c| {
            Ok(c.query_row("SELECT value FROM core_settings WHERE key = ?1", params![VOICE_KEY], |r| r.get(0))
                .optional()?)
        })
        .await
        .map_err(storage_to_rpc)?;
    Ok(parse_stored(raw.as_deref()))
}

async fn load_setting(ctx: &RpcContext, key: &'static str) -> Result<Option<String>, RpcError> {
    ctx.services
        .db
        .call(move |c| {
            Ok(c.query_row("SELECT value FROM core_settings WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
        })
        .await
        .map_err(storage_to_rpc)
}

async fn store_setting(ctx: &RpcContext, key: &'static str, value: Option<String>) -> Result<(), RpcError> {
    let now = chrono::Utc::now().to_rfc3339();
    ctx.services
        .db
        .call(move |c| {
            match value {
                Some(v) => c.execute(
                    "INSERT INTO core_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![key, v, now],
                )?,
                None => c.execute("DELETE FROM core_settings WHERE key = ?1", params![key])?,
            };
            Ok(())
        })
        .await
        .map_err(storage_to_rpc)
}

/// The provider cloud speech goes to, checked against the provider the
/// consent was given for. Consent is bound to one provider: switching the
/// speech provider requires consenting again. A consent recorded before any
/// provider existed binds to the first provider actually used.
async fn consented_provider(
    ctx: &RpcContext,
    config: &VoiceConfig,
    key: &'static str,
) -> Result<ProviderView, RpcError> {
    let provider = pick_provider(ctx, config)?;
    let id = provider.provider.id.to_string();
    match load_setting(ctx, key).await? {
        None => store_setting(ctx, key, Some(id)).await?,
        Some(bound) if bound == id => {}
        Some(_) => {
            return Err(RpcError::forbidden(
                "cloud speech consent was given for a different speech provider; grant consent again (voice.setConsent)",
            ))
        }
    }
    Ok(provider)
}

async fn save(ctx: &RpcContext, config: &VoiceConfig) -> Result<(), RpcError> {
    let value =
        serde_json::to_string(config).map_err(|e| RpcError::internal(format!("serialize voice config: {e}")))?;
    let now = chrono::Utc::now().to_rfc3339();
    ctx.services
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO core_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![VOICE_KEY, value, now],
            )?;
            Ok(())
        })
        .await
        .map_err(storage_to_rpc)
}

fn speech_capable(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::OpenAi | ProviderKind::OpenAiCompatible)
}

fn cloud_providers(ctx: &RpcContext) -> Vec<ProviderView> {
    ctx.services.providers.list().into_iter().filter(|v| speech_capable(v.provider.kind)).collect()
}

fn result(ctx: &RpcContext, config: VoiceConfig) -> VoiceConfigResult {
    let cloud_providers = cloud_providers(ctx)
        .into_iter()
        .map(|v| VoiceCloudProvider { id: v.provider.id, name: v.provider.name, has_credential: v.has_credential })
        .collect();
    VoiceConfigResult { config, cloud_providers }
}

/// The provider for cloud speech: the configured one, else the first
/// OpenAI provider with a key, else the first OpenAI-compatible one.
/// Disabled providers never receive audio or text.
fn pick_provider(ctx: &RpcContext, config: &VoiceConfig) -> Result<ProviderView, RpcError> {
    let list = cloud_providers(ctx);
    if let Some(id) = config.cloud_provider_id {
        let v = list
            .into_iter()
            .find(|v| v.provider.id == id)
            .ok_or_else(|| RpcError::not_found("the selected speech provider no longer exists"))?;
        if !v.provider.enabled {
            return Err(RpcError::bad_request(
                "the selected speech provider is disabled; enable it in Settings → AI providers",
            ));
        }
        return Ok(v);
    }
    let mut ranked: Vec<ProviderView> = list.into_iter().filter(|v| v.provider.enabled).collect();
    ranked.sort_by_key(|v| (v.provider.kind != ProviderKind::OpenAi, !v.has_credential));
    ranked.into_iter().next().ok_or_else(|| {
        RpcError::bad_request("no OpenAI provider is configured; add one in Settings → AI providers for cloud speech")
    })
}

async fn get_config(ctx: RpcContext) -> Result<VoiceConfigResult, RpcError> {
    let config = load(&ctx).await?;
    Ok(result(&ctx, config))
}

async fn set_config(ctx: RpcContext, p: SetVoiceConfigParams) -> Result<VoiceConfigResult, RpcError> {
    p.config.validate().map_err(|e| RpcError::bad_request(e.to_string()))?;
    let _guard = write_lock().lock().await;
    let stored = load(&ctx).await?;
    let mut next = p.config;
    // Consent is server-owned.
    next.stt_consent = stored.stt_consent;
    next.tts_consent_at = stored.tts_consent_at;
    if !next.stt_consented() {
        return Err(RpcError::forbidden(format!(
            "speech recognition with {:?} sends audio off this device and needs consent first (voice.setConsent)",
            next.stt_engine
        )));
    }
    if next.tts_engine == TtsEngine::OpenAi && !next.tts_consented() {
        return Err(RpcError::forbidden("cloud text-to-speech needs consent first (voice.setConsent)"));
    }
    if let Some(id) = next.cloud_provider_id {
        match cloud_providers(&ctx).iter().find(|v| v.provider.id == id) {
            None => return Err(RpcError::bad_request("cloudProviderId is not an OpenAI(-compatible) provider")),
            Some(v) if !v.provider.enabled => {
                return Err(RpcError::bad_request("cloudProviderId names a disabled provider"))
            }
            Some(_) => {}
        }
    }
    save(&ctx, &next).await?;
    tracing::info!(stt = ?next.stt_engine, tts = ?next.tts_engine, wake = next.wake.enabled, "voice settings updated");
    Ok(result(&ctx, next))
}

async fn set_consent(ctx: RpcContext, p: SetVoiceConsentParams) -> Result<VoiceConfigResult, RpcError> {
    let _guard = write_lock().lock().await;
    let mut next = load(&ctx).await?;
    let now = chrono::Utc::now();
    let resource = match (p.scope, p.granted) {
        (VoiceConsentScope::SpeechRecognition, true) => {
            let engine = p.engine.ok_or_else(|| RpcError::bad_request("engine is required to grant consent"))?;
            if !engine.needs_consent() {
                return Err(RpcError::bad_request("this engine does not need consent"));
            }
            next.stt_consent = Some(SttConsent { engine, granted_at: now });
            next.stt_engine = engine;
            format!("voice:stt:{engine:?}")
        }
        (VoiceConsentScope::SpeechRecognition, false) => {
            next.stt_consent = None;
            next.stt_engine = SttEngine::Disabled;
            "voice:stt".to_owned()
        }
        (VoiceConsentScope::CloudSpeech, true) => {
            next.tts_consent_at = Some(now);
            next.tts_engine = TtsEngine::OpenAi;
            "voice:tts:OpenAi".to_owned()
        }
        (VoiceConsentScope::CloudSpeech, false) => {
            next.tts_consent_at = None;
            next.tts_engine = TtsEngine::System;
            "voice:tts".to_owned()
        }
    };
    save(&ctx, &next).await?;
    // Bind the consent to the provider that will receive the data.
    let key = match p.scope {
        VoiceConsentScope::SpeechRecognition => STT_PROVIDER_KEY,
        VoiceConsentScope::CloudSpeech => TTS_PROVIDER_KEY,
    };
    let bound = if p.granted { pick_provider(&ctx, &next).ok().map(|v| v.provider.id.to_string()) } else { None };
    store_setting(&ctx, key, bound).await?;
    let sec = &ctx.services.security;
    let action = if p.granted { "voice.consentGranted" } else { "voice.consentRevoked" };
    sec.audit.record(NewAudit::new(sec.user(), action, resource, AuditOutcome::Executed)).await;
    tracing::info!(scope = ?p.scope, granted = p.granted, "voice consent changed");
    Ok(result(&ctx, next))
}

async fn transcribe(ctx: RpcContext, p: TranscribeParams) -> Result<TranscribeResult, RpcError> {
    let config = load(&ctx).await?;
    if config.stt_engine != SttEngine::OpenAiWhisper || !config.stt_consented() {
        return Err(RpcError::forbidden("cloud speech recognition is not enabled with consent"));
    }
    if p.audio_base64.len() > MAX_TRANSCRIBE_BYTES / 3 * 4 + 4 {
        return Err(RpcError::bad_request("audio is too large (max 8 MiB)"));
    }
    let audio = base64::engine::general_purpose::STANDARD
        .decode(p.audio_base64.as_bytes())
        .map_err(|_| RpcError::bad_request("audioBase64 is not valid base64"))?;
    if audio.is_empty() {
        return Err(RpcError::bad_request("audio is empty"));
    }
    let language =
        p.language.or_else(|| Some(config.language.clone()).filter(|l| l != "auto")).filter(|l| l.len() <= 16);
    let provider = consented_provider(&ctx, &config, STT_PROVIDER_KEY).await?;
    let adapter = ctx
        .services
        .providers
        .adapter(provider.provider.id)
        .ok_or_else(|| RpcError::not_found("speech provider is not loaded"))?;
    let out = adapter
        .transcribe(TranscriptionRequest { audio, mime_type: p.mime_type, language, model: None })
        .await
        .map_err(|e| provider_error_to_rpc(&e))?;
    Ok(TranscribeResult { text: out.text, language: out.language })
}

async fn synthesize(ctx: RpcContext, p: SynthesizeParams) -> Result<SynthesizeResult, RpcError> {
    let config = load(&ctx).await?;
    if !config.tts_consented() {
        return Err(RpcError::forbidden("cloud text-to-speech is not enabled with consent"));
    }
    let text = p.text.trim();
    if text.is_empty() || text.chars().count() > MAX_SYNTHESIZE_CHARS {
        return Err(RpcError::bad_request(format!("text must be 1..={MAX_SYNTHESIZE_CHARS} characters")));
    }
    let voice = p.voice.filter(|v| !v.trim().is_empty()).unwrap_or_else(|| config.cloud_voice.clone());
    if voice.len() > 40 || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(RpcError::bad_request("invalid voice id"));
    }
    let style = p.style.filter(|s| s.chars().count() <= 200);
    let provider = consented_provider(&ctx, &config, TTS_PROVIDER_KEY).await?;
    let adapter = ctx
        .services
        .providers
        .adapter(provider.provider.id)
        .ok_or_else(|| RpcError::not_found("speech provider is not loaded"))?;
    let out = adapter
        .synthesize_speech(SpeechRequest { text: text.to_owned(), voice, style, model: None })
        .await
        .map_err(|e| provider_error_to_rpc(&e))?;
    Ok(SynthesizeResult {
        audio_base64: base64::engine::general_purpose::STANDARD.encode(&out.bytes),
        mime_type: out.mime_type,
    })
}

fn is_code(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

async fn record_command(ctx: RpcContext, p: RecordVoiceCommandParams) -> Result<Empty, RpcError> {
    if !is_code(&p.intent, 64) || !is_code(&p.outcome, 32) {
        return Err(RpcError::bad_request("intent/outcome must be short codes"));
    }
    let transcript: String = p.transcript.chars().filter(|c| !c.is_control()).take(500).collect();
    let mut ev = NewEvent::new(EventKind::VoiceCommand(VoiceCommandPayload {
        transcript,
        intent: p.intent,
        outcome: p.outcome,
        source: p.source,
    }));
    if let Some(pid) = p.project_id {
        ev = ev.project(pid);
    }
    ctx.services.events.publish(ev).await.map_err(events_to_rpc)?;
    Ok(Empty {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_documents_fall_back_to_defaults() {
        assert_eq!(parse_stored(None), VoiceConfig::default());
        assert_eq!(parse_stored(Some("not json")), VoiceConfig::default());
        assert_eq!(parse_stored(Some(r#"{"ratePct": 5}"#)), VoiceConfig::default(), "invalid values are not loaded");
        let c = parse_stored(Some(r#"{"wake": {"enabled": true, "phrase": "Оркестр"}}"#));
        assert!(c.wake.enabled);
        assert_eq!(c.wake.phrase, "Оркестр");
    }

    #[test]
    fn code_validation() {
        assert!(is_code("stopProject", 64));
        assert!(!is_code("", 64));
        assert!(!is_code("drop table;", 64));
    }
}
