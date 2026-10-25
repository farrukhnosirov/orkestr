//! Jarvis persona & conversation mode (`jarvis.*`, ADR-0049).
//!
//! - Settings: one JSON document in `core_settings` under [`JARVIS_KEY`].
//! - History: the conversation (≤ [`HISTORY_CAP`] messages) under
//!   [`HISTORY_KEY`]; `jarvis.forget` clears it. Template answers and
//!   executed commands join it through `jarvis.record`, so the model knows
//!   what was said and done.
//! - `jarvis.match`: the local template bank ([`DialogBank`]), Russian only.
//! - `jarvis.chat`: one AI turn. The model call lives in [`call_model`] only:
//!   the configured conversation model first, then the AI Manager's model,
//!   then routing — all through the provider registry's fallback chain, so a
//!   failing subscription / key switches to the next provider. Deltas stream
//!   as ephemeral `JarvisMessage` events.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use orkestr_computer::dialog::{DialogBank, RenderContext};
use orkestr_domain::{AiPolicy, ModelRef, ProviderKind, TaskStatus};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::jarvis::{
    self as m, JarvisAddress, JarvisChatParams, JarvisChatResult, JarvisConfig, JarvisConfigResult, JarvisDeltaPayload,
    JarvisHistoryParams, JarvisHistoryResult, JarvisMatchParams, JarvisMatchResult, JarvisMessage, JarvisRecordParams,
    JarvisRole, JarvisTemplateReply, SetJarvisConfigParams, HISTORY_CAP, MAX_CHAT_CHARS,
};
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, RequiredCapabilities, RouteRequest};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;
use crate::Services;

/// `core_settings` key of the settings document.
pub const JARVIS_KEY: &str = "jarvis";
/// `core_settings` key of the conversation history.
pub const HISTORY_KEY: &str = "jarvis.history";
/// Overall limit of one AI turn.
const CHAT_TIMEOUT: Duration = Duration::from_secs(90);

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::GetConfig, _, _>(
            |ctx: RpcContext, _: Empty| async move { config_result(load_config(&ctx).await?) },
        )
        .method::<m::SetConfig, _, _>(set_config)
        .method::<m::Match, _, _>(match_text)
        .method::<m::Chat, _, _>(chat)
        .method::<m::Cancel, _, _>(|_ctx: RpcContext, p: m::JarvisCancelParams| async move {
            if let Some(t) = running().lock().remove(&p.message_id) {
                t.cancel();
            }
            Ok::<_, RpcError>(Empty {})
        })
        .method::<m::History, _, _>(|ctx: RpcContext, p: JarvisHistoryParams| async move {
            let mut messages = load_history(&ctx).await?;
            let limit = p.limit.unwrap_or(50).clamp(1, HISTORY_CAP as u32) as usize;
            if messages.len() > limit {
                messages.drain(..messages.len() - limit);
            }
            Ok::<_, RpcError>(JarvisHistoryResult { messages })
        })
        .method::<m::Record, _, _>(record)
        .method::<m::Forget, _, _>(|ctx: RpcContext, _: Empty| async move {
            let _guard = write_lock().lock().await;
            store(&ctx, HISTORY_KEY, None).await?;
            Ok::<_, RpcError>(Empty {})
        });
}

fn running() -> &'static Mutex<HashMap<String, CancellationToken>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn load(ctx: &RpcContext, key: &'static str) -> Result<Option<String>, RpcError> {
    ctx.services
        .db
        .call(move |c| {
            Ok(c.query_row("SELECT value FROM core_settings WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
        })
        .await
        .map_err(storage_to_rpc)
}

async fn store(ctx: &RpcContext, key: &'static str, value: Option<String>) -> Result<(), RpcError> {
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

/// Stored settings; anything unreadable or invalid → defaults.
async fn load_config(ctx: &RpcContext) -> Result<JarvisConfig, RpcError> {
    Ok(load(ctx, JARVIS_KEY)
        .await?
        .and_then(|raw| serde_json::from_str::<JarvisConfig>(&raw).ok())
        .filter(|c| c.validate().is_ok())
        .unwrap_or_default())
}

fn config_result(config: JarvisConfig) -> Result<JarvisConfigResult, RpcError> {
    Ok(JarvisConfigResult { config, bank: DialogBank::builtin().stats() })
}

async fn set_config(ctx: RpcContext, p: SetJarvisConfigParams) -> Result<JarvisConfigResult, RpcError> {
    p.config.validate().map_err(RpcError::bad_request)?;
    if let Some(model) = &p.config.model {
        if ctx.services.providers.list().iter().all(|v| v.provider.id != model.provider_id) {
            return Err(RpcError::bad_request("the conversation model references an unknown provider"));
        }
    }
    let raw = serde_json::to_string(&p.config).map_err(|e| RpcError::internal(format!("serialize: {e}")))?;
    store(&ctx, JARVIS_KEY, Some(raw)).await?;
    config_result(p.config)
}

async fn load_history(ctx: &RpcContext) -> Result<Vec<JarvisMessage>, RpcError> {
    Ok(load(ctx, HISTORY_KEY).await?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default())
}

/// Appends messages (capped) under the write lock.
async fn append_history(ctx: &RpcContext, add: Vec<JarvisMessage>) -> Result<(), RpcError> {
    let _guard = write_lock().lock().await;
    let mut all = load_history(ctx).await?;
    all.extend(add);
    if all.len() > HISTORY_CAP {
        all.drain(..all.len() - HISTORY_CAP);
    }
    let raw = serde_json::to_string(&all).map_err(|e| RpcError::internal(format!("serialize: {e}")))?;
    store(ctx, HISTORY_KEY, Some(raw)).await
}

fn message(role: JarvisRole, text: &str, source: &str) -> JarvisMessage {
    JarvisMessage {
        id: uuid::Uuid::now_v7().to_string(),
        role,
        text: text.to_owned(),
        at: chrono::Utc::now(),
        source: source.to_owned(),
    }
}

fn seed() -> u64 {
    let b = uuid::Uuid::now_v7().into_bytes();
    // v7: bytes 8..16 are random except the two variant bits of byte 8 (kept on top)
    u64::from_be_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]])
}

fn is_russian(lang: &str) -> bool {
    lang.trim().to_lowercase().starts_with("ru")
}

async fn match_text(ctx: RpcContext, p: JarvisMatchParams) -> Result<JarvisMatchResult, RpcError> {
    let started = std::time::Instant::now();
    if p.text.chars().count() > MAX_CHAT_CHARS {
        return Err(RpcError::bad_request(format!("text must be at most {MAX_CHAT_CHARS} characters")));
    }
    let config = load_config(&ctx).await?;
    let bank = DialogBank::builtin();
    let found = if config.local_templates && is_russian(&p.lang) { bank.find(&p.text) } else { None };
    let matched = found.map(|m| {
        let intent = bank.intent(&m);
        let render = RenderContext {
            address: config.address,
            user_name: config.user_name.clone(),
            persona: config.persona,
            now: chrono::Local::now().naive_local(),
            last_reply: p.last_reply.clone(),
            seed: seed(),
        };
        JarvisTemplateReply {
            intent: m.intent.clone(),
            category: intent.category.clone(),
            action: intent.action.clone(),
            text: bank.answer(&m, &render),
            score: m.score,
            phrase: m.phrase.clone(),
        }
    });
    Ok(JarvisMatchResult { matched, elapsed_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX) })
}

async fn record(ctx: RpcContext, p: JarvisRecordParams) -> Result<Empty, RpcError> {
    let (user, reply) = (p.user.trim(), p.reply.trim());
    if user.is_empty()
        || reply.is_empty()
        || user.chars().count() > MAX_CHAT_CHARS
        || reply.chars().count() > MAX_CHAT_CHARS
    {
        return Err(RpcError::bad_request("user and reply must be 1..4000 characters"));
    }
    let source = if p.source == "command" { "command" } else { "template" };
    append_history(&ctx, vec![message(JarvisRole::User, user, "user"), message(JarvisRole::Assistant, reply, source)])
        .await?;
    Ok(Empty {})
}

/// The address word in the reply language («сэр», "boss", the name), if any.
fn address_word(config: &JarvisConfig, ru: bool) -> Option<String> {
    match config.address {
        JarvisAddress::Sir => Some(if ru { "сэр" } else { "sir" }.to_owned()),
        JarvisAddress::Boss => Some(if ru { "босс" } else { "boss" }.to_owned()),
        JarvisAddress::Name => Some(config.user_name.trim().to_owned()).filter(|n| !n.is_empty()),
        JarvisAddress::None => None,
    }
}

/// The Jarvis system prompt.
#[must_use]
pub fn system_prompt(
    config: &JarvisConfig,
    lang: &str,
    spoken: bool,
    now: chrono::DateTime<chrono::Local>,
    projects: &str,
) -> String {
    let ru = is_russian(lang);
    let address = if !config.persona {
        "Do not use any form of address.".to_owned()
    } else {
        let word = address_word(config, ru);
        word.map_or_else(
            || "Do not use any form of address.".to_owned(),
            |w| format!("Address the user as «{w}» — naturally, usually once per reply (e.g. «Сделано, {w}.»)."),
        )
    };
    let persona = if config.persona {
        "You are JARVIS, the personal AI assistant of this computer's owner, living inside Orkestr (a desktop app that orchestrates AI coding agents and controls the computer by voice). Speak like JARVIS from Iron Man: polite, calm, precise, a British butler's dry wit; helpful first, witty second; never servile, never rude."
    } else {
        "You are the personal AI assistant inside Orkestr (a desktop app that orchestrates AI coding agents and controls the computer by voice). Be friendly, calm and precise."
    };
    let style = if spoken {
        "Your reply is spoken aloud: 1-3 short sentences, no markdown, lists, code, emoji or URLs; say numbers and times the way people say them."
    } else {
        "Keep replies short (at most about six sentences) and in plain text; use code only when asked."
    };
    let language = if ru { "Russian" } else { "English" };
    let mut s = format!(
        "{persona} {address} Answer in {language}. {style} Current local date and time: {} (the user's clock). \
         If the user asks you to do something on the computer (open a program or website, search the web, change the volume, take a screenshot, set a timer, scroll…), \
         reply with one short sentence and put the exact command on the last line as <<run: command in the user's language>> (for example <<run: открой youtube>>). \
         Never claim you did something yourself otherwise; never invent facts about the user's projects.",
        now.format("%A, %Y-%m-%d %H:%M")
    );
    if !projects.is_empty() {
        s.push_str("\nThe user's Orkestr projects (name: tasks running / failed):\n");
        s.push_str(projects);
    }
    s
}

/// One persona sentence for other spoken AI replies (`voice.phrase`): the
/// Jarvis manner and the form of address, or "" with the persona off.
pub(crate) async fn persona_line(ctx: &RpcContext, lang: &str) -> String {
    let config = load_config(ctx).await.unwrap_or_default();
    if !config.persona {
        return String::new();
    }
    let ru = is_russian(lang);
    let word = address_word(&config, ru);
    let address = word.map_or_else(String::new, |w| format!(" Address the user as «{w}» once."));
    format!("Speak like JARVIS from Iron Man: polite, calm, concise, with a light dry wit.{address}")
}

/// Short project summary for the prompt (names and task counts; best effort).
async fn project_summary(s: &Services) -> String {
    let Ok(projects) = s.projects.list(false).await else { return String::new() };
    let tasks = s.tasks.list(ListTasksParams::default()).await.unwrap_or_default();
    let mut out = String::new();
    for p in projects.iter().take(12) {
        let mine = tasks.iter().filter(|t| t.project_id == p.id);
        let (mut running, mut failed) = (0, 0);
        for t in mine {
            match t.status {
                TaskStatus::Running => running += 1,
                TaskStatus::Failed => failed += 1,
                _ => {}
            }
        }
        out.push_str(&format!("- {}: {running} running / {failed} failed\n", p.name.replace('_', " ")));
    }
    out
}

/// Splits the hand-off line off a reply: `(text, command)`.
#[must_use]
pub fn split_command(reply: &str) -> (String, Option<String>) {
    let Some(start) = reply.find("<<") else { return (reply.trim().to_owned(), None) };
    let rest = &reply[start + 2..];
    let end = rest.find(">>").unwrap_or(rest.len());
    let inner = rest[..end].trim();
    let cmd = inner
        .strip_prefix("run:")
        .or_else(|| inner.strip_prefix("run :"))
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty() && c.chars().count() <= 300);
    let after = rest.get(end + 2..).unwrap_or("");
    let text = format!("{}{}", &reply[..start], after).trim().to_owned();
    (text, cmd)
}

/// THE model call of the conversation (one place, ADR-0049): builds the
/// fallback chain — the configured conversation model, the AI Manager's
/// model, then routing (the offline Mock only when chosen explicitly) — and
/// streams through the provider registry's fallback (`providers.chat`), so a
/// failing subscription / API key moves on to the next provider. `on_delta`
/// gets `(delta, reset)`.
///
/// # Errors
/// No model available, the model failed on every provider, or cancelled.
pub(crate) async fn call_model(
    s: &Services,
    preferred: Option<ModelRef>,
    system: String,
    messages: Vec<ChatMessage>,
    max_output_tokens: u32,
    cancel: CancellationToken,
    mut on_delta: impl FnMut(&str, bool),
) -> Result<(String, String), String> {
    let mut chain: Vec<ModelRef> = Vec::new();
    let explicit = preferred.clone();
    if let Some(m) = preferred {
        chain.push(m);
    }
    if let Some(m) = s.providers.manager_config().await.ok().and_then(|c| c.model) {
        if !chain.contains(&m) {
            chain.push(m);
        }
    }
    let routed = s
        .providers
        .route(&RouteRequest {
            purpose: Some("Manager".into()),
            required: RequiredCapabilities::default(),
            preferred: chain.first().cloned(),
            fallback_chain: Vec::new(),
            policy: AiPolicy::default(),
            max_cost_usd: None,
            est_input_tokens: 2_000,
            max_output_tokens: u64::from(max_output_tokens),
            exclude: Vec::new(),
        })
        .await
        .unwrap_or_default();
    for r in routed {
        if !chain.contains(&r) {
            chain.push(r);
        }
    }
    let mock: Vec<_> = s
        .providers
        .list()
        .into_iter()
        .filter(|v| v.provider.kind == ProviderKind::Mock)
        .map(|v| v.provider.id)
        .collect();
    chain.retain(|m| !mock.contains(&m.provider_id) || explicit.as_ref() == Some(m));
    let head = chain.first().cloned().ok_or_else(|| "no chat model is available (connect a provider)".to_owned())?;
    let mut req = ChatRequest::new(head.model.clone(), messages);
    req.system = Some(system);
    req.max_output_tokens = Some(max_output_tokens);
    let mut stream = s.providers.chat(chain, req, None, cancel.clone());
    let (mut text, mut model) = (String::new(), head);
    while let Some(ev) = stream.next().await {
        match ev {
            ChatEvent::TextDelta(t) => {
                on_delta(&t, false);
                text.push_str(&t);
            }
            ChatEvent::Switched { to, .. } => {
                text.clear();
                on_delta("", true);
                model = to;
            }
            ChatEvent::Done { .. } => return Ok((text, model.model)),
            ChatEvent::Error(e) => return Err(format!("model call failed: {e}")),
            ChatEvent::Usage(_) | ChatEvent::ToolCall { .. } => {}
        }
    }
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    Err("the model stream ended unexpectedly".into())
}

/// Rolling context: the last `turns` messages, alternating roles (a run of
/// one role is merged), starting with the user.
fn context_messages(history: &[JarvisMessage], turns: usize) -> Vec<ChatMessage> {
    let tail = &history[history.len().saturating_sub(turns)..];
    let mut out: Vec<(JarvisRole, String)> = Vec::new();
    for m in tail {
        match out.last_mut() {
            Some((role, text)) if *role == m.role => {
                text.push('\n');
                text.push_str(&m.text);
            }
            _ => out.push((m.role, m.text.clone())),
        }
    }
    while out.first().is_some_and(|(r, _)| *r == JarvisRole::Assistant) {
        out.remove(0);
    }
    out.into_iter()
        .map(|(r, t)| if r == JarvisRole::User { ChatMessage::user(t) } else { ChatMessage::assistant(t) })
        .collect()
}

async fn chat(ctx: RpcContext, p: JarvisChatParams) -> Result<JarvisChatResult, RpcError> {
    let text = p.text.trim().to_owned();
    if text.is_empty() || text.chars().count() > MAX_CHAT_CHARS {
        return Err(RpcError::bad_request(format!("text must be 1..={MAX_CHAT_CHARS} characters")));
    }
    let message_id = p
        .message_id
        .clone()
        .filter(|id| !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let config = load_config(&ctx).await?;
    let history = load_history(&ctx).await?;
    let mut messages = context_messages(&history, usize::from(config.memory_turns));
    match messages.last_mut() {
        // the user spoke twice in a row (e.g. a failed turn): keep both
        Some(last) if last.role == orkestr_providers::Role::User => {
            last.content.push('\n');
            last.content.push_str(&text);
        }
        _ => messages.push(ChatMessage::user(text.clone())),
    }
    let projects = if config.project_context { project_summary(&ctx.services).await } else { String::new() };
    let system = system_prompt(&config, &p.lang, p.spoken, chrono::Local::now(), &projects);
    let cancel = CancellationToken::new();
    running().lock().insert(message_id.clone(), cancel.clone());
    let services: Arc<Services> = Arc::clone(&ctx.services);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, bool)>();
    let publisher = {
        let services = Arc::clone(&services);
        let id = message_id.clone();
        tokio::spawn(async move {
            while let Some((delta, reset)) = rx.recv().await {
                let ev = NewEvent::new(EventKind::JarvisMessage(JarvisDeltaPayload {
                    message_id: id.clone(),
                    delta,
                    reset,
                    done: false,
                }));
                if let Err(e) = services.events.publish(ev).await {
                    tracing::debug!(error = %e, "could not publish JarvisMessage");
                }
            }
        })
    };
    let max_tokens = if p.spoken { 400 } else { 1_200 };
    let call = call_model(&services, config.model.clone(), system, messages, max_tokens, cancel.clone(), |d, reset| {
        let _ = tx.send((d.to_owned(), reset));
    });
    let result = tokio::time::timeout(CHAT_TIMEOUT, call).await;
    drop(tx);
    let _ = publisher.await;
    running().lock().remove(&message_id);
    let done = NewEvent::new(EventKind::JarvisMessage(JarvisDeltaPayload {
        message_id: message_id.clone(),
        delta: String::new(),
        reset: false,
        done: true,
    }));
    let _ = services.events.publish(done).await;
    let (reply, model) = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(RpcError::conflict(e)),
        Err(_) => {
            cancel.cancel();
            return Err(RpcError::conflict("the model did not answer in time"));
        }
    };
    let (reply_text, command) = split_command(&reply);
    if reply_text.is_empty() && command.is_none() {
        return Err(RpcError::conflict("the model returned an empty reply"));
    }
    append_history(
        &ctx,
        vec![message(JarvisRole::User, &text, "user"), message(JarvisRole::Assistant, &reply_text, "ai")],
    )
    .await?;
    Ok(JarvisChatResult { message_id, text: reply_text, model, command })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_hand_off_is_split() {
        assert_eq!(
            split_command("Открываю YouTube, сэр.\n<<run: открой youtube>>"),
            ("Открываю YouTube, сэр.".into(), Some("открой youtube".into()))
        );
        assert_eq!(split_command("Просто ответ."), ("Просто ответ.".into(), None));
        assert_eq!(split_command("Текст <<что-то>> ещё"), ("Текст  ещё".into(), None));
    }

    #[test]
    fn context_alternates_and_starts_with_the_user() {
        let h = vec![
            message(JarvisRole::Assistant, "a0", "ai"),
            message(JarvisRole::User, "u1", "user"),
            message(JarvisRole::User, "u2", "user"),
            message(JarvisRole::Assistant, "a1", "ai"),
        ];
        let c = context_messages(&h, 10);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].content, "u1\nu2");
        let c = context_messages(&h, 1);
        assert!(c.is_empty(), "a lone assistant message is dropped");
    }

    #[test]
    fn prompt_carries_persona_address_and_time() {
        let now = chrono::Local::now();
        let p = system_prompt(&JarvisConfig::default(), "ru", true, now, "- shop: 1 running / 0 failed\n");
        assert!(p.contains("JARVIS") && p.contains("«сэр»") && p.contains("Russian") && p.contains("spoken aloud"));
        assert!(p.contains("<<run:") && p.contains("shop"));
        let c = JarvisConfig { address: JarvisAddress::Boss, ..JarvisConfig::default() };
        assert!(system_prompt(&c, "en", false, now, "").contains("«boss»"));
        let c = JarvisConfig { persona: false, ..JarvisConfig::default() };
        assert!(system_prompt(&c, "ru", false, now, "").contains("Do not use any form of address"));
    }
}
