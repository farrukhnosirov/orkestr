//! Every HTTP adapter against a local axum mock server: request shape and
//! auth headers, SSE/NDJSON parsing, tool-call assembly, usage extraction and
//! the error classification table (spec §4).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::StreamExt;
use orkestr_domain::{AuthMethod, Provider, ProviderId, ProviderKind, SecretRefId};
use orkestr_providers::factory::build_adapter;
use orkestr_providers::{
    AdapterDeps, ChatEvent, ChatMessage, ChatRequest, CredentialResolver, MemoryCredentials, ProviderAdapter,
    ProviderError, StopReason, ToolSpec, Usage,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
struct Captured {
    method: Method,
    path: String,
    query: String,
    headers: HeaderMap,
    body: Value,
}

/// `(status, headers, body)` served for a path.
type Canned = (u16, Vec<(String, String)>, String);

#[derive(Clone, Default)]
struct Mock {
    responses: Arc<Mutex<HashMap<String, Canned>>>,
    captured: Arc<Mutex<Vec<Captured>>>,
}

async fn handler(State(m): State<Mock>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path().to_owned();
    m.captured.lock().push(Captured {
        method,
        path: path.clone(),
        query: uri.query().unwrap_or_default().to_owned(),
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    let Some((status, hdrs, body)) = m.responses.lock().get(&path).cloned() else {
        return (StatusCode::NOT_FOUND, format!("no mock for {path}")).into_response();
    };
    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap();
    for (k, v) in hdrs {
        resp.headers_mut().insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
    }
    resp
}

impl Mock {
    async fn start() -> (Self, String) {
        let m = Mock::default();
        let app = Router::new().fallback(handler).with_state(m.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (m, format!("http://{addr}"))
    }

    fn on(&self, path: &str, status: u16, headers: &[(&str, &str)], body: impl Into<String>) {
        self.responses.lock().insert(
            path.to_owned(),
            (status, headers.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect(), body.into()),
        );
    }

    fn last(&self) -> Captured {
        self.captured.lock().last().cloned().expect("no request captured")
    }
}

async fn adapter(kind: ProviderKind, base_url: &str, key: Option<&str>, settings: Value) -> Arc<dyn ProviderAdapter> {
    let creds = Arc::new(MemoryCredentials::new());
    let id = ProviderId::new();
    let secret: Option<SecretRefId> = match key {
        Some(k) => Some(creds.store(id, "test", k).await.unwrap()),
        None => None,
    };
    let now = chrono::Utc::now();
    let p = Provider {
        id,
        kind,
        name: format!("{kind:?} test"),
        enabled: true,
        base_url: Some(base_url.to_owned()),
        auth_method: AuthMethod::ApiKey,
        credential_ref_id: None,
        secret_ref_id: secret,
        is_local: false,
        default_model: None,
        settings,
        created_at: now,
        updated_at: now,
    };
    let mut deps = AdapterDeps::new(creds);
    deps.idle_timeout = Duration::from_secs(5);
    build_adapter(&p, &deps).unwrap()
}

fn tool_request(model: &str) -> ChatRequest {
    let mut r = ChatRequest::new(model, vec![ChatMessage::system("be terse"), ChatMessage::user("write hello.txt")]);
    r.tools = vec![ToolSpec {
        name: "write_file".into(),
        description: "Write a file".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
    }];
    r
}

async fn collect(a: &Arc<dyn ProviderAdapter>, r: ChatRequest) -> Vec<ChatEvent> {
    match a.chat(r, CancellationToken::new()).await {
        Ok(s) => tokio::time::timeout(Duration::from_secs(10), s.collect::<Vec<_>>()).await.expect("stream timed out"),
        Err(e) => vec![ChatEvent::Error(e)],
    }
}

fn text(ev: &[ChatEvent]) -> String {
    ev.iter().filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect()
}

fn terminal_error(ev: &[ChatEvent]) -> ProviderError {
    match ev.last() {
        Some(ChatEvent::Error(e)) => e.clone(),
        other => panic!("expected terminal error, got {other:?}"),
    }
}

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(
            |(ev, data)| {
                if ev.is_empty() {
                    format!("data: {data}\n\n")
                } else {
                    format!("event: {ev}\ndata: {data}\n\n")
                }
            },
        )
        .collect()
}

/// Runs the classification table (§4) against one adapter's chat endpoint.
async fn check_error_table(mock: &Mock, path: &str, a: &Arc<dyn ProviderAdapter>, model: &str, quota: (u16, &str)) {
    let req = || ChatRequest::new(model, vec![ChatMessage::user("hi")]);
    mock.on(
        path,
        429,
        &[("retry-after", "7")],
        r#"{"error":{"message":"Rate limit reached","type":"rate_limit_error"}}"#,
    );
    assert_eq!(
        terminal_error(&collect(a, req()).await),
        ProviderError::RateLimited { retry_after_secs: Some(7) },
        "{path} 429"
    );
    mock.on(path, 401, &[], r#"{"error":{"message":"invalid key"}}"#);
    assert!(matches!(terminal_error(&collect(a, req()).await), ProviderError::Auth(_)), "{path} 401");
    mock.on(path, 403, &[], r#"{"error":{"message":"permission denied"}}"#);
    assert!(matches!(terminal_error(&collect(a, req()).await), ProviderError::Auth(_)), "{path} 403");
    mock.on(path, quota.0, &[], quota.1);
    assert!(matches!(terminal_error(&collect(a, req()).await), ProviderError::QuotaExceeded(_)), "{path} quota");
    mock.on(path, 503, &[], r#"{"error":{"message":"overloaded"}}"#);
    assert!(matches!(terminal_error(&collect(a, req()).await), ProviderError::Server(_)), "{path} 5xx");
    mock.on(path, 400, &[], r#"{"error":{"message":"bad schema"}}"#);
    assert!(matches!(terminal_error(&collect(a, req()).await), ProviderError::BadRequest(_)), "{path} 400");
}

fn openai_tool_stream() -> String {
    let mut s = sse(&[
        ("", json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"Sure"}}]})),
        ("", json!({"choices":[{"index":0,"delta":{"content":", writing."}}]})),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"write_file","arguments":""}}]}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":\"hello.txt\","}}]}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"content\":\"hi\"}"}}]}}]}),
        ),
        ("", json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),
        (
            "",
            json!({"choices":[],"usage":{"prompt_tokens":31,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":8},"completion_tokens_details":{"reasoning_tokens":3}}}),
        ),
    ]);
    s.push_str("data: [DONE]\n\n");
    s
}

fn assert_tool_stream(ev: &[ChatEvent]) {
    assert_eq!(text(ev), "Sure, writing.");
    let call = ev.iter().find(|e| matches!(e, ChatEvent::ToolCall { .. })).expect("tool call");
    assert_eq!(
        call,
        &ChatEvent::ToolCall {
            id: "call_abc".into(),
            name: "write_file".into(),
            arguments: json!({"path":"hello.txt","content":"hi"})
        }
    );
    assert_eq!(ev.last(), Some(&ChatEvent::Done { stop_reason: StopReason::ToolUse }));
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1, "exactly one terminal event");
}

#[tokio::test]
async fn openai_streaming_tools_usage_and_errors() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::OpenAi, &format!("{url}/v1"), Some("sk-test"), json!({})).await;
    mock.on("/v1/chat/completions", 200, &[("content-type", "text/event-stream")], openai_tool_stream());
    let ev = collect(&a, tool_request("gpt-4.1-mini")).await;
    assert_tool_stream(&ev);
    assert!(ev.iter().any(|e| matches!(
        e,
        ChatEvent::Usage(Usage {
            input_tokens: 31,
            output_tokens: 12,
            cached_input_tokens: 8,
            reasoning_tokens: 3,
            estimated: false,
            ..
        })
    )));
    let req = mock.last();
    assert_eq!(req.method, Method::POST);
    assert_eq!(req.path, "/v1/chat/completions");
    assert_eq!(req.headers["authorization"], "Bearer sk-test");
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.body["stream_options"]["include_usage"], true);
    assert_eq!(req.body["tools"][0]["function"]["name"], "write_file");
    assert_eq!(req.body["messages"][0]["role"], "system");

    mock.on(
        "/v1/models",
        200,
        &[],
        json!({"data":[{"id":"gpt-4.1-mini"},{"id":"text-embedding-3-small"},{"id":"gpt-4o"}]}).to_string(),
    );
    let models = a.list_models().await.unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["gpt-4.1-mini", "gpt-4o"]);
    assert!(models[0].pricing.is_some());
    let h = a.health().await;
    assert_eq!(h.health, orkestr_domain::ProviderHealth::Healthy);

    check_error_table(&mock, "/v1/chat/completions", &a, "gpt-4o", (429, r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#)).await;

    // Missing key is a pre-flight Auth error.
    let nokey = adapter(ProviderKind::OpenAi, &format!("{url}/v1"), None, json!({})).await;
    assert!(matches!(
        nokey.chat(ChatRequest::new("gpt-4o", vec![]), CancellationToken::new()).await,
        Err(ProviderError::Auth(_))
    ));
}

#[tokio::test]
async fn deepseek_preset_drops_reasoning_and_maps_402() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::DeepSeek, &url, Some("ds-key"), json!({})).await;
    let mut body = sse(&[
        ("", json!({"choices":[{"delta":{"reasoning_content":"internal thoughts"}}]})),
        (
            "",
            json!({"choices":[{"delta":{"content":"Answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"prompt_cache_hit_tokens":4}}),
        ),
    ]);
    body.push_str("data: [DONE]\n\n");
    mock.on("/chat/completions", 200, &[], body);
    let ev = collect(&a, ChatRequest::new("deepseek-reasoner", vec![ChatMessage::user("q")])).await;
    assert_eq!(text(&ev), "Answer");
    assert!(!format!("{ev:?}").contains("internal thoughts"));
    assert!(ev.iter().any(|e| matches!(e, ChatEvent::Usage(Usage { cached_input_tokens: 4, .. }))));
    assert_eq!(mock.last().headers["authorization"], "Bearer ds-key");
    check_error_table(
        &mock,
        "/chat/completions",
        &a,
        "deepseek-chat",
        (402, r#"{"error":{"message":"Insufficient Balance","type":"unknown_error"}}"#),
    )
    .await;
}

#[tokio::test]
async fn openai_compatible_custom_headers_optional_key_and_missing_models() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::OpenAiCompatible, &format!("{url}/v1"), None, json!({"headers": {"X-Team": "core"}}))
        .await;
    // Server reports no usage → estimated usage is emitted.
    let mut body = sse(&[("", json!({"choices":[{"delta":{"content":"hi there"},"finish_reason":"stop"}]}))]);
    body.push_str("data: [DONE]\n\n");
    mock.on("/v1/chat/completions", 200, &[], body);
    let ev = collect(&a, ChatRequest::new("local-model", vec![ChatMessage::user("hello")])).await;
    assert_eq!(text(&ev), "hi there");
    assert!(ev.iter().any(|e| matches!(e, ChatEvent::Usage(Usage { estimated: true, .. }))));
    let req = mock.last();
    assert_eq!(req.headers["x-team"], "core");
    assert!(req.headers.get("authorization").is_none(), "no key → no auth header");
    assert_eq!(req.body["max_tokens"], Value::Null);
    // No /models endpoint: tolerated.
    assert!(a.list_models().await.unwrap().is_empty());
    assert_eq!(a.health().await.health, orkestr_domain::ProviderHealth::Healthy);

    let keyed = adapter(ProviderKind::OpenAiCompatible, &format!("{url}/v1"), Some("gw-key"), json!({})).await;
    mock.on("/v1/chat/completions", 200, &[], openai_tool_stream());
    assert_tool_stream(&collect(&keyed, tool_request("m")).await);
    assert_eq!(mock.last().headers["authorization"], "Bearer gw-key");
    check_error_table(
        &mock,
        "/v1/chat/completions",
        &keyed,
        "m",
        (429, r#"{"error":{"code":"insufficient_quota","message":"quota"}}"#),
    )
    .await;
}

#[tokio::test]
async fn anthropic_messages_stream_tool_use_and_errors() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::Anthropic, &url, Some("sk-ant-test"), json!({})).await;
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":20,"cache_read_input_tokens":5,"output_tokens":1}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"secret plan"}}),
        ),
        ("content_block_stop", json!({"type":"content_block_stop","index":0})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Sure, "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"writing."}}),
        ),
        ("content_block_stop", json!({"type":"content_block_stop","index":1})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"write_file","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"hello.txt\", "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"content\": \"hi\"}"}}),
        ),
        ("content_block_stop", json!({"type":"content_block_stop","index":2})),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":33}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    mock.on("/v1/messages", 200, &[("content-type", "text/event-stream")], body);
    let ev = collect(&a, tool_request("claude-sonnet-4-5")).await;
    assert_eq!(text(&ev), "Sure, writing.");
    assert!(!format!("{ev:?}").contains("secret plan"));
    assert!(ev.contains(&ChatEvent::ToolCall {
        id: "toolu_1".into(),
        name: "write_file".into(),
        arguments: json!({"path":"hello.txt","content":"hi"})
    }));
    assert!(ev.iter().any(|e| matches!(
        e,
        ChatEvent::Usage(Usage { input_tokens: 25, cached_input_tokens: 5, output_tokens: 33, .. })
    )));
    assert_eq!(ev.last(), Some(&ChatEvent::Done { stop_reason: StopReason::ToolUse }));
    let req = mock.last();
    assert_eq!(req.headers["x-api-key"], "sk-ant-test");
    assert_eq!(req.headers["anthropic-version"], "2023-06-01");
    assert!(req.headers.get("authorization").is_none());
    assert_eq!(req.body["system"], "be terse");
    assert_eq!(req.body["tools"][0]["input_schema"]["type"], "object");

    // Mid-stream overload error event → Server.
    mock.on(
        "/v1/messages",
        200,
        &[],
        sse(&[("error", json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}))]),
    );
    assert!(matches!(
        terminal_error(&collect(&a, ChatRequest::new("m", vec![ChatMessage::user("x")])).await),
        ProviderError::Server(_)
    ));

    mock.on("/v1/models", 200, &[], json!({"data":[{"id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5"},{"id":"claude-new-x","display_name":"New"}]}).to_string());
    let models = a.list_models().await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[1].display_name, "New");

    check_error_table(&mock, "/v1/messages", &a, "claude-sonnet-4-5", (400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#)).await;
}

#[tokio::test]
async fn gemini_stream_function_calls_and_errors() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::Gemini, &url, Some("g-key"), json!({})).await;
    let path = "/v1beta/models/gemini-2.5-flash:streamGenerateContent";
    let body = sse(&[
        ("", json!({"candidates":[{"content":{"role":"model","parts":[{"text":"thinking hard","thought":true}]}}]})),
        ("", json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Sure, "}]}}]})),
        (
            "",
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"writing."},{"functionCall":{"name":"write_file","args":{"path":"hello.txt","content":"hi"}}}]},"finishReason":"STOP"}],
                   "usageMetadata":{"promptTokenCount":40,"candidatesTokenCount":9,"thoughtsTokenCount":4,"cachedContentTokenCount":10}}),
        ),
    ]);
    mock.on(path, 200, &[], body);
    let ev = collect(&a, tool_request("gemini-2.5-flash")).await;
    assert_eq!(text(&ev), "Sure, writing.");
    assert!(!format!("{ev:?}").contains("thinking hard"));
    assert!(ev.iter().any(|e| matches!(e, ChatEvent::ToolCall { name, arguments, .. } if name == "write_file" && arguments["path"] == "hello.txt")));
    assert!(ev.iter().any(|e| matches!(
        e,
        ChatEvent::Usage(Usage {
            input_tokens: 40,
            output_tokens: 13,
            reasoning_tokens: 4,
            cached_input_tokens: 10,
            ..
        })
    )));
    assert_eq!(ev.last(), Some(&ChatEvent::Done { stop_reason: StopReason::ToolUse }));
    let req = mock.last();
    assert_eq!(req.headers["x-goog-api-key"], "g-key");
    assert_eq!(req.query, "alt=sse", "the key never travels in the URL");
    assert_eq!(req.body["systemInstruction"]["parts"][0]["text"], "be terse");
    assert_eq!(req.body["tools"][0]["functionDeclarations"][0]["name"], "write_file");

    // RESOURCE_EXHAUSTED with RetryInfo → RateLimited with the delay.
    let rl = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"Resource exhausted","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"12s"}]}}"#;
    mock.on(path, 429, &[], rl);
    assert_eq!(
        terminal_error(&collect(&a, ChatRequest::new("gemini-2.5-flash", vec![ChatMessage::user("x")])).await),
        ProviderError::RateLimited { retry_after_secs: Some(12) }
    );

    mock.on("/v1beta/models", 200, &[], json!({"models":[
        {"name":"models/gemini-2.5-flash","displayName":"Gemini 2.5 Flash","inputTokenLimit":1048576,"outputTokenLimit":65536,"supportedGenerationMethods":["generateContent","countTokens"]},
        {"name":"models/text-embedding-004","displayName":"Embedding","supportedGenerationMethods":["embedContent"]}]}).to_string());
    let models = a.list_models().await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gemini-2.5-flash");
    assert_eq!(models[0].max_output_tokens, Some(65536));

    check_error_table(&mock, path, &a, "gemini-2.5-flash", (429, r#"{"error":{"code":429,"message":"You exceeded your current quota, please check your plan and billing details.","status":"RESOURCE_EXHAUSTED"}}"#)).await;
}

#[tokio::test]
async fn ollama_ndjson_tags_version_and_errors() {
    let (mock, url) = Mock::start().await;
    let a = adapter(ProviderKind::Ollama, &url, None, json!({})).await;
    let lines = [
        json!({"model":"llama3.2","message":{"role":"assistant","content":"","thinking":"hmm secret"},"done":false}),
        json!({"model":"llama3.2","message":{"role":"assistant","content":"Sure, "},"done":false}),
        json!({"model":"llama3.2","message":{"role":"assistant","content":"writing."},"done":false}),
        json!({"model":"llama3.2","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"write_file","arguments":{"path":"hello.txt","content":"hi"}}}]},"done":false}),
        json!({"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":26,"eval_count":11}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    mock.on("/api/chat", 200, &[("content-type", "application/x-ndjson")], body);
    let ev = collect(&a, tool_request("llama3.2")).await;
    assert_eq!(text(&ev), "Sure, writing.");
    assert!(!format!("{ev:?}").contains("hmm secret"));
    assert!(ev.iter().any(|e| matches!(e, ChatEvent::ToolCall { name, arguments, .. } if name == "write_file" && arguments["content"] == "hi")));
    assert!(ev.iter().any(|e| matches!(e, ChatEvent::Usage(Usage { input_tokens: 26, output_tokens: 11, .. }))));
    assert_eq!(ev.last(), Some(&ChatEvent::Done { stop_reason: StopReason::ToolUse }));
    let req = mock.last();
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.body["think"], false);
    assert!(req.headers.get("authorization").is_none());

    mock.on(
        "/api/tags",
        200,
        &[],
        json!({"models":[{"name":"llama3.2:3b","details":{"parameter_size":"3.2B","family":"llama"}}]}).to_string(),
    );
    let models = a.list_models().await.unwrap();
    assert_eq!(models[0].id, "llama3.2:3b");
    assert!(models[0].pricing.is_none());
    mock.on("/api/version", 200, &[], r#"{"version":"0.12.3"}"#);
    let h = a.health().await;
    assert_eq!(h.version.as_deref(), Some("0.12.3"));
    assert!(matches!(a.auth_state().await, orkestr_providers::AuthStatus::Ready { .. }));

    // Model not pulled.
    mock.on("/api/chat", 404, &[], r#"{"error":"model \"nope\" not found, try pulling it first"}"#);
    assert!(
        matches!(terminal_error(&collect(&a, ChatRequest::new("nope", vec![ChatMessage::user("x")])).await), ProviderError::BadRequest(m) if m.contains("ollama pull"))
    );
    check_error_table(&mock, "/api/chat", &a, "llama3.2", (429, r#"{"error":"insufficient_quota"}"#)).await;
}

#[tokio::test]
async fn unreachable_endpoint_is_network_and_ollama_unreachable() {
    // Bind and drop to get a closed port.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("http://127.0.0.1:{port}");
    let a = adapter(ProviderKind::Ollama, &url, None, json!({})).await;
    assert!(matches!(
        terminal_error(&collect(&a, ChatRequest::new("m", vec![ChatMessage::user("x")])).await),
        ProviderError::Network(_)
    ));
    assert!(matches!(a.auth_state().await, orkestr_providers::AuthStatus::Unreachable { .. }));
    assert_eq!(a.health().await.health, orkestr_domain::ProviderHealth::Down);
}

#[tokio::test]
async fn idle_stream_times_out_and_cancel_is_terminal() {
    // A server that sends headers then stalls.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                    )
                    .await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });
    let creds = Arc::new(MemoryCredentials::new());
    let id = ProviderId::new();
    let secret = creds.store(id, "t", "k").await.unwrap();
    let now = chrono::Utc::now();
    let p = Provider {
        id,
        kind: ProviderKind::OpenAi,
        name: "stall".into(),
        enabled: true,
        base_url: Some(format!("http://{addr}/v1")),
        auth_method: AuthMethod::ApiKey,
        credential_ref_id: None,
        secret_ref_id: Some(secret),
        is_local: false,
        default_model: None,
        settings: json!({}),
        created_at: now,
        updated_at: now,
    };
    let mut deps = AdapterDeps::new(creds);
    deps.idle_timeout = Duration::from_millis(300);
    let a = build_adapter(&p, &deps).unwrap();
    let ev = collect(&a, ChatRequest::new("gpt-4o", vec![ChatMessage::user("x")])).await;
    assert!(matches!(terminal_error(&ev), ProviderError::Network(m) if m.contains("idle")));

    deps.idle_timeout = Duration::from_secs(30);
    let a = build_adapter(&p, &deps).unwrap();
    let cancel = CancellationToken::new();
    let s = a.chat(ChatRequest::new("gpt-4o", vec![ChatMessage::user("x")]), cancel.clone()).await.unwrap();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let ev = tokio::time::timeout(Duration::from_secs(2), s.collect::<Vec<_>>())
        .await
        .expect("cancel must end the stream within 2s");
    assert_eq!(ev, vec![ChatEvent::Error(ProviderError::Cancelled)]);
}
