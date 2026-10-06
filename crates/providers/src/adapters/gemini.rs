//! Google Gemini API (`streamGenerateContent?alt=sse`, function calling).
//!
//! Key travels in the `x-goog-api-key` header — never in the URL, so it cannot
//! leak into logs. Thought parts are dropped (no-CoT rule), but the opaque
//! `thoughtSignature` of a function-call part is kept (in memory, keyed by
//! call id) and echoed back on replay: Gemini 3 rejects a function-calling
//! turn whose earlier `functionCall` lost its signature.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderId, ProviderKind};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::catalog::default_models;
use crate::http::{self, drive, Emitter, HttpConfig, SseParser, PROBE_TIMEOUT};
use crate::pricing::pricing_for;
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality, ProviderDescriptor,
    ProviderError, ReasoningLevel, ResponseFormat, Role, StopReason, ToolChoice, Usage,
};

/// Documented placeholder signature for function calls whose real signature
/// is unknown (history from another provider or a previous app session).
pub const DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";
/// Signatures kept per adapter (oldest evicted first).
const MAX_SIGNATURES: usize = 4096;

/// Bounded `call id -> thoughtSignature` map.
#[derive(Debug, Default)]
pub struct SignatureCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl SignatureCache {
    /// Records a signature.
    pub fn insert(&mut self, id: String, signature: String) {
        if self.map.insert(id.clone(), signature).is_none() {
            self.order.push_back(id);
            while self.order.len() > MAX_SIGNATURES {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }

    /// Signature of a call.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&str> {
        self.map.get(id).map(String::as_str)
    }
}

/// Gemini adapter.
#[derive(Debug)]
pub struct GeminiAdapter {
    cfg: HttpConfig,
    signatures: Arc<Mutex<SignatureCache>>,
}

impl GeminiAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(cfg: HttpConfig) -> Self {
        Self { cfg, signatures: Arc::default() }
    }

    async fn authed(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, ProviderError> {
        let key = self.cfg.require_api_key().await?;
        Ok(self.cfg.with_headers(rb).header("x-goog-api-key", key))
    }

    async fn fetch_models(&self) -> Result<Vec<Model>, ProviderError> {
        let rb =
            self.authed(self.cfg.client.get(self.cfg.url("v1beta/models?pageSize=200")).timeout(PROBE_TIMEOUT)).await?;
        let body: Value = http::send(rb).await?.json().await.map_err(http::map_reqwest_error)?;
        Ok(body
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|m| {
                        m.get("supportedGenerationMethods")
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
                    })
                    .filter_map(|m| {
                        let id = m.get("name")?.as_str()?.trim_start_matches("models/").to_owned();
                        let name = m.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_owned();
                        Some(Model {
                            provider_id: self.cfg.id,
                            pricing: pricing_for(&id),
                            display_name: name,
                            context_window: m.get("inputTokenLimit").and_then(Value::as_u64).unwrap_or(1_048_576)
                                as u32,
                            max_output_tokens: m.get("outputTokenLimit").and_then(Value::as_u64).map(|v| v as u32),
                            capabilities: ModelCapabilities {
                                tools: true,
                                vision: true,
                                structured_output: true,
                                streaming: true,
                                reasoning: m.get("thinking").and_then(Value::as_bool).unwrap_or(id.contains("2.5")),
                                coding_agent: false,
                            },
                            is_local: false,
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Removes JSON-schema keys Gemini rejects in function declarations.
fn sanitize_schema(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| {
                    !matches!(k.as_str(), "$schema" | "additionalProperties" | "$id" | "$defs" | "definitions")
                })
                .map(|(k, v)| (k.clone(), sanitize_schema(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

/// Whether the model validates thought signatures on function-calling turns
/// (Gemini 3 and later).
fn requires_thought_signatures(model: &str) -> bool {
    let id = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    id.strip_prefix("gemini-")
        .and_then(|rest| rest.split(['.', '-']).next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 3)
}

/// Builds the request body without known signatures (pure; unit-tested).
#[must_use]
pub fn build_body(req: &ChatRequest) -> Value {
    build_body_with(req, &|_| None)
}

/// Builds the request body; `signature` returns the `thoughtSignature`
/// recorded for a function call id.
#[must_use]
pub fn build_body_with(req: &ChatRequest, signature: &dyn Fn(&str) -> Option<String>) -> Value {
    let needs_signatures = requires_thought_signatures(&req.model);
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut contents: Vec<Value> = Vec::new();
    let push = |contents: &mut Vec<Value>, role: &str, part: Value| {
        if let Some(last) = contents.last_mut() {
            if last["role"] == role {
                if let Some(parts) = last["parts"].as_array_mut() {
                    parts.push(part);
                    return;
                }
            }
        }
        contents.push(json!({"role": role, "parts": [part]}));
    };
    for m in req.conversation() {
        match m.role {
            Role::User => {
                for img in &m.images {
                    push(
                        &mut contents,
                        "user",
                        json!({"inlineData": {"mimeType": img.media_type, "data": img.data_base64}}),
                    );
                }
                if !m.content.is_empty() || m.images.is_empty() {
                    push(&mut contents, "user", json!({"text": m.content}));
                }
            }
            Role::Assistant => {
                if !m.content.is_empty() {
                    push(&mut contents, "model", json!({"text": m.content}));
                }
                let known: Vec<Option<String>> = m.tool_calls.iter().map(|c| signature(&c.id)).collect();
                // Gemini signs the first call of a turn; when no real signature
                // survived, Gemini 3 needs the documented placeholder there.
                let placeholder = needs_signatures && known.iter().all(Option::is_none);
                for (i, (c, sig)) in m.tool_calls.iter().zip(known).enumerate() {
                    names.insert(c.id.as_str(), c.name.as_str());
                    let mut part = json!({"functionCall": {"name": c.name, "args": c.arguments}});
                    match sig {
                        Some(sig) => part["thoughtSignature"] = json!(sig),
                        None if placeholder && i == 0 => part["thoughtSignature"] = json!(DUMMY_THOUGHT_SIGNATURE),
                        None => {}
                    }
                    push(&mut contents, "model", part);
                }
            }
            Role::Tool => {
                let id = m.tool_call_id.as_deref().unwrap_or_default();
                let name = names.get(id).copied().unwrap_or("tool");
                let response: Value =
                    serde_json::from_str::<Value>(&m.content).ok().filter(Value::is_object).unwrap_or_else(|| {
                        if m.is_error {
                            json!({"error": m.content})
                        } else {
                            json!({"result": m.content})
                        }
                    });
                push(&mut contents, "user", json!({"functionResponse": {"name": name, "response": response}}));
            }
            Role::System => {}
        }
    }
    let mut body = json!({"contents": contents});
    if let Some(sys) = req.system_prompt() {
        body["systemInstruction"] = json!({"parts": [{"text": sys}]});
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(
                |t| json!({"name": t.name, "description": t.description, "parameters": sanitize_schema(&t.parameters)}),
            )
            .collect();
        body["tools"] = json!([{"functionDeclarations": decls}]);
        body["toolConfig"] = match &req.tool_choice {
            ToolChoice::Auto => json!({"functionCallingConfig": {"mode": "AUTO"}}),
            ToolChoice::None => json!({"functionCallingConfig": {"mode": "NONE"}}),
            ToolChoice::Required => json!({"functionCallingConfig": {"mode": "ANY"}}),
            ToolChoice::Specific(n) => json!({"functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": [n]}}),
        };
    }
    let mut gen = serde_json::Map::new();
    if let Some(m) = req.max_output_tokens {
        gen.insert("maxOutputTokens".into(), json!(m));
    }
    if let Some(t) = req.temperature {
        gen.insert("temperature".into(), json!(t));
    }
    if !req.stop.is_empty() {
        gen.insert("stopSequences".into(), json!(req.stop));
    }
    match &req.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject => {
            gen.insert("responseMimeType".into(), json!("application/json"));
        }
        ResponseFormat::JsonSchema { schema, .. } => {
            gen.insert("responseMimeType".into(), json!("application/json"));
            gen.insert("responseJsonSchema".into(), schema.clone());
        }
    }
    let mut thinking = json!({"includeThoughts": false});
    if let Some(level) = req.reasoning {
        thinking["thinkingBudget"] = json!(match level {
            ReasoningLevel::Low => 1024,
            ReasoningLevel::Medium => 8192,
            ReasoningLevel::High => 24_576,
        });
    }
    if req.model.contains("2.5") || req.reasoning.is_some() {
        gen.insert("thinkingConfig".into(), thinking);
    }
    if !gen.is_empty() {
        body["generationConfig"] = Value::Object(gen);
    }
    body
}

/// Unique id for a function call Gemini returned without one (ids must not
/// repeat across turns: thought signatures are keyed by them).
fn generated_call_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("gemini_call_{:x}_{}", chrono::Utc::now().timestamp_millis(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Incremental decoder for `streamGenerateContent` SSE chunks.
#[derive(Default)]
pub struct GeminiDecoder {
    usage: Option<Usage>,
    stop: Option<StopReason>,
    calls: u32,
    signatures: Vec<(String, String)>,
}

impl GeminiDecoder {
    /// Decodes one chunk.
    pub fn data(&mut self, data: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        let v: Value =
            serde_json::from_str(data).map_err(|e| ProviderError::Server(format!("malformed stream chunk: {e}")))?;
        if let Some(err) = v.get("error") {
            let status = err.get("status").and_then(Value::as_str).unwrap_or_default();
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("stream error").to_owned();
            return Err(match status {
                "RESOURCE_EXHAUSTED" => ProviderError::RateLimited { retry_after_secs: None },
                "UNAUTHENTICATED" | "PERMISSION_DENIED" => ProviderError::Auth(msg),
                "INVALID_ARGUMENT" | "NOT_FOUND" | "FAILED_PRECONDITION" => ProviderError::BadRequest(msg),
                _ => ProviderError::Server(msg),
            });
        }
        let mut out = Vec::new();
        if let Some(cand) = v.get("candidates").and_then(Value::as_array).and_then(|c| c.first()) {
            if let Some(parts) = cand.pointer("/content/parts").and_then(Value::as_array) {
                for p in parts {
                    if p.get("thought").and_then(Value::as_bool) == Some(true) {
                        continue; // hidden reasoning: dropped
                    }
                    if let Some(t) = p.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                        out.push(ChatEvent::TextDelta(t.to_owned()));
                    }
                    if let Some(fc) = p.get("functionCall") {
                        self.calls += 1;
                        let id = fc
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map_or_else(generated_call_id, str::to_owned);
                        if let Some(sig) = p.get("thoughtSignature").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                            self.signatures.push((id.clone(), sig.to_owned()));
                        }
                        out.push(ChatEvent::ToolCall {
                            id,
                            name: fc.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            arguments: fc.get("args").cloned().unwrap_or_else(|| json!({})),
                        });
                    }
                }
            }
            if let Some(reason) = cand.get("finishReason").and_then(Value::as_str) {
                self.stop = Some(match reason {
                    "STOP" if self.calls > 0 => StopReason::ToolUse,
                    "STOP" => StopReason::EndTurn,
                    "MAX_TOKENS" => StopReason::MaxTokens,
                    "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => StopReason::ContentFilter,
                    other => StopReason::Other(other.to_owned()),
                });
            }
        }
        if let Some(u) = v.get("usageMetadata") {
            let get = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let thoughts = get("thoughtsTokenCount");
            self.usage = Some(Usage {
                input_tokens: get("promptTokenCount"),
                output_tokens: get("candidatesTokenCount") + thoughts,
                cached_input_tokens: get("cachedContentTokenCount"),
                reasoning_tokens: thoughts,
                ..Usage::default()
            });
        }
        Ok(out)
    }

    /// Thought signatures seen so far as `(call id, signature)` (drained).
    pub fn take_signatures(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.signatures)
    }

    /// Final events.
    pub fn finish(&mut self) -> Result<Vec<ChatEvent>, ProviderError> {
        let Some(stop) = self.stop.clone() else {
            return Err(ProviderError::Server("stream ended without finishReason".into()));
        };
        let mut out = Vec::new();
        if let Some(u) = self.usage {
            out.push(ChatEvent::Usage(u));
        }
        out.push(ChatEvent::Done { stop_reason: stop });
        Ok(out)
    }
}

async fn run_stream(
    resp: reqwest::Response,
    em: Emitter<ChatEvent>,
    idle: std::time::Duration,
    signatures: Arc<Mutex<SignatureCache>>,
) -> Result<(), ProviderError> {
    let mut body = resp.bytes_stream();
    let mut sse = SseParser::default();
    let mut dec = GeminiDecoder::default();
    loop {
        let chunk = http::next_chunk(&mut body, idle).await?;
        let events = match &chunk {
            Some(b) => sse.push(b),
            None => sse.finish(),
        };
        for ev in events {
            let outs = dec.data(&ev.data)?;
            // Record before the call is emitted, so the next turn finds it.
            for (id, sig) in dec.take_signatures() {
                signatures.lock().insert(id, sig);
            }
            for out in outs {
                em.emit(out).await?;
            }
        }
        if chunk.is_none() {
            break;
        }
    }
    for out in dec.finish()? {
        em.emit(out).await?;
    }
    Ok(())
}

#[async_trait]
impl ProviderAdapter for GeminiAdapter {
    fn id(&self) -> ProviderId {
        self.cfg.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Gemini,
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
            max_context_tokens: 1_048_576,
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
        Ok(if m.is_empty() { default_models(ProviderKind::Gemini, self.cfg.id, false) } else { m })
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
        let model = request.model.trim_start_matches("models/").to_owned();
        if model.is_empty() || model.contains(['/', '?', '#']) {
            return Err(ProviderError::BadRequest(format!("invalid Gemini model id `{model}`")));
        }
        let body = {
            let cache = self.signatures.lock();
            build_body_with(&request, &|id| cache.get(id).map(str::to_owned))
        };
        let url = self.cfg.url(&format!("v1beta/models/{model}:streamGenerateContent?alt=sse"));
        let rb = self.authed(self.cfg.client.post(url).json(&body)).await?;
        let idle = self.cfg.idle_timeout;
        let signatures = Arc::clone(&self.signatures);
        Ok(drive(cancel, move |em| async move {
            let resp = http::send_with_header_timeout(rb, idle).await?;
            run_stream(resp, em, idle, signatures).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolCallPart, ToolSpec};

    #[test]
    fn body_maps_function_calls_and_responses() {
        let mut req = ChatRequest::new(
            "gemini-2.5-flash",
            vec![
                ChatMessage::system("sys"),
                ChatMessage::user("hi"),
                ChatMessage::assistant_tool_calls(
                    "",
                    vec![ToolCallPart { id: "c1".into(), name: "lookup".into(), arguments: json!({"q":"x"}) }],
                ),
                ChatMessage::tool_result("c1", "plain text", false),
            ],
        );
        req.tools = vec![ToolSpec {
            name: "lookup".into(),
            description: "d".into(),
            parameters: json!({"type":"object","additionalProperties":false,"properties":{"q":{"type":"string"}}}),
        }];
        req.response_format = ResponseFormat::JsonSchema { name: "x".into(), schema: json!({"type":"object"}) };
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(b["contents"][1]["role"], "model");
        assert_eq!(b["contents"][1]["parts"][0]["functionCall"]["name"], "lookup");
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["name"], "lookup");
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["response"]["result"], "plain text");
        assert!(b["tools"][0]["functionDeclarations"][0]["parameters"].get("additionalProperties").is_none());
        assert_eq!(b["generationConfig"]["responseMimeType"], "application/json");
        assert_eq!(b["generationConfig"]["thinkingConfig"]["includeThoughts"], false);
    }

    #[test]
    fn thought_signatures_round_trip_on_function_calls() {
        let mut d = GeminiDecoder::default();
        let out = d
            .data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"f","args":{}},"thoughtSignature":"SIG1"},{"functionCall":{"name":"g","args":{}}}]},"finishReason":"STOP"}]}"#)
            .unwrap();
        let ids: Vec<String> = out
            .iter()
            .filter_map(|e| match e {
                ChatEvent::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let mut cache = SignatureCache::default();
        for (id, sig) in d.take_signatures() {
            cache.insert(id, sig);
        }
        assert_eq!(cache.get(&ids[0]), Some("SIG1"));
        assert_eq!(cache.get(&ids[1]), None);

        let req = ChatRequest::new(
            "gemini-3-pro-preview",
            vec![
                ChatMessage::user("hi"),
                ChatMessage::assistant_tool_calls(
                    "",
                    vec![
                        ToolCallPart { id: ids[0].clone(), name: "f".into(), arguments: json!({}) },
                        ToolCallPart { id: ids[1].clone(), name: "g".into(), arguments: json!({}) },
                    ],
                ),
                ChatMessage::tool_result(&ids[0], "a", false),
                ChatMessage::tool_result(&ids[1], "b", false),
            ],
        );
        let b = build_body_with(&req, &|id| cache.get(id).map(str::to_owned));
        let parts = &b["contents"][1]["parts"];
        assert_eq!(parts[0]["thoughtSignature"], "SIG1");
        assert!(parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn unknown_signatures_use_placeholder_on_gemini_3_only() {
        let history = |model: &str| {
            ChatRequest::new(
                model,
                vec![
                    ChatMessage::user("hi"),
                    ChatMessage::assistant_tool_calls(
                        "",
                        vec![
                            ToolCallPart { id: "call_from_openai".into(), name: "f".into(), arguments: json!({}) },
                            ToolCallPart { id: "call_2".into(), name: "f".into(), arguments: json!({}) },
                        ],
                    ),
                    ChatMessage::tool_result("call_from_openai", "a", false),
                    ChatMessage::tool_result("call_2", "b", false),
                ],
            )
        };
        let b = build_body(&history("gemini-3-pro-preview"));
        assert_eq!(b["contents"][1]["parts"][0]["thoughtSignature"], DUMMY_THOUGHT_SIGNATURE);
        assert!(b["contents"][1]["parts"][1].get("thoughtSignature").is_none());
        let b = build_body(&history("models/gemini-3.1-flash"));
        assert_eq!(b["contents"][1]["parts"][0]["thoughtSignature"], DUMMY_THOUGHT_SIGNATURE);
        let b = build_body(&history("gemini-2.5-flash"));
        assert!(b["contents"][1]["parts"][0].get("thoughtSignature").is_none());
    }

    #[test]
    fn signature_cache_is_bounded() {
        let mut c = SignatureCache::default();
        for i in 0..(MAX_SIGNATURES + 10) {
            c.insert(format!("id{i}"), "s".into());
        }
        assert!(c.get("id0").is_none());
        assert_eq!(c.get(&format!("id{}", MAX_SIGNATURES + 9)), Some("s"));
        assert_eq!(c.map.len(), MAX_SIGNATURES);
    }

    #[test]
    fn decoder_drops_thoughts_and_maps_calls() {
        let mut d = GeminiDecoder::default();
        let mut out = d
            .data(r#"{"candidates":[{"content":{"parts":[{"text":"pondering","thought":true},{"text":"Hi"}]}}]}"#)
            .unwrap();
        out.extend(d.data(r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"f","args":{"a":1}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":3,"thoughtsTokenCount":2}}"#).unwrap());
        out.extend(d.finish().unwrap());
        assert!(!format!("{out:?}").contains("pondering"));
        assert_eq!(out[0], ChatEvent::TextDelta("Hi".into()));
        assert!(matches!(&out[1], ChatEvent::ToolCall { name, .. } if name == "f"));
        assert!(matches!(
            out[2],
            ChatEvent::Usage(Usage { input_tokens: 4, output_tokens: 5, reasoning_tokens: 2, .. })
        ));
        assert_eq!(out[3], ChatEvent::Done { stop_reason: StopReason::ToolUse });
    }
}
