//! Anthropic Messages API (streaming SSE, tool_use).
//!
//! Wire API pinned: `POST {base}/v1/messages`, header
//! `anthropic-version: 2023-06-01`. Thinking blocks are dropped (no-CoT rule).
//! Structured output is implemented with a synthetic tool whose input schema
//! is the requested JSON schema; its input is surfaced as text. The tool is
//! forced on models that accept forced tool use and requested by prompt
//! (`tool_choice: auto`) on those that reject it (see `ModelQuirks`).

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

/// Claude model family parsed from a model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Opus,
    Sonnet,
    Haiku,
    Fable,
    Mythos,
    Other,
}

/// Parsed Claude model id: family plus `major * 100 + minor`
/// (`claude-opus-4-5` -> 405, `claude-3-7-sonnet` -> 307, `claude-opus-5` -> 500).
fn parse_claude(model: &str) -> Option<(Family, u32)> {
    let id = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    let start = id.find("claude-")?;
    let id = &id[start + "claude-".len()..];
    let id = id.split(['@', '[', ':']).next().unwrap_or(id);
    let parts: Vec<&str> = id.split('-').collect();
    let num = |s: Option<&&str>| s.and_then(|s| s.parse::<u32>().ok());
    // A number above 99 is a snapshot date, not a minor version.
    let minor = |s: Option<&&str>| num(s).filter(|n| *n < 100);
    let family = |s: &str| match s {
        "opus" => Family::Opus,
        "sonnet" => Family::Sonnet,
        "haiku" => Family::Haiku,
        "fable" => Family::Fable,
        "mythos" => Family::Mythos,
        _ => Family::Other,
    };
    if let Some(major) = num(parts.first()) {
        // Legacy naming: claude-3-5-sonnet, claude-3-haiku.
        let (minor, fam_at) = match minor(parts.get(1)) {
            Some(m) => (m, 2),
            None => (0, 1),
        };
        let fam = parts.get(fam_at).map_or(Family::Other, |s| family(s));
        return Some((fam, major * 100 + minor));
    }
    let fam = family(parts.first()?);
    let major = num(parts.get(1))?;
    Some((fam, major * 100 + minor(parts.get(2)).unwrap_or(0)))
}

/// How extended thinking is requested from a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingMode {
    /// No extended thinking (or not known for this id): send nothing.
    Unsupported,
    /// Pre-4.6 models: `{type: "enabled", budget_tokens}`.
    Budget,
    /// 4.6+ / 5.x / Fable: `{type: "adaptive"}` + `output_config.effort`
    /// (`budget_tokens` is a 400 on Opus 4.7+, Sonnet 5+ and Fable).
    Adaptive,
}

/// Request-shape differences between Claude generations, so one body builder
/// never sends a parameter the target model rejects with a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModelQuirks {
    /// `tool_choice` `any` / `tool` is accepted (a 400 on Opus 5.5,
    /// Sonnet 5.5, Fable 5.1, Mythos 5.1; assumed rejected on newer releases).
    forced_tool_choice: bool,
    thinking: ThinkingMode,
    /// `temperature` is accepted (removed on Opus 4.7+, Sonnet 5+, Fable).
    sampling: bool,
}

impl ModelQuirks {
    fn of(model: &str) -> Self {
        let Some((fam, v)) = parse_claude(model) else {
            // Not a recognisable Claude id (e.g. a proxy alias): keep forced
            // tools and sampling, but never send thinking we cannot vouch for.
            return Self { forced_tool_choice: true, thinking: ThinkingMode::Unsupported, sampling: true };
        };
        // A generation newer than this table follows the newest known rules.
        let future = v >= 600;
        let forced_tool_choice = !future
            && match fam {
                Family::Opus | Family::Sonnet => v < 505,
                Family::Fable | Family::Mythos => v < 501,
                Family::Haiku | Family::Other => true,
            };
        let thinking = match fam {
            _ if future => ThinkingMode::Adaptive,
            Family::Fable | Family::Mythos => ThinkingMode::Adaptive,
            Family::Opus | Family::Sonnet if v >= 406 => ThinkingMode::Adaptive,
            _ if v >= 307 => ThinkingMode::Budget,
            _ => ThinkingMode::Unsupported,
        };
        let sampling = !future
            && match fam {
                Family::Opus => v < 407,
                Family::Sonnet => v < 500,
                Family::Fable | Family::Mythos => false,
                Family::Haiku | Family::Other => true,
            };
        Self { forced_tool_choice, thinking, sampling }
    }
}

/// Whether the body asks for structured output through the synthetic tool.
fn is_structured(body: &Value) -> bool {
    body["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == STRUCTURED_TOOL))
}

/// Builds the request body (pure; unit-tested). Consecutive same-role
/// messages are merged (tool results become user turns).
#[must_use]
pub fn build_body(req: &ChatRequest) -> Value {
    let quirks = ModelQuirks::of(&req.model);
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
    // Instructions that replace a forced `tool_choice` on models rejecting it.
    let mut steer: Vec<String> = Vec::new();
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
        .collect();
    let mut tool_choice = match &req.tool_choice {
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::None => json!({"type": "none"}),
        ToolChoice::Required if quirks.forced_tool_choice => json!({"type": "any"}),
        ToolChoice::Required => {
            steer.push("You must answer by calling one of the provided tools.".to_owned());
            json!({"type": "auto"})
        }
        ToolChoice::Specific(n) if quirks.forced_tool_choice => json!({"type": "tool", "name": n}),
        ToolChoice::Specific(n) => {
            steer.push(format!("You must answer by calling the `{n}` tool."));
            json!({"type": "auto"})
        }
    };
    let structured = match &req.response_format {
        ResponseFormat::JsonSchema { schema, .. } if req.tools.is_empty() => Some(schema.clone()),
        ResponseFormat::JsonObject if req.tools.is_empty() => Some(json!({"type": "object"})),
        _ => None,
    };
    if let Some(schema) = structured {
        tools.push(json!({"name": STRUCTURED_TOOL, "description": "Return the final answer as structured JSON.", "input_schema": schema}));
        if quirks.forced_tool_choice {
            tool_choice = json!({"type": "tool", "name": STRUCTURED_TOOL});
        } else {
            // Forced tool use is a 400 on this model: request the call by
            // prompt. A plain-text JSON answer still reaches the caller's
            // JSON extraction / repair loop.
            steer.push(format!(
                "Give your final answer by calling the `{STRUCTURED_TOOL}` tool exactly once with the complete \
                 answer as its input. Do not answer in prose."
            ));
            tool_choice = json!({"type": "auto", "disable_parallel_tool_use": true});
        }
    }
    let system = match (req.system_prompt(), steer.is_empty()) {
        (Some(sys), true) => Some(sys.to_owned()),
        (Some(sys), false) => Some(format!("{sys}\n\n{}", steer.join("\n"))),
        (None, false) => Some(steer.join("\n")),
        (None, true) => None,
    };
    if let Some(sys) = system {
        body["system"] = json!(sys);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = tool_choice;
    }
    if let (Some(t), true) = (req.temperature, quirks.sampling) {
        body["temperature"] = json!(t);
    }
    // Extended thinking only without tools: with tools Anthropic requires the
    // signed thinking blocks to be echoed back, which Phase 1 does not persist.
    if let (Some(level), true) = (req.reasoning, body.get("tools").is_none()) {
        match quirks.thinking {
            ThinkingMode::Budget => {
                let budget: u32 = match level {
                    ReasoningLevel::Low => 1024,
                    ReasoningLevel::Medium => 4096,
                    ReasoningLevel::High => 16_000,
                };
                if max_tokens > budget {
                    body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
                    if let Some(obj) = body.as_object_mut() {
                        obj.remove("temperature");
                    }
                }
            }
            ThinkingMode::Adaptive => {
                let effort = match level {
                    ReasoningLevel::Low => "low",
                    ReasoningLevel::Medium => "medium",
                    ReasoningLevel::High => "high",
                };
                body["thinking"] = json!({"type": "adaptive"});
                body["output_config"] = json!({"effort": effort});
                if let Some(obj) = body.as_object_mut() {
                    obj.remove("temperature");
                }
            }
            ThinkingMode::Unsupported => {}
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
        let structured = is_structured(&body);
        let rb = self.authed(self.cfg.client.post(self.cfg.url("v1/messages")).json(&body)).await?;
        let idle = self.cfg.idle_timeout;
        Ok(drive(cancel, move |em| async move {
            let resp = http::send_with_header_timeout(rb, idle).await?;
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

    fn structured_req(model: &str) -> ChatRequest {
        let mut req = ChatRequest::new(model, vec![ChatMessage::system("sys"), ChatMessage::user("plan")]);
        req.response_format = ResponseFormat::JsonSchema { name: "plan".into(), schema: json!({"type":"object"}) };
        req
    }

    #[test]
    fn structured_output_is_not_forced_on_models_rejecting_forced_tools() {
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-1", "claude-mythos-5-1", "claude-opus-6"]
        {
            let b = build_body(&structured_req(model));
            assert_eq!(b["tool_choice"]["type"], "auto", "{model}");
            assert!(is_structured(&b), "{model}");
            assert!(b["system"].as_str().unwrap().starts_with("sys\n\n"), "{model}");
            assert!(b["system"].as_str().unwrap().contains(STRUCTURED_TOOL), "{model}");
        }
        // Models that accept forced tool use keep the guarantee.
        for model in ["claude-opus-5", "claude-fable-5", "claude-sonnet-4-5", "claude-haiku-4-5", "claude-opus-4-8"] {
            let b = build_body(&structured_req(model));
            assert_eq!(b["tool_choice"], json!({"type": "tool", "name": STRUCTURED_TOOL}), "{model}");
            assert_eq!(b["system"], "sys", "{model}");
        }
    }

    #[test]
    fn required_and_specific_tool_choice_map_to_auto_on_new_models() {
        let mut req = ChatRequest::new("claude-sonnet-5-5", vec![ChatMessage::user("go")]);
        req.tools = vec![ToolSpec { name: "f".into(), description: "d".into(), parameters: json!({"type":"object"}) }];
        req.tool_choice = ToolChoice::Required;
        let b = build_body(&req);
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert!(b["system"].as_str().unwrap().contains("calling one of the provided tools"));
        req.tool_choice = ToolChoice::Specific("f".into());
        let b = build_body(&req);
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert!(b["system"].as_str().unwrap().contains("`f`"));
        req.model = "claude-sonnet-4-5".into();
        assert_eq!(build_body(&req)["tool_choice"], json!({"type": "tool", "name": "f"}));
        req.tool_choice = ToolChoice::Required;
        assert_eq!(build_body(&req)["tool_choice"], json!({"type": "any"}));
    }

    #[test]
    fn thinking_uses_adaptive_on_new_models_and_budget_on_old() {
        let req = |model: &str| {
            let mut r = ChatRequest::new(model, vec![ChatMessage::user("hi")]);
            r.reasoning = Some(ReasoningLevel::High);
            r.temperature = Some(0.2);
            r.max_output_tokens = Some(32_000);
            r
        };
        for model in ["claude-opus-5-5", "claude-opus-4-7", "claude-opus-4-6", "claude-sonnet-5", "claude-fable-5-1"] {
            let b = build_body(&req(model));
            assert_eq!(b["thinking"], json!({"type": "adaptive"}), "{model}");
            assert_eq!(b["output_config"]["effort"], "high", "{model}");
            assert!(b.get("temperature").is_none(), "{model}");
            assert!(!b.to_string().contains("budget_tokens"), "{model}");
        }
        for model in ["claude-sonnet-4-5", "claude-haiku-4-5", "claude-opus-4-1", "claude-3-7-sonnet-20250219"] {
            let b = build_body(&req(model));
            assert_eq!(b["thinking"]["budget_tokens"], 16_000, "{model}");
            assert!(b.get("output_config").is_none(), "{model}");
        }
        // No thinking for models without it.
        assert!(build_body(&req("claude-3-5-haiku")).get("thinking").is_none());
        assert!(build_body(&req("claude-3-haiku-20240307")).get("thinking").is_none());
    }

    #[test]
    fn temperature_is_dropped_where_sampling_was_removed() {
        let req = |model: &str| {
            let mut r = ChatRequest::new(model, vec![ChatMessage::user("hi")]);
            r.temperature = Some(0.3);
            r
        };
        for model in ["claude-opus-4-7", "claude-opus-5-5", "claude-sonnet-5", "claude-sonnet-5-5", "claude-fable-5"] {
            assert!(build_body(&req(model)).get("temperature").is_none(), "{model}");
        }
        for model in ["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5", "my-proxy-alias"] {
            assert!(build_body(&req(model)).get("temperature").is_some(), "{model}");
        }
    }

    #[test]
    fn parses_claude_ids() {
        assert_eq!(parse_claude("claude-opus-4-5"), Some((Family::Opus, 405)));
        assert_eq!(parse_claude("claude-opus-4-20250514"), Some((Family::Opus, 400)));
        assert_eq!(parse_claude("claude-sonnet-4-5-20250929"), Some((Family::Sonnet, 405)));
        assert_eq!(parse_claude("claude-opus-5-5"), Some((Family::Opus, 505)));
        assert_eq!(parse_claude("claude-3-7-sonnet-latest"), Some((Family::Sonnet, 307)));
        assert_eq!(parse_claude("claude-3-haiku-20240307"), Some((Family::Haiku, 300)));
        assert_eq!(parse_claude("us.anthropic.claude-sonnet-5-5"), Some((Family::Sonnet, 505)));
        assert_eq!(parse_claude("claude-opus-4-6[1m]"), Some((Family::Opus, 406)));
        assert_eq!(parse_claude("gpt-4o"), None);
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
