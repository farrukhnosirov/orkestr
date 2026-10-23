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
//! - ADR-0046: `voice.transcribe` / `voice.synthesize` route through the
//!   configured engine chain (per-language default, then the fallback
//!   chain) run by `orkestr-voice`; local engines need no consent, cloud
//!   engines need a per-engine consent. Paid engines' API keys live in the
//!   secret vault (`voice.setEngineKey`), never in the config document.
//! - `voice.recordCommand` publishes a `VoiceCommand` event (transcript +
//!   intent, never audio).

use base64::Engine as _;
use orkestr_domain::{Actor, SecretScope};
use orkestr_domain::{AuditOutcome, ProviderKind};
use orkestr_protocol::event::{EventKind, VoiceCommandPayload};
use orkestr_protocol::methods::voice::{
    self as m, ListVoiceEnginesParams, RecordVoiceCommandParams, SetVoiceConfigParams, SetVoiceConsentParams,
    SetVoiceEngineKeyParams, SttConsent, SttEngine, SynthesizeParams, SynthesizeResult, TranscribeParams,
    TranscribeResult, TtsConsent, TtsEngine, VoiceCloudProvider, VoiceConfig, VoiceConfigResult, VoiceConsentScope,
    VoiceEngineStatus, VoiceEnginesResult, VoiceKeyId, VoiceModelsResult, VoicePackageParams, VoiceVerifyResult,
    MAX_SYNTHESIZE_CHARS, MAX_TRANSCRIBE_BYTES, PhraseReplyParams, PhraseReplyResult,
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
        .method::<m::RecordCommand, _, _>(record_command)
        .method::<m::Phrase, _, _>(phrase)
        .method::<m::ListEngines, _, _>(list_engines)
        .method::<m::SetEngineKey, _, _>(set_engine_key)
        .method::<m::ListModels, _, _>(|ctx: RpcContext, _: Empty| list_models(ctx))
        .method::<m::DownloadModel, _, _>(download_model)
        .method::<m::CancelDownload, _, _>(cancel_download)
        .method::<m::RemoveModel, _, _>(remove_model)
        .method::<m::VerifyModel, _, _>(verify_model);
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
        Some(_) => return Err(RpcError::forbidden(
            "cloud speech consent was given for a different speech provider; grant consent again (voice.setConsent)",
        )),
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
    next.stt_consents = stored.stt_consents;
    next.tts_consents = stored.tts_consents;
    if let Some(e) = next.stt_engines_in_use().into_iter().find(|e| !next.stt_engine_consented(*e)) {
        return Err(RpcError::forbidden(format!(
            "speech recognition with {e:?} sends audio off this device and needs consent first (voice.setConsent)"
        )));
    }
    if let Some(e) = next.tts_engines_in_use().into_iter().find(|e| !next.tts_engine_consented(*e)) {
        return Err(RpcError::forbidden(format!(
            "cloud text-to-speech with {e:?} needs consent first (voice.setConsent)"
        )));
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

/// Drops STT engines that lost their consent from the selection.
fn prune_stt(c: &mut VoiceConfig) {
    let keep = |c: &VoiceConfig, e: SttEngine| c.stt_engine_consented(e);
    if !keep(c, c.stt_engine) {
        c.stt_engine = SttEngine::Disabled;
    }
    let snapshot = c.clone();
    c.stt_fallback.retain(|e| keep(&snapshot, *e));
    c.stt_by_language.retain(|_, e| keep(&snapshot, *e));
}

/// Drops TTS engines that lost their consent from the selection.
fn prune_tts(c: &mut VoiceConfig) {
    let keep = |c: &VoiceConfig, e: TtsEngine| c.tts_engine_consented(e);
    if !keep(c, c.tts_engine) {
        c.tts_engine = TtsEngine::System;
    }
    let snapshot = c.clone();
    c.tts_fallback.retain(|e| keep(&snapshot, *e));
    c.tts_by_language.retain(|_, e| keep(&snapshot, *e));
}

async fn set_consent(ctx: RpcContext, p: SetVoiceConsentParams) -> Result<VoiceConfigResult, RpcError> {
    let _guard = write_lock().lock().await;
    let mut next = load(&ctx).await?;
    let now = chrono::Utc::now();
    let select = !p.keep_selection.unwrap_or(false);
    let resource = match (p.scope, p.granted) {
        (VoiceConsentScope::SpeechRecognition, true) => {
            let engine = p.engine.ok_or_else(|| RpcError::bad_request("engine is required to grant consent"))?;
            if !engine.needs_consent() {
                return Err(RpcError::bad_request("this engine does not need consent"));
            }
            next.stt_consents.retain(|c| c.engine != engine);
            next.stt_consents.push(SttConsent { engine, granted_at: now });
            if select {
                next.stt_consent = Some(SttConsent { engine, granted_at: now });
                next.stt_engine = engine;
            }
            format!("voice:stt:{engine:?}")
        }
        (VoiceConsentScope::SpeechRecognition, false) => {
            match p.engine {
                Some(engine) => {
                    next.stt_consents.retain(|c| c.engine != engine);
                    if next.stt_consent.as_ref().is_some_and(|c| c.engine == engine) {
                        next.stt_consent = None;
                    }
                }
                None => {
                    next.stt_consents.clear();
                    next.stt_consent = None;
                }
            }
            prune_stt(&mut next);
            p.engine.map_or_else(|| "voice:stt".to_owned(), |e| format!("voice:stt:{e:?}"))
        }
        (VoiceConsentScope::CloudSpeech, true) => {
            let engine = p.tts_engine.unwrap_or(TtsEngine::OpenAi);
            if !engine.is_cloud() {
                return Err(RpcError::bad_request("this engine does not need consent"));
            }
            if engine == TtsEngine::OpenAi {
                next.tts_consent_at = Some(now);
            }
            next.tts_consents.retain(|c| c.engine != engine);
            next.tts_consents.push(TtsConsent { engine, granted_at: now });
            if select {
                next.tts_engine = engine;
            }
            format!("voice:tts:{engine:?}")
        }
        (VoiceConsentScope::CloudSpeech, false) => {
            match p.tts_engine {
                Some(engine) => {
                    next.tts_consents.retain(|c| c.engine != engine);
                    if engine == TtsEngine::OpenAi {
                        next.tts_consent_at = None;
                    }
                }
                None => {
                    next.tts_consents.clear();
                    next.tts_consent_at = None;
                }
            }
            prune_tts(&mut next);
            p.tts_engine.map_or_else(|| "voice:tts".to_owned(), |e| format!("voice:tts:{e:?}"))
        }
    };
    save(&ctx, &next).await?;
    // Bind the OpenAI-provider consent to the provider that will receive the data.
    let openai = match p.scope {
        VoiceConsentScope::SpeechRecognition => p.engine.is_none_or(|e| e == SttEngine::OpenAiWhisper),
        VoiceConsentScope::CloudSpeech => p.tts_engine.is_none_or(|e| e == TtsEngine::OpenAi),
    };
    if openai {
        let key = match p.scope {
            VoiceConsentScope::SpeechRecognition => STT_PROVIDER_KEY,
            VoiceConsentScope::CloudSpeech => TTS_PROVIDER_KEY,
        };
        let bound = if p.granted { pick_provider(&ctx, &next).ok().map(|v| v.provider.id.to_string()) } else { None };
        store_setting(&ctx, key, bound).await?;
    }
    let sec = &ctx.services.security;
    let action = if p.granted { "voice.consentGranted" } else { "voice.consentRevoked" };
    sec.audit.record(NewAudit::new(sec.user(), action, resource, AuditOutcome::Executed)).await;
    tracing::info!(scope = ?p.scope, granted = p.granted, "voice consent changed");
    Ok(result(&ctx, next))
}

/// The voice API key of a slot, from the secret vault (`None` when unset).
async fn engine_key(ctx: &RpcContext, key: VoiceKeyId) -> Option<zeroize::Zeroizing<String>> {
    if !configured_keys(ctx).await.contains(&key) {
        return None;
    }
    ctx.services
        .security
        .secrets
        .resolve_by_name(key.secret_name(), Actor::System, orkestr_security::AccessContext::default())
        .await
        .ok()
}

async fn configured_keys(ctx: &RpcContext) -> Vec<VoiceKeyId> {
    let names: Vec<String> = ctx
        .services
        .security
        .secrets
        .list(None)
        .await
        .map(|v| v.into_iter().map(|r| r.name).collect())
        .unwrap_or_default();
    VoiceKeyId::ALL.into_iter().filter(|k| names.iter().any(|n| n == k.secret_name())).collect()
}

fn voice_error_to_rpc(e: orkestr_voice::VoiceError) -> RpcError {
    use orkestr_voice::VoiceError as E;
    match e {
        E::NotInstalled(_) | E::NeedsKey(_) | E::Unavailable(_) | E::BadRequest(_) => {
            RpcError::bad_request(e.to_string())
        }
        E::Network(m) | E::Failed(m) => RpcError::internal(m),
    }
}

/// OpenAI through the user's provider (no voice API key configured).
async fn provider_transcribe(
    ctx: &RpcContext,
    config: &VoiceConfig,
    audio: Vec<u8>,
    mime_type: String,
    language: Option<String>,
) -> Result<(String, Option<String>), RpcError> {
    let provider = consented_provider(ctx, config, STT_PROVIDER_KEY).await?;
    let adapter = ctx
        .services
        .providers
        .adapter(provider.provider.id)
        .ok_or_else(|| RpcError::not_found("speech provider is not loaded"))?;
    let out = adapter
        .transcribe(TranscriptionRequest { audio, mime_type, language, model: None })
        .await
        .map_err(|e| provider_error_to_rpc(&e))?;
    Ok((out.text, out.language))
}

async fn transcribe(ctx: RpcContext, p: TranscribeParams) -> Result<TranscribeResult, RpcError> {
    let started = std::time::Instant::now();
    let config = load(&ctx).await?;
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
    let chain = match p.engine {
        Some(e) => {
            if !config.stt_engine_consented(e) {
                return Err(RpcError::forbidden(format!("speech recognition with {e:?} needs consent first")));
            }
            vec![e]
        }
        None => config.stt_chain(language.as_deref()),
    };
    if chain.is_empty() || chain.iter().any(|e| !config.stt_engine_consented(*e)) {
        return Err(RpcError::forbidden("speech recognition in the core is not enabled (with consent)"));
    }
    // Local engines need PCM; uploads are WAV (the voice worklet encodes 16 kHz WAV).
    let pcm = orkestr_voice::audio::to_pcm16k(&audio).ok();
    let mut first_err: Option<RpcError> = None;
    for engine in chain {
        let r: Result<(String, Option<String>), RpcError> = async {
            let key = match engine.key() {
                Some(k) => engine_key(&ctx, k).await,
                None => None,
            };
            if engine == SttEngine::OpenAiWhisper && key.is_none() {
                return provider_transcribe(&ctx, &config, audio.clone(), p.mime_type.clone(), language.clone()).await;
            }
            let pcm = pcm.as_ref().ok_or_else(|| RpcError::bad_request("audio must be a PCM WAV file"))?;
            ctx.services
                .voice
                .transcribe(engine, &config.engines, key.as_deref().map(String::as_str), pcm, language.as_deref())
                .await
                .map(|t| (t, None))
                .map_err(voice_error_to_rpc)
        }
        .await;
        match r {
            Ok((text, lang)) => {
                return Ok(TranscribeResult {
                    text: text.trim().to_owned(),
                    language: lang,
                    engine: Some(engine),
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                })
            }
            Err(e) => {
                tracing::warn!(?engine, error = %e.message, "speech recognition engine failed");
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| RpcError::internal("speech recognition failed")))
}

const PHRASE_SYSTEM: &str = "You are Orkestr, a friendly voice assistant that oversees AI coding agents. Turn the facts you are given into a short spoken reply. Rules: answer in the requested language; 1-2 conversational sentences; no IDs, slugs, file paths, markdown, lists or emoji; mention the project only if the facts say to, and then by its human name; round numbers; put what needs attention last and, if useful, offer one next step as a short question. Reply with the spoken text only.";

/// `voice.phrase`: facts → a natural spoken sentence via the Manager's model.
async fn phrase(ctx: RpcContext, p: PhraseReplyParams) -> Result<PhraseReplyResult, RpcError> {
    let facts = p.facts.trim();
    if facts.is_empty() || facts.chars().count() > 2_000 {
        return Err(RpcError::bad_request("facts must be 1..2000 characters"));
    }
    let lang = if p.lang.trim().is_empty() { "en" } else { p.lang.trim() };
    let user = format!("Language: {lang}
Facts:
{facts}");
    let limit = std::time::Duration::from_millis(u64::from(p.timeout_ms.unwrap_or(3_000).clamp(500, 10_000)));
    let cancel = tokio_util::sync::CancellationToken::new();
    let call = super::computer::manager_complete(&ctx.services, PHRASE_SYSTEM.to_owned(), user, 160, true, cancel.clone());
    let (text, model) = match tokio::time::timeout(limit, call).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(RpcError::conflict(e)),
        Err(_) => {
            cancel.cancel();
            return Err(RpcError::conflict("the model did not answer in time"));
        }
    };
    let text = text.trim().trim_matches('"').trim().to_owned();
    if text.is_empty() || text.chars().count() > 600 || text.contains("```") || text.starts_with('{') {
        return Err(RpcError::conflict("the model returned an unusable reply"));
    }
    Ok(PhraseReplyResult { text, model })
}

async fn synthesize(ctx: RpcContext, p: SynthesizeParams) -> Result<SynthesizeResult, RpcError> {
    let config = load(&ctx).await?;
    let text = p.text.trim();
    if text.is_empty() || text.chars().count() > MAX_SYNTHESIZE_CHARS {
        return Err(RpcError::bad_request(format!("text must be 1..={MAX_SYNTHESIZE_CHARS} characters")));
    }
    if p.voice.as_deref().is_some_and(|v| v.chars().count() > 100 || v.chars().any(char::is_control)) {
        return Err(RpcError::bad_request("invalid voice id"));
    }
    let language = p
        .language
        .clone()
        .filter(|l| l.len() <= 16 && l != "auto")
        .or_else(|| Some(config.language.clone()).filter(|l| l != "auto"))
        .unwrap_or_else(|| "ru-RU".into());
    let locale = orkestr_voice::cloud::locale(Some(&language));
    let chain = match p.engine {
        Some(e) => {
            if !config.tts_engine_consented(e) {
                return Err(RpcError::forbidden(format!("cloud text-to-speech with {e:?} needs consent first")));
            }
            vec![e]
        }
        None => config.tts_chain(Some(&language)),
    };
    if chain.is_empty() || chain.iter().any(|e| !config.tts_engine_consented(*e)) {
        return Err(RpcError::forbidden("no core text-to-speech engine is enabled (with consent)"));
    }
    let style = p.style.filter(|s| s.chars().count() <= 200);
    let mut first_err: Option<RpcError> = None;
    for engine in chain {
        // A voice from the request applies to the requested engine only.
        let voice = if p.engine.is_some() || engine == config.tts_engine_for(Some(&language)) {
            p.voice.clone().filter(|v| !v.trim().is_empty())
        } else {
            None
        };
        let r: Result<(Vec<u8>, String), RpcError> = async {
            let key = match engine.key() {
                Some(k) => engine_key(&ctx, k).await,
                None => None,
            };
            if engine == TtsEngine::OpenAi && key.is_none() {
                let voice = voice.clone().unwrap_or_else(|| config.cloud_voice.clone());
                if voice.len() > 40 || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    return Err(RpcError::bad_request("invalid voice id"));
                }
                let provider = consented_provider(&ctx, &config, TTS_PROVIDER_KEY).await?;
                let adapter = ctx
                    .services
                    .providers
                    .adapter(provider.provider.id)
                    .ok_or_else(|| RpcError::not_found("speech provider is not loaded"))?;
                let out = adapter
                    .synthesize_speech(SpeechRequest {
                        text: text.to_owned(),
                        voice,
                        style: style.clone(),
                        model: None,
                    })
                    .await
                    .map_err(|e| provider_error_to_rpc(&e))?;
                return Ok((out.bytes, out.mime_type));
            }
            let voice = voice.or_else(|| (engine == TtsEngine::OpenAi).then(|| config.cloud_voice.clone()));
            ctx.services
                .voice
                .synthesize(
                    engine,
                    &config.engines,
                    key.as_deref().map(String::as_str),
                    voice.as_deref(),
                    &locale,
                    config.rate_pct,
                    text,
                )
                .await
                .map(|c| (c.bytes, c.mime_type))
                .map_err(voice_error_to_rpc)
        }
        .await;
        match r {
            Ok((bytes, mime_type)) => {
                return Ok(SynthesizeResult {
                    audio_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    mime_type,
                    engine: Some(engine),
                })
            }
            Err(e) => {
                tracing::warn!(?engine, error = %e.message, "text-to-speech engine failed");
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| RpcError::internal("text-to-speech failed")))
}

async fn list_engines(ctx: RpcContext, p: ListVoiceEnginesParams) -> Result<VoiceEnginesResult, RpcError> {
    let config = load(&ctx).await?;
    engines_result(&ctx, &config, p.refresh.unwrap_or(false)).await
}

async fn engines_result(ctx: &RpcContext, config: &VoiceConfig, refresh: bool) -> Result<VoiceEnginesResult, RpcError> {
    let keys = configured_keys(ctx).await;
    let mut r = ctx.services.voice.engines(config, &keys, refresh).await;
    // OpenAI also works through an OpenAI provider without a voice key.
    let provider = pick_provider(ctx, config).is_ok_and(|v| v.has_credential);
    for e in r.stt.iter_mut().filter(|e| e.stt == Some(SttEngine::OpenAiWhisper)) {
        if provider && e.status == VoiceEngineStatus::NeedsKey {
            e.status = VoiceEngineStatus::Ready;
            e.detail = Some("uses your OpenAI provider".into());
        }
    }
    for e in r.tts.iter_mut().filter(|e| e.tts == Some(TtsEngine::OpenAi)) {
        if provider && e.status == VoiceEngineStatus::NeedsKey {
            e.status = VoiceEngineStatus::Ready;
            e.detail = Some("uses your OpenAI provider".into());
        }
    }
    Ok(r)
}

async fn set_engine_key(ctx: RpcContext, p: SetVoiceEngineKeyParams) -> Result<VoiceEnginesResult, RpcError> {
    let sec = &ctx.services.security;
    let name = p.key.secret_name();
    match p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(value) => {
            if value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(RpcError::bad_request("the API key must be 1..=4096 printable characters"));
            }
            sec.secrets
                .set(name, SecretScope::Global, zeroize::Zeroizing::new(value.to_owned()), sec.user())
                .await
                .map_err(crate::error::security_to_rpc)?;
        }
        None => {
            if let Some(r) = sec
                .secrets
                .list(None)
                .await
                .map_err(crate::error::security_to_rpc)?
                .into_iter()
                .find(|r| r.name == name)
            {
                sec.secrets.delete(r.id, sec.user()).await.map_err(crate::error::security_to_rpc)?;
            }
        }
    }
    tracing::info!(key = ?p.key, set = p.api_key.is_some(), "voice engine key updated");
    let config = load(&ctx).await?;
    engines_result(&ctx, &config, false).await
}

fn models_err(e: orkestr_voice::VoiceError) -> RpcError {
    voice_error_to_rpc(e)
}

async fn list_models(ctx: RpcContext) -> Result<VoiceModelsResult, RpcError> {
    let models = ctx.services.voice.models().clone();
    tokio::task::spawn_blocking(move || models.list()).await.map_err(|e| RpcError::internal(e.to_string()))
}

async fn download_model(ctx: RpcContext, p: VoicePackageParams) -> Result<VoiceModelsResult, RpcError> {
    ctx.services.voice.models().download(&p.id).map_err(models_err)?;
    let sec = &ctx.services.security;
    sec.audit
        .record(NewAudit::new(
            sec.user(),
            "voice.modelDownload",
            format!("voice:model:{}", p.id),
            AuditOutcome::Executed,
        ))
        .await;
    list_models(ctx).await
}

async fn cancel_download(ctx: RpcContext, p: VoicePackageParams) -> Result<VoiceModelsResult, RpcError> {
    ctx.services.voice.models().cancel(&p.id);
    list_models(ctx).await
}

async fn remove_model(ctx: RpcContext, p: VoicePackageParams) -> Result<VoiceModelsResult, RpcError> {
    ctx.services.voice.models().remove(&p.id).map_err(models_err)?;
    let sec = &ctx.services.security;
    sec.audit
        .record(NewAudit::new(sec.user(), "voice.modelRemove", format!("voice:model:{}", p.id), AuditOutcome::Executed))
        .await;
    list_models(ctx).await
}

async fn verify_model(ctx: RpcContext, p: VoicePackageParams) -> Result<VoiceVerifyResult, RpcError> {
    ctx.services.voice.models().verify(&p.id).await.map_err(models_err)
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
