//! Anthropic Messages API (streaming SSE, tool_use).
//!
//! Wire API pinned: `POST {base}/v1/messages`, header
//! `anthropic-version: 2023-06-01`. Thinking blocks are dropped (no-CoT rule).
//! Structured output is implemented with a forced tool whose input schema is
//! the requested JSON schema; its input is surfaced as text.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderId, ProviderKind};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::catalog::default_models;
use crate::http::{self, drive, Emitter, HttpConfig, SseParser, PROBE_TIMEOUT};
use crate::pricing::pricing_for;
use crate::types::{
    parse_tool_arguments, AuthStatus, ChatEvent, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality,
    ProviderDescriptor, ProviderError, ReasoningLevel, ResponseFormat, Role, StopReason, ToolChoice, Usage,
};

/// Pinned API version.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Name of the synthetic tool used for structured output.
pub const STRUCTURED_TOOL: &str = "structured_output";
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Anthropic adapter.
#[derive(Debug)]
pub struct AnthropicAdapter {
    cfg: HttpConfig,
}

impl AnthropicAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(cfg: HttpConfig) -> Self {
        Self { cfg }
    }

    async fn authed(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, ProviderError> {
        let key = self.cfg.require_api_key().await?;
        Ok(self.cfg.with_headers(rb).header("x-api-key", key).header("anthropic-version", ANTHROPIC_VERSION))
    }

    async fn fetch_models(&self) -> Result<Vec<Model>, ProviderError> {
        let rb = self.authed(self.cfg.client.get(self.cfg.url("v1/models?limit=100")).timeout(PROBE_TIMEOUT)).await?;
        let body: Value = http::send(rb).await?.json().await.map_err(http::map_reqwest_error)?;
        let defaults = default_models(ProviderKind::Anthropic, self.cfg.id, false);
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?;
                        let name = m.get("display_name").and_then(Value::as_str).unwrap_or(id);
                        Some(defaults.iter().find(|d| d.id == id).cloned().unwrap_or_else(|| Model {
                            provider_id: self.cfg.id,
                            id: id.to_owned(),
                            display_name: name.to_owned(),
                            context_window: 200_000,
                            max_output_tokens: Some(DEFAULT_MAX_TOKENS),
                            capabilities: ModelCapabilities {
                                tools: true,
                                vision: true,
                                structured_output: true,
                                streaming: true,
                                reasoning: true,
                                coding_agent: false,
                            },
                            pricing: pricing_for(id),
                            is_local: false,
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

fn push_block(messages: &mut Vec<Value>, role: &str, block: Value) {
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            if let Some(arr) = last["content"].as_array_mut() {
                arr.push(block);
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": [block]}));
}

/// Builds the request body (pure; unit-tested). Consecutive same-role
/// messages are merged (tool results become user turns).
#[must_use]
pub fn build_body(req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    for m in req.conversation() {
        match m.role {
            Role::User => {
                for img in &m.images {
                    push_block(
                        &mut messages,
                        "user",
                        json!({"type": "image", "source": {"type": "base64", "media_type": img.media_type, "data": img.data_base64}}),
                    );
                }
                if !m.content.is_empty() || m.images.is_empty() {
                    push_block(&mut messages, "user", json!({"type": "text", "text": m.content}));
                }
            }
            Role::Assistant => {
                if !m.content.is_empty() {
                    push_block(&mut messages, "assistant", json!({"type": "text", "text": m.content}));
                }
                for c in &m.tool_calls {
                    push_block(
                        &mut messages,
                        "assistant",
                        json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments}),
                    );
                }
            }
            Role::Tool => push_block(
                &mut messages,
                "user",
                json!({"type": "tool_result", "tool_use_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content, "is_error": m.is_error}),
            ),
            Role::System => {}
        }
    }
    let max_tokens = req.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = json!({"model": req.model, "messages": messages, "max_tokens": max_tokens, "stream": true});
    if let Some(sys) = req.system_prompt() {
        body["system"] = json!(sys);
    }
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
        .collect();
    let mut tool_choice = match &req.tool_choice {
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::None => json!({"type": "none"}),
        ToolChoice::Required => json!({"type": "any"}),
        ToolChoice::Specific(n) => json!({"type": "tool", "name": n}),
    };
    let structured = match &req.response_format {
        ResponseFormat::JsonSchema { schema, .. } if req.tools.is_empty() => Some(schema.clone()),
        ResponseFormat::JsonObject if req.tools.is_empty() => Some(json!({"type": "object"})),
        _ => None,
    };
    if let Some(schema) = structured {
        tools.push(json!({"name": STRUCTURED_TOOL, "description": "Return the final answer as structured JSON.", "input_schema": schema}));
        tool_choice = json!({"type": "tool", "name": STRUCTURED_TOOL});
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = tool_choice;
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    // Extended thinking only without tools: with tools Anthropic requires the
    // signed thinking blocks to be echoed back, which Phase 1 does not persist.
    if let (Some(level), true) = (req.reasoning, body.get("tools").is_none()) {
        let budget: u32 = match level {
            ReasoningLevel::Low => 1024,
            ReasoningLevel::Medium => 4096,
            ReasoningLevel::High => 16_000,
        };
        if max_tokens > budget && req.model.contains("claude") && !req.model.contains("haiku-3") {
            body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
            if let Some(obj) = body.as_object_mut() {
                obj.remove("temperature");
            }
        }
    }
    if !req.stop.is_empty() {
        body["stop_sequences"] = json!(req.stop);
    }
    body
}

enum Block {
    Text,
    Tool { id: String, name: String, json: String },
    Ignored,
}

/// Incremental Messages SSE decoder (pure; unit-tested).
#[derive(Default)]
pub struct MessagesDecoder {
    blocks: BTreeMap<u64, Block>,
    usage: Usage,
    saw_usage: bool,
    stop: Option<StopReason>,
    finished: bool,
}

impl MessagesDecoder {
    /// Decodes one SSE event.
    pub fn event(&mut self, event: &str, data: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        let mut out = Vec::new();
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => return Err(ProviderError::Server(format!("malformed stream event: {e}"))),
        };
        let kind = if event.is_empty() { v.get("type").and_then(Value::as_str).unwrap_or_default() } else { event };
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.merge_usage(u);
                }
            }
            "content_block_start" => {
                let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                let b = match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(ChatEvent::TextDelta(t.to_owned()));
                        }
                        Block::Text
                    }
                    Some("tool_use") => Block::Tool {
                        id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        json: String::new(),
                    },
                    // thinking / redacted_thinking / server tools: dropped.
                    _ => Block::Ignored,
                };
                self.blocks.insert(index, b);
            }
            "content_block_delta" => {
                let delta = v.get("delta").cloned().unwrap_or(Value::Null);
                match (self.blocks.get_mut(&index), delta.get("type").and_then(Value::as_str)) {
                    (Some(Block::Text), Some("text_delta")) => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(ChatEvent::TextDelta(t.to_owned()));
                        }
                    }
                    (Some(Block::Tool { json, .. }), Some("input_json_delta")) => {
                        json.push_str(delta.get("partial_json").and_then(Value::as_str).unwrap_or_default());
                    }
                    _ => {} // thinking_delta / signature_delta: dropped
                }
            }
            "content_block_stop" => {
                if let Some(Block::Tool { id, name, json }) = self.blocks.remove(&index) {
                    let arguments = parse_tool_arguments(&json);
                    if name == STRUCTURED_TOOL {
                        out.push(ChatEvent::TextDelta(arguments.to_string()));
                    } else {
                        out.push(ChatEvent::ToolCall { id, name, arguments });
                    }
                }
            }
            "message_delta" => {
                if let Some(u) = v.get("usage") {
                    self.merge_usage(u);
                }
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(match r {
                        "end_turn" => StopReason::EndTurn,
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::MaxTokens,
                        "stop_sequence" => StopReason::StopSequence,
                        "refusal" => StopReason::ContentFilter,
                        other => StopReason::Other(other.to_owned()),
                    });
                }
            }
            "message_stop" => self.finished = true,
            "error" => {
                let et = v.pointer("/error/type").and_then(Value::as_str).unwrap_or_default();
                let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("stream error").to_owned();
                return Err(match et {
                    "rate_limit_error" => ProviderError::RateLimited { retry_after_secs: None },
                    "authentication_error" | "permission_error" => ProviderError::Auth(msg),
                    "invalid_request_error" if msg.to_ascii_lowercase().contains("credit balance") => {
                        ProviderError::QuotaExceeded(msg)
                    }
                    "invalid_request_error" => ProviderError::BadRequest(msg),
                    _ => ProviderError::Server(format!("{et}: {msg}")),
                });
            }
            _ => {} // ping and unknown events
        }
        Ok(out)
    }

    fn merge_usage(&mut self, u: &Value) {
        let get = |k: &str| u.get(k).and_then(Value::as_u64);
        self.saw_usage = true;
        let cache_read = get("cache_read_input_tokens");
        let cache_write = get("cache_creation_input_tokens");
        if let Some(i) = get("input_tokens") {
            self.usage.input_tokens = i + cache_read.unwrap_or(0) + cache_write.unwrap_or(0);
        }
        if let Some(c) = cache_read {
            self.usage.cached_input_tokens = c;
        }
        if let Some(c) = cache_write {
            self.usage.cache_write_tokens = c;
        }
        if let Some(o) = get("output_tokens") {
            self.usage.output_tokens = o;
        }
    }

    /// Whether `message_stop` arrived.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Final events.
    pub fn finish(&mut self) -> Result<Vec<ChatEvent>, ProviderError> {
        if !self.finished {
            return Err(ProviderError::Server("stream ended before message_stop".into()));
        }
        let mut out = Vec::new();
        if self.saw_usage {
            out.push(ChatEvent::Usage(self.usage));
        }
        out.push(ChatEvent::Done { stop_reason: self.stop.clone().unwrap_or(StopReason::EndTurn) });
        Ok(out)
    }
}

async fn run_stream(
    resp: reqwest::Response,
    em: Emitter<ChatEvent>,
    idle: std::time::Duration,
    structured: bool,
) -> Result<(), ProviderError> {
    let mut body = resp.bytes_stream();
    let mut sse = SseParser::default();
    let mut dec = MessagesDecoder::default();
    loop {
        let chunk = http::next_chunk(&mut body, idle).await?;
        let events = match &chunk {
            Some(b) => sse.push(b),
            None => sse.finish(),
        };
        for ev in events {
            for out in dec.event(&ev.event, &ev.data)? {
                em.emit(out).await?;
            }
        }
        if chunk.is_none() || dec.is_finished() {
            break;
        }
    }
    for mut out in dec.finish()? {
        // A forced structured-output tool is a normal end of turn for the caller.
        if structured {
            if let ChatEvent::Done { stop_reason } = &mut out {
                if *stop_reason == StopReason::ToolUse {
                    *stop_reason = StopReason::EndTurn;
                }
            }
        }
        em.emit(out).await?;
    }
    Ok(())
}

#[async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn id(&self) -> ProviderId {
        self.cfg.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Anthropic,
            display_name: self.cfg.name.clone(),
            is_local: false,
            locality: Locality::Cloud,
            capabilities: ModelCapabilities {
                tools: true,
                vision: true,
                structured_output: true,
                streaming: true,
                reasoning: true,
                coding_agent: false,
            },
            max_context_tokens: 200_000,
            coding_agent: false,
            cost_model: CostModelKind::PerToken,
            max_concurrency: 4,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        match self.cfg.api_key().await {
            Ok(Some(_)) => AuthStatus::Unverified { label: "API key".into() },
            Ok(None) => AuthStatus::NotConfigured,
            Err(e) => AuthStatus::Invalid { reason: e.to_string() },
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        let m = self.fetch_models().await?;
        Ok(if m.is_empty() { default_models(ProviderKind::Anthropic, self.cfg.id, false) } else { m })
    }

    async fn health(&self) -> HealthReport {
        let start = Instant::now();
        let r = self.fetch_models().await;
        let ms = Some(start.elapsed().as_millis() as u64);
        match r {
            Ok(m) => HealthReport::healthy(ms, Some(format!("{} models", m.len()))),
            Err(e) => HealthReport::from_error(&e, ms),
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        let body = build_body(&request);
        let structured = body["tool_choice"]["name"] == STRUCTURED_TOOL;
        let rb = self.authed(self.cfg.client.post(self.cfg.url("v1/messages")).json(&body)).await?;
        let idle = self.cfg.idle_timeout;
        Ok(drive(cancel, move |em| async move {
            let resp = http::send(rb).await?;
            run_stream(resp, em, idle, structured).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolCallPart, ToolSpec};

    #[test]
    fn body_merges_tool_results_and_sets_system() {
        let mut req = ChatRequest::new(
            "claude-sonnet-4-5",
            vec![
                ChatMessage::system("sys"),
                ChatMessage::user("hi"),
                ChatMessage::assistant_tool_calls(
                    "let me",
                    vec![
                        ToolCallPart { id: "t1".into(), name: "f".into(), arguments: json!({}) },
                        ToolCallPart { id: "t2".into(), name: "f".into(), arguments: json!({}) },
                    ],
                ),
                ChatMessage::tool_result("t1", "a", false),
                ChatMessage::tool_result("t2", "b", true),
            ],
        );
        req.tools = vec![ToolSpec { name: "f".into(), description: "d".into(), parameters: json!({"type":"object"}) }];
        let b = build_body(&req);
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"].as_array().unwrap().len(), 3);
        assert_eq!(b["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(b["messages"][2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(b["messages"][2]["content"][1]["is_error"], true);
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert_eq!(b["max_tokens"], 8192);
    }

    #[test]
    fn structured_output_uses_forced_tool() {
        let mut req = ChatRequest::new("claude-x", vec![ChatMessage::user("plan")]);
        req.response_format = ResponseFormat::JsonSchema {
            name: "plan".into(),
            schema: json!({"type":"object","properties":{"a":{"type":"string"}}}),
        };
        let b = build_body(&req);
        assert_eq!(b["tool_choice"]["name"], STRUCTURED_TOOL);
        let mut d = MessagesDecoder::default();
        let mut out = Vec::new();
        out.extend(
            d.event(
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"tool_use","id":"x","name":"structured_output","input":{}}}"#,
            )
            .unwrap(),
        );
        out.extend(
            d.event(
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\":\"b\"}"}}"#,
            )
            .unwrap(),
        );
        out.extend(d.event("content_block_stop", r#"{"index":0}"#).unwrap());
        assert_eq!(out, vec![ChatEvent::TextDelta("{\"a\":\"b\"}".into())]);
    }

    #[test]
    fn decoder_drops_thinking_and_reports_usage() {
        let mut d = MessagesDecoder::default();
        let mut out = Vec::new();
        let evs = [
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":12,"cache_read_input_tokens":3,"output_tokens":1}}}"#,
            ),
            ("content_block_start", r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#),
            ("content_block_delta", r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"hidden chain"}}"#),
            ("content_block_stop", r#"{"index":0}"#),
            ("content_block_start", r#"{"index":1,"content_block":{"type":"text","text":""}}"#),
            ("content_block_delta", r#"{"index":1,"delta":{"type":"text_delta","text":"Hi"}}"#),
            ("ping", "{}"),
            ("message_delta", r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#),
            ("message_stop", "{}"),
        ];
        for (e, data) in evs {
            out.extend(d.event(e, data).unwrap());
        }
        out.extend(d.finish().unwrap());
        assert!(!format!("{out:?}").contains("hidden chain"));
        assert_eq!(out[0], ChatEvent::TextDelta("Hi".into()));
        assert!(matches!(
            out[1],
            ChatEvent::Usage(Usage { input_tokens: 15, output_tokens: 7, cached_input_tokens: 3, .. })
        ));
        assert_eq!(out[2], ChatEvent::Done { stop_reason: StopReason::EndTurn });
    }

    #[test]
    fn stream_error_event_is_classified() {
        let mut d = MessagesDecoder::default();
        let e = d
            .event("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
            .unwrap_err();
        assert!(matches!(e, ProviderError::Server(_)));
        let e =
            d.event("error", r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#).unwrap_err();
        assert!(matches!(e, ProviderError::RateLimited { .. }));
        assert!(MessagesDecoder::default().finish().is_err(), "truncated stream is a server error");
    }
}
