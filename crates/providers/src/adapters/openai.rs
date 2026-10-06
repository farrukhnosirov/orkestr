//! OpenAI Chat Completions (streaming + tools + structured output).
//!
//! The same codec serves `openai`, `deepseek` (fixed base URL preset) and
//! `openai_compatible` (custom base URL, optional key, custom headers).
//! Wire API pinned: `POST {base}/chat/completions` with `stream: true` and
//! `stream_options.include_usage = true`.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::catalog::{default_models, kind_info};
use crate::http::{self, drive, Emitter, HttpConfig, SseParser, PROBE_TIMEOUT};
use crate::pricing::pricing_for;
use crate::types::{
    parse_tool_arguments, AuthStatus, ChatEvent, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality,
    ProviderDescriptor, ProviderError, ResponseFormat, Role, StopReason, ToolChoice, Usage,
};

/// Which preset of the OpenAI wire this adapter speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiFlavor {
    /// api.openai.com.
    OpenAi,
    /// api.deepseek.com.
    DeepSeek,
    /// Any compatible server.
    Compatible {
        /// Endpoint is on this machine (privacy routing).
        local: bool,
    },
}

/// OpenAI-wire adapter.
#[derive(Debug)]
pub struct OpenAiAdapter {
    cfg: HttpConfig,
    flavor: OpenAiFlavor,
}

impl OpenAiAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(cfg: HttpConfig, flavor: OpenAiFlavor) -> Self {
        Self { cfg, flavor }
    }

    fn kind(&self) -> ProviderKind {
        match self.flavor {
            OpenAiFlavor::OpenAi => ProviderKind::OpenAi,
            OpenAiFlavor::DeepSeek => ProviderKind::DeepSeek,
            OpenAiFlavor::Compatible { .. } => ProviderKind::OpenAiCompatible,
        }
    }

    fn is_local(&self) -> bool {
        matches!(self.flavor, OpenAiFlavor::Compatible { local: true })
    }

    async fn authed(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, ProviderError> {
        let key = match self.flavor {
            OpenAiFlavor::Compatible { .. } => self.cfg.api_key().await?,
            _ => Some(self.cfg.require_api_key().await?),
        };
        let rb = self.cfg.with_headers(rb);
        Ok(match key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        })
    }

    async fn fetch_models(&self) -> Result<Vec<Model>, ProviderError> {
        let rb = self.authed(self.cfg.client.get(self.cfg.url("models")).timeout(PROBE_TIMEOUT)).await?;
        let resp = http::send(rb).await?;
        let body: Value = resp.json().await.map_err(http::map_reqwest_error)?;
        let defaults = default_models(self.kind(), self.cfg.id, self.is_local());
        let mut out: Vec<Model> = body
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .filter(|id| self.flavor != OpenAiFlavor::OpenAi || is_chat_model(id))
                    .map(|id| {
                        defaults.iter().find(|d| d.id == id).cloned().unwrap_or_else(|| Model {
                            provider_id: self.cfg.id,
                            id: id.to_owned(),
                            display_name: id.to_owned(),
                            context_window: 128_000,
                            max_output_tokens: None,
                            capabilities: ModelCapabilities {
                                tools: true,
                                vision: false,
                                structured_output: true,
                                streaming: true,
                                reasoning: false,
                                coding_agent: false,
                            },
                            pricing: if self.is_local() { None } else { pricing_for(id) },
                            is_local: self.is_local(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
}

fn is_chat_model(id: &str) -> bool {
    let chat = id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("chatgpt");
    let excluded = ["audio", "realtime", "transcribe", "tts", "image", "embedding", "search", "instruct"];
    chat && !excluded.iter().any(|x| id.contains(x))
}

/// Builds the request body (pure; unit-tested).
#[must_use]
pub fn build_body(req: &ChatRequest, flavor: OpenAiFlavor) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = req.system_prompt() {
        messages.push(json!({"role": "system", "content": sys}));
    }
    for m in req.conversation() {
        match m.role {
            Role::User => {
                if m.images.is_empty() {
                    messages.push(json!({"role": "user", "content": m.content}));
                } else {
                    let mut parts = vec![json!({"type": "text", "text": m.content})];
                    for img in &m.images {
                        parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", img.media_type, img.data_base64)}}));
                    }
                    messages.push(json!({"role": "user", "content": parts}));
                }
            }
            Role::Assistant => {
                let mut msg = json!({"role": "assistant", "content": if m.content.is_empty() && !m.tool_calls.is_empty() { Value::Null } else { Value::String(m.content.clone()) }});
                if !m.tool_calls.is_empty() {
                    msg["tool_calls"] = m
                        .tool_calls
                        .iter()
                        .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments.to_string()}}))
                        .collect();
                }
                messages.push(msg);
            }
            Role::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": m.content,
            })),
            Role::System => {}
        }
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
        body["tool_choice"] = match &req.tool_choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Required => json!("required"),
            ToolChoice::Specific(n) => json!({"type": "function", "function": {"name": n}}),
        };
    }
    match &req.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject => body["response_format"] = json!({"type": "json_object"}),
        ResponseFormat::JsonSchema { name, schema } => {
            body["response_format"] = if flavor == OpenAiFlavor::DeepSeek {
                // DeepSeek supports JSON mode only; the schema goes into the prompt.
                json!({"type": "json_object"})
            } else {
                json!({"type": "json_schema", "json_schema": {"name": name, "schema": schema, "strict": false}})
            };
            if flavor == OpenAiFlavor::DeepSeek {
                if let Some(msgs) = body["messages"].as_array_mut() {
                    msgs.insert(0, json!({"role": "system", "content": format!("Respond with JSON only, matching this JSON schema:\n{schema}")}));
                }
            }
        }
    }
    if let Some(max) = req.max_output_tokens {
        let key = if flavor == OpenAiFlavor::OpenAi { "max_completion_tokens" } else { "max_tokens" };
        body[key] = json!(max);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let (Some(r), OpenAiFlavor::OpenAi) = (req.reasoning, flavor) {
        if is_reasoning_model(&req.model) {
            body["reasoning_effort"] = json!(r.as_str());
        }
    }
    if !req.stop.is_empty() {
        body["stop"] = json!(req.stop);
    }
    body
}

fn is_reasoning_model(model: &str) -> bool {
    model.starts_with('o') || model.starts_with("gpt-5")
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    args: String,
}

/// First key used for tool-call deltas that carry no `index` (kept apart
/// from real indexes so the two never merge).
const UNINDEXED_BASE: u64 = 1 << 32;

/// Incremental Chat Completions SSE decoder (pure state machine; unit-tested).
#[derive(Default)]
pub struct ChatCompletionsDecoder {
    calls: BTreeMap<u64, PendingCall>,
    /// Key of the call the last tool-call delta went to.
    last_call: Option<u64>,
    /// Index-less calls seen so far.
    unindexed: u64,
    usage: Option<Usage>,
    stop: Option<StopReason>,
    output_chars: usize,
    done: bool,
}

impl ChatCompletionsDecoder {
    /// Decodes one SSE `data:` payload into events. `[DONE]` marks the end.
    pub fn data(&mut self, data: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        let mut out = Vec::new();
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(out);
        }
        let v: Value =
            serde_json::from_str(data).map_err(|e| ProviderError::Server(format!("malformed stream chunk: {e}")))?;
        if let Some(err) = v.get("error") {
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("stream error").to_owned();
            let code = err.get("code").and_then(Value::as_str).unwrap_or_default();
            return Err(if code.contains("rate_limit") {
                ProviderError::RateLimited { retry_after_secs: None }
            } else if code.contains("quota") {
                ProviderError::QuotaExceeded(msg)
            } else {
                ProviderError::Server(msg)
            });
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            let get = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
            self.usage = Some(Usage {
                input_tokens: get("/prompt_tokens"),
                output_tokens: get("/completion_tokens"),
                cached_input_tokens: get("/prompt_tokens_details/cached_tokens").max(get("/prompt_cache_hit_tokens")),
                reasoning_tokens: get("/completion_tokens_details/reasoning_tokens"),
                ..Usage::default()
            });
        }
        let Some(choice) = v.get("choices").and_then(Value::as_array).and_then(|c| c.first()) else {
            return Ok(out);
        };
        if let Some(delta) = choice.get("delta") {
            // `reasoning_content` (DeepSeek) / `reasoning` are dropped by design.
            if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                self.output_chars += text.len();
                out.push(ChatEvent::TextDelta(text.to_owned()));
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for c in calls {
                    let index = match c.get("index").and_then(Value::as_u64) {
                        Some(i) => i,
                        None => self.unindexed_key(c.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())),
                    };
                    self.last_call = Some(index);
                    let entry = self.calls.entry(index).or_default();
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        entry.id = id.to_owned();
                    }
                    if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                        entry.name.push_str(name);
                    }
                    if let Some(args) = c.pointer("/function/arguments").and_then(Value::as_str) {
                        entry.args.push_str(args);
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop = Some(match reason {
                "stop" => StopReason::EndTurn,
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::ContentFilter,
                other => StopReason::Other(other.to_owned()),
            });
            out.extend(self.flush_calls());
        }
        Ok(out)
    }

    /// Key for a tool-call delta without `index` (non-conforming servers):
    /// a new non-empty `id` starts a new call; a delta without an id (or with
    /// the current call's id) continues the most recent call.
    fn unindexed_key(&mut self, id: Option<&str>) -> u64 {
        let current = self.last_call.filter(|k| self.calls.contains_key(k));
        match (id, current) {
            (None, Some(k)) => k,
            (Some(id), Some(k)) if self.calls.get(&k).is_some_and(|c| c.id == id) => k,
            _ => {
                let k = UNINDEXED_BASE + self.unindexed;
                self.unindexed += 1;
                k
            }
        }
    }

    fn flush_calls(&mut self) -> Vec<ChatEvent> {
        std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(i, c)| {
                self.output_chars += c.args.len();
                ChatEvent::ToolCall {
                    id: match (c.id.is_empty(), i.checked_sub(UNINDEXED_BASE)) {
                        (false, _) => c.id,
                        (true, Some(n)) => format!("call_u{n}"),
                        (true, None) => format!("call_{i}"),
                    },
                    name: c.name,
                    arguments: parse_tool_arguments(&c.args),
                }
            })
            .collect()
    }

    /// Whether `[DONE]` was seen.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Final events (pending tool calls, usage, Done).
    pub fn finish(&mut self, prompt_chars: usize) -> Vec<ChatEvent> {
        let mut out = self.flush_calls();
        let usage = self.usage.unwrap_or_else(|| Usage::estimate(prompt_chars, self.output_chars));
        out.push(ChatEvent::Usage(usage));
        let stop = self.stop.clone().unwrap_or(StopReason::EndTurn);
        out.push(ChatEvent::Done { stop_reason: stop });
        out
    }
}

async fn run_stream(
    resp: reqwest::Response,
    em: Emitter<ChatEvent>,
    idle: std::time::Duration,
    prompt_chars: usize,
) -> Result<(), ProviderError> {
    let mut body = resp.bytes_stream();
    let mut sse = SseParser::default();
    let mut dec = ChatCompletionsDecoder::default();
    loop {
        let chunk = http::next_chunk(&mut body, idle).await?;
        let events = match &chunk {
            Some(b) => sse.push(b),
            None => sse.finish(),
        };
        for ev in events {
            for out in dec.data(&ev.data)? {
                em.emit(out).await?;
            }
        }
        if chunk.is_none() || dec.is_done() {
            break;
        }
    }
    for out in dec.finish(prompt_chars) {
        em.emit(out).await?;
    }
    Ok(())
}

#[async_trait]
impl ProviderAdapter for OpenAiAdapter {
    fn id(&self) -> ProviderId {
        self.cfg.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        let local = self.is_local();
        ProviderDescriptor {
            kind: self.kind(),
            display_name: self.cfg.name.clone(),
            is_local: local,
            locality: if local {
                Locality::Local
            } else if matches!(self.flavor, OpenAiFlavor::Compatible { .. }) {
                Locality::SelfHosted
            } else {
                Locality::Cloud
            },
            capabilities: ModelCapabilities {
                tools: true,
                vision: self.flavor == OpenAiFlavor::OpenAi,
                structured_output: true,
                streaming: true,
                reasoning: self.flavor != OpenAiFlavor::Compatible { local },
                coding_agent: false,
            },
            max_context_tokens: if self.flavor == OpenAiFlavor::OpenAi { 1_047_576 } else { 128_000 },
            coding_agent: false,
            cost_model: match self.flavor {
                OpenAiFlavor::Compatible { local: true } => CostModelKind::Local,
                OpenAiFlavor::Compatible { local: false } => CostModelKind::Unknown,
                _ => CostModelKind::PerToken,
            },
            max_concurrency: if local { 1 } else { 4 },
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        match (self.flavor, self.cfg.api_key().await) {
            (_, Err(e)) => AuthStatus::Invalid { reason: e.to_string() },
            (OpenAiFlavor::Compatible { .. }, Ok(None)) => {
                AuthStatus::Unverified { label: "no API key (endpoint without auth)".into() }
            }
            (_, Ok(None)) => AuthStatus::NotConfigured,
            (_, Ok(Some(_))) => AuthStatus::Unverified { label: "API key".into() },
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        match self.fetch_models().await {
            Ok(m) if !m.is_empty() => Ok(m),
            Ok(_) => Ok(default_models(self.kind(), self.cfg.id, self.is_local())),
            // Many compatible servers have no /models (spec §3.3: tolerate 404).
            Err(ProviderError::BadRequest(_)) if matches!(self.flavor, OpenAiFlavor::Compatible { .. }) => {
                Ok(default_models(self.kind(), self.cfg.id, self.is_local()))
            }
            Err(e) => Err(e),
        }
    }

    async fn health(&self) -> HealthReport {
        let start = Instant::now();
        let result = self.fetch_models().await;
        let ms = Some(start.elapsed().as_millis() as u64);
        match result {
            Ok(models) => HealthReport::healthy(ms, Some(format!("{} models", models.len()))),
            Err(ProviderError::BadRequest(_)) if matches!(self.flavor, OpenAiFlavor::Compatible { .. }) => {
                HealthReport {
                    health: ProviderHealth::Healthy,
                    latency_ms: ms,
                    detail: Some("reachable (no /models endpoint)".into()),
                    version: None,
                }
            }
            Err(e) => HealthReport::from_error(&e, ms),
        }
    }

    async fn transcribe(
        &self,
        request: crate::audio::TranscriptionRequest,
    ) -> Result<crate::audio::Transcription, ProviderError> {
        if self.flavor == OpenAiFlavor::DeepSeek {
            return Err(ProviderError::Unsupported("DeepSeek has no speech-to-text".into()));
        }
        let key = match self.flavor {
            OpenAiFlavor::Compatible { .. } => self.cfg.api_key().await?,
            _ => Some(self.cfg.require_api_key().await?),
        };
        crate::audio::openai_transcribe(&self.cfg, key, request).await
    }

    async fn synthesize_speech(
        &self,
        request: crate::audio::SpeechRequest,
    ) -> Result<crate::audio::SpeechAudio, ProviderError> {
        if self.flavor == OpenAiFlavor::DeepSeek {
            return Err(ProviderError::Unsupported("DeepSeek has no text-to-speech".into()));
        }
        let key = match self.flavor {
            OpenAiFlavor::Compatible { .. } => self.cfg.api_key().await?,
            _ => Some(self.cfg.require_api_key().await?),
        };
        crate::audio::openai_speech(&self.cfg, key, request).await
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        let body = build_body(&request, self.flavor);
        let rb = self.authed(self.cfg.client.post(self.cfg.url("chat/completions")).json(&body)).await?;
        let idle = self.cfg.idle_timeout;
        let prompt_chars = request.prompt_chars();
        Ok(drive(cancel, move |em| async move {
            let resp = http::send_with_header_timeout(rb, idle).await?;
            run_stream(resp, em, idle, prompt_chars).await
        }))
    }
}

/// Default display name for a flavor.
#[must_use]
pub fn default_name(flavor: OpenAiFlavor) -> &'static str {
    match flavor {
        OpenAiFlavor::OpenAi => kind_info(ProviderKind::OpenAi).display_name,
        OpenAiFlavor::DeepSeek => kind_info(ProviderKind::DeepSeek).display_name,
        OpenAiFlavor::Compatible { .. } => kind_info(ProviderKind::OpenAiCompatible).display_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolCallPart, ToolSpec};

    #[test]
    fn body_maps_messages_tools_and_schema() {
        let mut req = ChatRequest::new(
            "gpt-4o",
            vec![
                ChatMessage::system("sys"),
                ChatMessage::user("hi"),
                ChatMessage::assistant_tool_calls(
                    "",
                    vec![ToolCallPart { id: "c1".into(), name: "f".into(), arguments: json!({"a":1}) }],
                ),
                ChatMessage::tool_result("c1", "ok", false),
            ],
        );
        req.tools = vec![ToolSpec { name: "f".into(), description: "d".into(), parameters: json!({"type":"object"}) }];
        req.response_format = ResponseFormat::JsonSchema { name: "plan".into(), schema: json!({"type":"object"}) };
        req.max_output_tokens = Some(100);
        let b = build_body(&req, OpenAiFlavor::OpenAi);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(b["messages"][2]["content"], Value::Null);
        assert_eq!(b["messages"][3]["tool_call_id"], "c1");
        assert_eq!(b["tools"][0]["function"]["name"], "f");
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["max_completion_tokens"], 100);
        let d = build_body(&req, OpenAiFlavor::DeepSeek);
        assert_eq!(d["response_format"]["type"], "json_object");
        assert_eq!(d["max_tokens"], 100);
    }

    #[test]
    fn decoder_assembles_tool_calls_and_drops_reasoning() {
        let mut d = ChatCompletionsDecoder::default();
        let mut all = Vec::new();
        for chunk in [
            r#"{"choices":[{"delta":{"reasoning_content":"secret thoughts"}}]}"#,
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"write_file","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.txt\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":2}}}"#,
            "[DONE]",
        ] {
            all.extend(d.data(chunk).unwrap());
        }
        assert!(d.is_done());
        all.extend(d.finish(0));
        assert_eq!(all[0], ChatEvent::TextDelta("Hel".into()));
        assert_eq!(
            all[2],
            ChatEvent::ToolCall { id: "call_1".into(), name: "write_file".into(), arguments: json!({"path":"a.txt"}) }
        );
        assert!(matches!(
            all[3],
            ChatEvent::Usage(Usage { input_tokens: 10, output_tokens: 5, cached_input_tokens: 2, .. })
        ));
        assert_eq!(all[4], ChatEvent::Done { stop_reason: StopReason::ToolUse });
        assert!(!format!("{all:?}").contains("secret thoughts"));
    }

    #[test]
    fn decoder_keeps_parallel_calls_apart_without_index() {
        let mut d = ChatCompletionsDecoder::default();
        let mut all = Vec::new();
        for chunk in [
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"\"x\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"name":"read_file","arguments":"{\"path\":\"y\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ] {
            all.extend(d.data(chunk).unwrap());
        }
        assert_eq!(
            all,
            vec![
                ChatEvent::ToolCall { id: "a".into(), name: "read_file".into(), arguments: json!({"path":"x"}) },
                ChatEvent::ToolCall { id: "b".into(), name: "read_file".into(), arguments: json!({"path":"y"}) },
            ]
        );
        // Several index-less calls in one chunk (each with its id) stay apart too.
        let mut d = ChatCompletionsDecoder::default();
        let out = d
            .data(r#"{"choices":[{"delta":{"tool_calls":[{"id":"c","function":{"name":"f","arguments":"{}"}},{"id":"e","function":{"name":"g","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[1], ChatEvent::ToolCall { id, name, .. } if id == "e" && name == "g"));
    }

    #[test]
    fn decoder_estimates_usage_when_absent() {
        let mut d = ChatCompletionsDecoder::default();
        d.data(r#"{"choices":[{"delta":{"content":"abcdefgh"},"finish_reason":"stop"}]}"#).unwrap();
        let fin = d.finish(40);
        assert!(matches!(fin[0], ChatEvent::Usage(Usage { input_tokens: 10, output_tokens: 2, estimated: true, .. })));
    }
}
