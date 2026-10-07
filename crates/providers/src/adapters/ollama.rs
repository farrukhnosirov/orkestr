//! Ollama (`/api/chat` NDJSON streaming, `/api/tags` models, `/api/version` health).

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::http::{self, drive, Emitter, HttpConfig, LineBuffer, PROBE_TIMEOUT};
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality, ProviderDescriptor,
    ProviderError, ResponseFormat, Role, StopReason, Usage,
};

/// Context window assumed when `/api/show` does not report one.
pub const FALLBACK_CONTEXT_WINDOW: u64 = 32_768;
/// Smallest `num_ctx` sent (Ollama's own default is only 2K-4K tokens).
const MIN_NUM_CTX: u64 = 8_192;
/// Output reserve when the request sets no `max_output_tokens`.
const DEFAULT_OUTPUT_RESERVE: u64 = 4_096;

/// Ollama adapter.
#[derive(Debug)]
pub struct OllamaAdapter {
    cfg: HttpConfig,
    local: bool,
    /// Real context length per model (from `/api/show`).
    contexts: Mutex<HashMap<String, u64>>,
}

impl OllamaAdapter {
    /// Creates the adapter. `local` = endpoint on this machine.
    #[must_use]
    pub fn new(cfg: HttpConfig) -> Self {
        let local = is_loopback_url(&cfg.base_url);
        Self { cfg, local, contexts: Mutex::new(HashMap::new()) }
    }

    /// The model's trained context length (`/api/show`), cached; `None` when
    /// the server does not report it.
    async fn context_length(&self, model: &str) -> Option<u64> {
        if let Some(n) = self.contexts.lock().get(model) {
            return Some(*n);
        }
        let rb = self.rb(self.cfg.client.post(self.cfg.url("api/show")).json(&json!({"model": model})));
        let v: Value = http::send(rb.timeout(PROBE_TIMEOUT)).await.ok()?.json().await.ok()?;
        let n = context_length_from_show(&v)?;
        self.contexts.lock().insert(model.to_owned(), n);
        Some(n)
    }

    fn rb(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.cfg.with_headers(rb)
    }

    async fn version(&self) -> Result<String, ProviderError> {
        let resp = http::send(self.rb(self.cfg.client.get(self.cfg.url("api/version")).timeout(PROBE_TIMEOUT))).await?;
        let v: Value = resp.json().await.map_err(http::map_reqwest_error)?;
        Ok(v.get("version").and_then(Value::as_str).unwrap_or("unknown").to_owned())
    }
}

/// Whether a URL points at this machine (privacy routing: `LocalModelsOnly`).
///
/// The URL is parsed exactly as the HTTP client will parse it, so the host
/// checked is the host connected to. URLs with userinfo are never local
/// (`http://127.0.0.1:x@remote/` connects to `remote`), and only `localhost`
/// or a literal loopback / unspecified IP counts: a DNS name such as
/// `127.example.com` can resolve anywhere.
#[must_use]
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else { return false };
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let Some(host) = u.host_str() else { return false };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<IpAddr>() {
        Ok(ip) => is_local_ip(ip),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// Loopback, or the unspecified address (connecting to `0.0.0.0` / `::`
/// reaches this machine).
fn is_local_ip(ip: IpAddr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
}

/// Reads `model_info.<arch>.context_length` from an `/api/show` response.
#[must_use]
pub fn context_length_from_show(v: &Value) -> Option<u64> {
    v.get("model_info")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, n)| n.as_u64())
        .filter(|n| *n > 0)
}

/// `num_ctx` for a request: enough for the estimated prompt plus the output
/// reserve, rounded up to a power of two (so the model is not reloaded for
/// every small size change), at least [`MIN_NUM_CTX`] and capped at the
/// model's context window. Without it Ollama silently truncates the prompt
/// to its 2K-4K default.
#[must_use]
pub fn num_ctx_for(req: &ChatRequest, model_context: Option<u64>) -> u64 {
    // chars/3 over-estimates tokens for code-heavy prompts on purpose.
    let prompt = (req.prompt_chars() as u64).div_ceil(3) + 256;
    let output = req.max_output_tokens.map_or(DEFAULT_OUTPUT_RESERVE, u64::from);
    let needed = (prompt + output).max(MIN_NUM_CTX).next_power_of_two();
    needed.min(model_context.unwrap_or(FALLBACK_CONTEXT_WINDOW))
}

/// Builds the `/api/chat` body (pure; unit-tested). `model_context` is the
/// model's real context length when known (sizes `options.num_ctx`).
#[must_use]
pub fn build_body(req: &ChatRequest, model_context: Option<u64>) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = req.system_prompt() {
        messages.push(json!({"role": "system", "content": sys}));
    }
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in req.conversation() {
        match m.role {
            Role::User => {
                let mut msg = json!({"role": "user", "content": m.content});
                if !m.images.is_empty() {
                    msg["images"] = m.images.iter().map(|i| json!(i.data_base64)).collect();
                }
                messages.push(msg);
            }
            Role::Assistant => {
                let mut msg = json!({"role": "assistant", "content": m.content});
                if !m.tool_calls.is_empty() {
                    for c in &m.tool_calls {
                        names.insert(c.id.as_str(), c.name.as_str());
                    }
                    msg["tool_calls"] = m
                        .tool_calls
                        .iter()
                        .map(|c| json!({"function": {"name": c.name, "arguments": c.arguments}}))
                        .collect();
                }
                messages.push(msg);
            }
            Role::Tool => {
                let mut msg = json!({"role": "tool", "content": m.content});
                if let Some(name) = m.tool_call_id.as_deref().and_then(|id| names.get(id)) {
                    msg["tool_name"] = json!(name);
                }
                messages.push(msg);
            }
            Role::System => {}
        }
    }
    let mut body = json!({"model": req.model, "messages": messages, "stream": true, "think": false});
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
    }
    match &req.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject => body["format"] = json!("json"),
        ResponseFormat::JsonSchema { schema, .. } => body["format"] = schema.clone(),
    }
    let mut options = serde_json::Map::new();
    options.insert("num_ctx".into(), json!(num_ctx_for(req, model_context)));
    if let Some(t) = req.temperature {
        options.insert("temperature".into(), json!(t));
    }
    if let Some(n) = req.max_output_tokens {
        options.insert("num_predict".into(), json!(n));
    }
    if !req.stop.is_empty() {
        options.insert("stop".into(), json!(req.stop));
    }
    body["options"] = Value::Object(options);
    body
}

/// NDJSON decoder for `/api/chat`.
#[derive(Default)]
pub struct OllamaDecoder {
    calls: u32,
    done: bool,
    usage: Option<Usage>,
    stop: Option<StopReason>,
}

impl OllamaDecoder {
    /// Decodes one JSON line.
    pub fn line(&mut self, line: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        if line.trim().is_empty() {
            return Ok(vec![]);
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| ProviderError::Server(format!("malformed stream line: {e}")))?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(ProviderError::Server(err.to_owned()));
        }
        let mut out = Vec::new();
        // `message.thinking` is dropped by design.
        if let Some(t) = v.pointer("/message/content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            out.push(ChatEvent::TextDelta(t.to_owned()));
        }
        if let Some(calls) = v.pointer("/message/tool_calls").and_then(Value::as_array) {
            for c in calls {
                self.calls += 1;
                let args = c.pointer("/function/arguments").cloned().unwrap_or_else(|| json!({}));
                let args = match args {
                    Value::String(s) => crate::types::parse_tool_arguments(&s),
                    other => other,
                };
                out.push(ChatEvent::ToolCall {
                    id: c
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map_or_else(|| crate::types::unique_call_id("ollama_call"), str::to_owned),
                    name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    arguments: args,
                });
            }
        }
        if v.get("done").and_then(Value::as_bool) == Some(true) {
            self.done = true;
            let get = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
            self.usage = Some(Usage {
                input_tokens: get("prompt_eval_count"),
                output_tokens: get("eval_count"),
                ..Usage::default()
            });
            self.stop = Some(match v.get("done_reason").and_then(Value::as_str) {
                Some("length") => StopReason::MaxTokens,
                _ if self.calls > 0 => StopReason::ToolUse,
                _ => StopReason::EndTurn,
            });
        }
        Ok(out)
    }

    /// Whether `done: true` arrived.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Final events.
    pub fn finish(&mut self) -> Result<Vec<ChatEvent>, ProviderError> {
        if !self.done {
            return Err(ProviderError::Server("stream ended before done".into()));
        }
        let mut out = Vec::new();
        if let Some(u) = self.usage {
            out.push(ChatEvent::Usage(u));
        }
        out.push(ChatEvent::Done { stop_reason: self.stop.clone().unwrap_or(StopReason::EndTurn) });
        Ok(out)
    }
}

async fn run_stream(
    resp: reqwest::Response,
    em: Emitter<ChatEvent>,
    idle: std::time::Duration,
) -> Result<(), ProviderError> {
    let mut body = resp.bytes_stream();
    let mut lines = LineBuffer::default();
    let mut dec = OllamaDecoder::default();
    loop {
        let chunk = http::next_chunk(&mut body, idle).await?;
        let batch = match &chunk {
            Some(b) => lines.push(b),
            None => lines.finish().into_iter().collect(),
        };
        for l in batch {
            for out in dec.line(&l)? {
                em.emit(out).await?;
            }
        }
        if chunk.is_none() || dec.is_done() {
            break;
        }
    }
    for out in dec.finish()? {
        em.emit(out).await?;
    }
    Ok(())
}

fn map_unreachable(e: ProviderError) -> ProviderError {
    match e {
        ProviderError::Network(m) => {
            ProviderError::Network(format!("Ollama is not reachable ({m}) — is `ollama serve` running?"))
        }
        ProviderError::BadRequest(m) if m.contains("404") => {
            ProviderError::BadRequest(format!("{m} (pull the model with `ollama pull <model>`)"))
        }
        other => other,
    }
}

#[async_trait]
impl ProviderAdapter for OllamaAdapter {
    fn id(&self) -> ProviderId {
        self.cfg.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Ollama,
            display_name: self.cfg.name.clone(),
            is_local: self.local,
            locality: if self.local { Locality::Local } else { Locality::SelfHosted },
            capabilities: ModelCapabilities {
                tools: true,
                vision: true,
                structured_output: true,
                streaming: true,
                reasoning: false,
                coding_agent: false,
            },
            max_context_tokens: 131_072,
            coding_agent: false,
            cost_model: CostModelKind::Local,
            max_concurrency: 1,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        match self.version().await {
            Ok(v) => AuthStatus::Ready { label: format!("local endpoint (Ollama {v})") },
            Err(e) => AuthStatus::Unreachable { reason: map_unreachable(e).to_string() },
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        let resp = http::send(self.rb(self.cfg.client.get(self.cfg.url("api/tags")).timeout(PROBE_TIMEOUT)))
            .await
            .map_err(map_unreachable)?;
        let v: Value = resp.json().await.map_err(http::map_reqwest_error)?;
        let mut out: Vec<Model> = v
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("name").or_else(|| m.get("model"))?.as_str()?.to_owned();
                        let size = m.pointer("/details/parameter_size").and_then(Value::as_str);
                        let family = m.pointer("/details/family").and_then(Value::as_str).unwrap_or_default();
                        Some(Model {
                            provider_id: self.cfg.id,
                            display_name: match size {
                                Some(s) => format!("{id} ({s})"),
                                None => id.clone(),
                            },
                            context_window: 32_768,
                            max_output_tokens: None,
                            capabilities: ModelCapabilities {
                                tools: true,
                                vision: family.contains("llava") || id.contains("vision") || id.contains("llava"),
                                structured_output: true,
                                streaming: true,
                                reasoning: id.contains("r1") || id.contains("qwq"),
                                coding_agent: false,
                            },
                            pricing: None,
                            is_local: self.local,
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Advertise the real context window (the router's min-context filter
        // and the context optimizer size prompts from it).
        let windows = futures::future::join_all(out.iter().map(|m| self.context_length(&m.id))).await;
        for (m, w) in out.iter_mut().zip(windows) {
            if let Some(w) = w {
                m.context_window = u32::try_from(w).unwrap_or(u32::MAX);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn health(&self) -> HealthReport {
        let start = Instant::now();
        match self.version().await {
            Ok(v) => HealthReport {
                health: ProviderHealth::Healthy,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                detail: Some(format!("Ollama {v}")),
                version: Some(v),
            },
            Err(e) => HealthReport::from_error(&map_unreachable(e), None),
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        let context = self.context_length(&request.model).await;
        let body = build_body(&request, context);
        let rb = self.rb(self.cfg.client.post(self.cfg.url("api/chat")).json(&body));
        let idle = self.cfg.idle_timeout;
        Ok(drive(cancel, move |em| async move {
            let wait = idle.max(http::LOCAL_LOAD_TIMEOUT);
            let resp = http::send_with_header_timeout(rb, wait).await.map_err(map_unreachable)?;
            run_stream(resp, em, idle).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolCallPart};

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://localhost:11434"));
        assert!(is_loopback_url("http://LOCALHOST:11434/v1"));
        assert!(is_loopback_url("http://127.0.0.1:11434/"));
        assert!(is_loopback_url("http://127.0.0.2:8000"));
        assert!(is_loopback_url("http://[::1]:11434"));
        assert!(is_loopback_url("http://0.0.0.0:11434"));
        assert!(!is_loopback_url("http://gpu-box.lan:11434"));
    }

    #[test]
    fn loopback_detection_rejects_userinfo_and_lookalike_names() {
        // reqwest connects to the host after '@', not the userinfo.
        assert!(!is_loopback_url("http://127.0.0.1:x@gpu.example.com:11434"));
        assert!(!is_loopback_url("http://localhost:x@llm.example.com/v1"));
        assert!(!is_loopback_url("http://localhost:pw@remote-host:11434"));
        assert!(!is_loopback_url("http://user@localhost:11434"));
        // DNS names that merely start with "127." can resolve anywhere.
        assert!(!is_loopback_url("http://127.evil.example:11434"));
        assert!(!is_loopback_url("http://127.example.com:8000/v1"));
        assert!(!is_loopback_url("http://localhost.example.com:11434"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn num_ctx_covers_prompt_and_is_capped_by_model() {
        let mut req = ChatRequest::new("llama3.1:8b", vec![ChatMessage::user("x".repeat(45_000))]);
        req.max_output_tokens = Some(4_096);
        // ~15K prompt tokens + 4K output -> 32K bucket, not Ollama's 2-4K default.
        let b = build_body(&req, Some(131_072));
        assert_eq!(b["options"]["num_ctx"], 32_768);
        // Never above the model's real window.
        assert_eq!(num_ctx_for(&req, Some(16_384)), 16_384);
        // Short prompts still get a sane minimum.
        let small = ChatRequest::new("llama3.1:8b", vec![ChatMessage::user("hi")]);
        assert_eq!(num_ctx_for(&small, Some(131_072)), 8_192);
        assert_eq!(num_ctx_for(&small, Some(2_048)), 2_048);
        // Unknown window: the advertised fallback.
        req.messages = vec![ChatMessage::user("x".repeat(400_000))];
        assert_eq!(num_ctx_for(&req, None), FALLBACK_CONTEXT_WINDOW);
    }

    #[test]
    fn reads_context_length_from_show() {
        let v = json!({"model_info": {"general.architecture": "llama", "llama.context_length": 131_072}});
        assert_eq!(context_length_from_show(&v), Some(131_072));
        assert_eq!(context_length_from_show(&json!({"model_info": {}})), None);
        assert_eq!(context_length_from_show(&json!({})), None);
    }

    #[test]
    fn body_and_decoder() {
        let mut req = ChatRequest::new(
            "llama3.2",
            vec![
                ChatMessage::user("hi"),
                ChatMessage::assistant_tool_calls(
                    "",
                    vec![ToolCallPart { id: "x".into(), name: "f".into(), arguments: json!({}) }],
                ),
                ChatMessage::tool_result("x", "r", false),
            ],
        );
        req.response_format = ResponseFormat::JsonObject;
        req.max_output_tokens = Some(5);
        let b = build_body(&req, None);
        assert_eq!(b["format"], "json");
        assert_eq!(b["options"]["num_predict"], 5);
        assert_eq!(b["messages"][2]["tool_name"], "f");

        let mut d = OllamaDecoder::default();
        let mut out =
            d.line(r#"{"message":{"role":"assistant","content":"","thinking":"secret"},"done":false}"#).unwrap();
        out.extend(d.line(r#"{"message":{"role":"assistant","content":"Hi"},"done":false}"#).unwrap());
        out.extend(d.line(r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"f","arguments":{"a":1}}}]},"done":false}"#).unwrap());
        out.extend(d.line(r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":4}"#).unwrap());
        out.extend(d.finish().unwrap());
        assert!(!format!("{out:?}").contains("secret"));
        assert_eq!(out[0], ChatEvent::TextDelta("Hi".into()));
        assert!(matches!(&out[1], ChatEvent::ToolCall { arguments, .. } if arguments["a"] == 1));
        assert!(matches!(out[2], ChatEvent::Usage(Usage { input_tokens: 9, output_tokens: 4, .. })));
        assert_eq!(out[3], ChatEvent::Done { stop_reason: StopReason::ToolUse });
    }

    /// Id-less calls must get ids that never repeat across responses, or a
    /// fallback provider (Anthropic) rejects the duplicated `tool_use` ids.
    #[test]
    fn id_less_calls_get_unique_ids_across_responses() {
        let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"f","arguments":{}}}]},"done":false}"#;
        let ids: Vec<String> = (0..3)
            .map(|_| {
                let mut d = OllamaDecoder::default();
                match d.line(line).unwrap().remove(0) {
                    ChatEvent::ToolCall { id, .. } => id,
                    other => panic!("unexpected {other:?}"),
                }
            })
            .collect();
        assert!(ids.iter().all(|id| id.starts_with("ollama_call_")));
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
        assert_ne!(ids[0], ids[2]);
    }
}
