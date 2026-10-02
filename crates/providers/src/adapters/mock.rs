//! "Mock (offline demo)" — deterministic, offline, clearly labelled provider
//! (spec §10). Its output is a pure function of (model id, conversation, tool
//! list, response format), so the Manager and agents can be demoed and tested
//! without keys or network.
//!
//! Scripted models:
//! * `mock-smart` (default) — tool result → summary; JSON schema → a
//!   schema-valid document; planning system prompt → plan JSON; a
//!   `write_file` tool → calls it, then finishes; otherwise a short reply.
//! * `mock-echo` — echoes the last user message.
//! * `mock-tool-roundtrip` — calls the first declared tool once with
//!   schema-valid arguments, then finishes.
//! * `mock-slow` — like smart, slow stream.
//! * `mock-rate-limit` / `mock-quota` / `mock-server-error` — fail with the
//!   corresponding classified error (fallback demos/tests).

use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::catalog::{default_models, MOCK_DISPLAY_NAME};
use crate::http::drive;
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality, ProviderDescriptor,
    ProviderError, ResponseFormat, Role, StopReason, ToolSpec, Usage,
};

/// Mock tuning.
#[derive(Debug, Clone, Copy)]
pub struct MockOptions {
    /// Delay between streamed chunks.
    pub chunk_delay: Duration,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self { chunk_delay: Duration::from_millis(15) }
    }
}

/// The mock adapter.
#[derive(Debug)]
pub struct MockAdapter {
    id: ProviderId,
    name: String,
    options: MockOptions,
}

impl MockAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(id: ProviderId, name: impl Into<String>, options: MockOptions) -> Self {
        let name = name.into();
        // The label must always identify the provider as a mock.
        let name = if name.contains("Mock") { name } else { format!("{name} — {MOCK_DISPLAY_NAME}") };
        Self { id, name, options }
    }
}

/// What the mock decided to do.
#[derive(Debug, Clone, PartialEq)]
pub enum MockScript {
    /// Stream this text.
    Text(String),
    /// Call a tool.
    Tool {
        /// Tool name.
        name: String,
        /// Arguments.
        arguments: Value,
    },
    /// Fail.
    Fail(ProviderError),
}

fn last_user(req: &ChatRequest) -> String {
    req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "demo".into()
    } else {
        out
    }
}

/// Deterministic schema-valid sample (supports the JSON-schema subset used
/// for tools and structured output).
#[must_use]
pub fn sample_from_schema(schema: &Value, key: &str, hint: &str, index: usize) -> Value {
    sample(schema, key, hint, index, 0)
}

fn sample(schema: &Value, key: &str, hint: &str, index: usize, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    if let Some(c) = schema.get("const") {
        return c.clone();
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.get(index % a.len().max(1))) {
        return e.clone();
    }
    if let Some(d) = schema.get("default") {
        return d.clone();
    }
    for comb in ["anyOf", "oneOf", "allOf"] {
        if let Some(first) = schema
            .get(comb)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|s| s.get("type") != Some(&json!("null"))))
        {
            return sample(first, key, hint, index, depth + 1);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null").to_owned(),
        _ if schema.get("properties").is_some() => "object".into(),
        _ if schema.get("items").is_some() => "array".into(),
        _ => "string".into(),
    };
    let k = key.to_ascii_lowercase();
    match ty.as_str() {
        "object" => {
            let mut map = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (name, sub) in props {
                    map.insert(name.clone(), sample(sub, name, hint, index, depth + 1));
                }
            }
            Value::Object(map)
        }
        "array" => {
            let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max = schema.get("maxItems").and_then(Value::as_u64).map(|m| m as usize);
            let plural = ["tasks", "steps", "subtasks", "items"].iter().any(|p| k.contains(p));
            let mut n = min.max(if plural { 3 } else { 1 });
            if let Some(max) = max {
                n = n.min(max);
            }
            let items = schema.get("items").cloned().unwrap_or_else(|| json!({"type": "string"}));
            let is_dep = k.contains("depend") || k.contains("after") || k.contains("blocked");
            if is_dep {
                // Dependencies point at the previous item (a simple chain).
                return if index == 0 { json!([]) } else { json!([format!("t{index}")]) };
            }
            Value::Array((0..n).map(|i| sample(&items, key.trim_end_matches('s'), hint, i, depth + 1)).collect())
        }
        "integer" => json!(schema.get("minimum").and_then(Value::as_i64).unwrap_or(1).max(if k.contains("priority") {
            1
        } else {
            0
        })),
        "number" => json!(schema.get("minimum").and_then(Value::as_f64).unwrap_or(1.0)),
        "boolean" => json!(true),
        "null" => Value::Null,
        _ => {
            let n = index + 1;
            let s = if k == "id" || k.ends_with("_id") || k.ends_with("id") {
                format!("t{n}")
            } else if k.contains("path") || k.contains("file") {
                format!("orkestr-demo/{}.md", slug(hint))
            } else if k.contains("content") || k.contains("body") || k.contains("text") {
                format!("# {}\n\nGenerated by Mock (offline demo).\n", short(hint, 80))
            } else if k.contains("title") || k.contains("name") {
                const TITLES: [&str; 4] =
                    ["Design the approach", "Implement the change", "Write tests", "Review and document"];
                TITLES[index % TITLES.len()].to_owned()
            } else if k.contains("role") || k.contains("agent") {
                const ROLES: [&str; 4] = ["Architect", "Backend", "QA", "Reviewer"];
                ROLES[index % ROLES.len()].to_owned()
            } else if k.contains("summary") || k.contains("description") || k.contains("goal") || k.contains("reason") {
                format!("Mock (offline demo) step {n} for: {}", short(hint, 80))
            } else {
                format!("mock {key}")
            };
            let min_len = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut s = s;
            while s.chars().count() < min_len {
                s.push('.');
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                s = s.chars().take(max as usize).collect();
            }
            json!(s)
        }
    }
}

/// The fixed plan returned for planning prompts without a schema.
#[must_use]
pub fn demo_plan(goal: &str) -> Value {
    let g = short(goal, 120);
    json!({
        "summary": format!("Demo plan (Mock, offline) for: {g}"),
        "tasks": [
            {"id": "t1", "title": "Design the approach", "description": format!("Outline the solution for: {g}"), "role": "Architect", "dependsOn": [], "priority": "High"},
            {"id": "t2", "title": "Implement the change", "description": "Write the code in the agent worktree.", "role": "Backend", "dependsOn": ["t1"], "priority": "High"},
            {"id": "t3", "title": "Write tests", "description": "Cover the change with unit tests.", "role": "QA", "dependsOn": ["t2"], "priority": "Normal"},
            {"id": "t4", "title": "Review", "description": "Review the diff and summarize risks.", "role": "Reviewer", "dependsOn": ["t2", "t3"], "priority": "Normal"}
        ]
    })
}

fn tool_args(tool: &ToolSpec, hint: &str) -> Value {
    sample(&tool.parameters, "", hint, 0, 0)
}

/// Decides the scripted response (pure; unit-tested).
#[must_use]
pub fn script(req: &ChatRequest) -> MockScript {
    let model = req.model.as_str();
    match model {
        "mock-rate-limit" => return MockScript::Fail(ProviderError::RateLimited { retry_after_secs: Some(1) }),
        "mock-quota" => {
            return MockScript::Fail(ProviderError::QuotaExceeded("mock quota exhausted (offline demo)".into()))
        }
        "mock-server-error" => {
            return MockScript::Fail(ProviderError::Server("mock upstream failure (offline demo)".into()))
        }
        _ => {}
    }
    let user = last_user(req);
    let last = req.messages.iter().rev().find(|m| m.role != Role::System);
    let after_tool = last.is_some_and(|m| m.role == Role::Tool);
    if model == "mock-echo" {
        return MockScript::Text(if user.is_empty() { "(empty)".into() } else { user });
    }
    if model == "mock-tool-roundtrip" {
        if let (false, Some(tool)) = (after_tool, req.tools.first()) {
            return MockScript::Tool { name: tool.name.clone(), arguments: tool_args(tool, &user) };
        }
        return MockScript::Text("Tool round-trip complete.".into());
    }
    // mock-smart / mock-slow / anything else.
    if after_tool {
        let result = last.map(|m| short(&m.content, 120)).unwrap_or_default();
        let failed = last.is_some_and(|m| m.is_error);
        return MockScript::Text(if failed {
            format!("The tool reported an error ({result}). Stopping here — Mock (offline demo).")
        } else {
            format!("Done. The tool returned: {result}\n\nTask complete — Mock (offline demo).")
        });
    }
    if let ResponseFormat::JsonSchema { schema, .. } = &req.response_format {
        return MockScript::Text(sample(schema, "", &user, 0, 0).to_string());
    }
    let system = req.system_prompt().unwrap_or_default().to_ascii_lowercase();
    let planning = system.contains("plan") || system.contains("decompose");
    if planning && (req.response_format != ResponseFormat::Text || system.contains("json") || req.tools.is_empty()) {
        return MockScript::Text(demo_plan(&user).to_string());
    }
    if let Some(tool) = req.tools.iter().find(|t| t.name == "write_file") {
        return MockScript::Tool { name: tool.name.clone(), arguments: tool_args(tool, &user) };
    }
    if req.response_format == ResponseFormat::JsonObject {
        return MockScript::Text(
            json!({"answer": format!("Mock (offline demo) answer to: {}", short(&user, 120))}).to_string(),
        );
    }
    MockScript::Text(format!(
        "Hello from Mock (offline demo). You said: \"{}\". I'm a scripted offline provider — connect a real provider in Settings → AI Providers for real answers.",
        short(&user, 200)
    ))
}

fn chunks(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(size.max(1)).map(|c| c.iter().collect()).collect()
}

#[async_trait]
impl ProviderAdapter for MockAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Mock,
            display_name: self.name.clone(),
            is_local: true,
            locality: Locality::Local,
            capabilities: ModelCapabilities {
                tools: true,
                vision: false,
                structured_output: true,
                streaming: true,
                reasoning: false,
                coding_agent: false,
            },
            max_context_tokens: 128_000,
            coding_agent: false,
            cost_model: CostModelKind::Local,
            max_concurrency: 8,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        AuthStatus::Ready { label: "no credentials needed (offline demo)".into() }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(default_models(ProviderKind::Mock, self.id, true))
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            health: ProviderHealth::Healthy,
            latency_ms: Some(0),
            detail: Some("offline demo provider".into()),
            version: None,
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        let plan = script(&request);
        let delay = if request.model == "mock-slow" {
            Duration::from_millis(250).max(self.options.chunk_delay)
        } else {
            self.options.chunk_delay
        };
        let prompt_chars = request.prompt_chars();
        Ok(drive(cancel, move |em| async move {
            let output_chars;
            let stop = match plan {
                MockScript::Fail(e) => return Err(e),
                MockScript::Text(text) => {
                    output_chars = text.len();
                    for c in chunks(&text, 12) {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        em.emit(ChatEvent::TextDelta(c)).await?;
                    }
                    StopReason::EndTurn
                }
                MockScript::Tool { name, arguments } => {
                    output_chars = arguments.to_string().len();
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    em.emit(ChatEvent::ToolCall { id: format!("mock_call_{}", slug(&name)), name, arguments }).await?;
                    StopReason::ToolUse
                }
            };
            em.emit(ChatEvent::Usage(Usage::estimate(prompt_chars, output_chars))).await?;
            em.emit(ChatEvent::Done { stop_reason: stop }).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;
    use futures::StreamExt;

    fn tool(name: &str, schema: Value) -> ToolSpec {
        ToolSpec { name: name.into(), description: "d".into(), parameters: schema }
    }

    #[test]
    fn planning_prompt_returns_plan_json() {
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user("Build a todo app")]);
        r.system = Some("You are the AI Manager. Produce a plan as JSON.".into());
        let MockScript::Text(t) = script(&r) else { panic!() };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["tasks"].as_array().unwrap().len(), 4);
        assert_eq!(v["tasks"][1]["dependsOn"][0], "t1");
    }

    #[test]
    fn schema_output_is_valid_shape() {
        let schema = json!({"type":"object","required":["summary","tasks"],"properties":{
            "summary":{"type":"string"},
            "tasks":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"role":{"type":"string","enum":["Backend","QA"]},"dependsOn":{"type":"array","items":{"type":"string"}},"estimate":{"type":"integer","minimum":2}}}}}});
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user("x")]);
        r.response_format = ResponseFormat::JsonSchema { name: "plan".into(), schema };
        let MockScript::Text(t) = script(&r) else { panic!() };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["tasks"].as_array().unwrap().len(), 3);
        assert_eq!(v["tasks"][0]["role"], "Backend");
        assert_eq!(v["tasks"][1]["role"], "QA");
        assert_eq!(v["tasks"][2]["id"], "t3");
        assert_eq!(v["tasks"][2]["dependsOn"], json!(["t2"]));
        assert_eq!(v["tasks"][0]["estimate"], 2);
    }

    #[test]
    fn agent_task_writes_file_then_finishes() {
        let wf = tool(
            "write_file",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        );
        let mut r = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::system("You are a Backend agent."), ChatMessage::user("Add a README")],
        );
        r.tools = vec![tool("read_file", json!({"type":"object"})), wf];
        let MockScript::Tool { name, arguments } = script(&r) else { panic!("expected tool call") };
        assert_eq!(name, "write_file");
        assert_eq!(arguments["path"], "orkestr-demo/add-a-readme.md");
        assert!(arguments["content"].as_str().unwrap().contains("Add a README"));
        r.messages.push(ChatMessage::assistant_tool_calls("", vec![]));
        r.messages.push(ChatMessage::tool_result("mock_call_write-file", "wrote 40 bytes", false));
        assert!(matches!(script(&r), MockScript::Text(t) if t.contains("Task complete")));
    }

    #[test]
    fn deterministic() {
        let r = ChatRequest::new("mock-smart", vec![ChatMessage::user("hello")]);
        assert_eq!(script(&r), script(&r));
    }

    #[tokio::test]
    async fn stream_shape_and_errors() {
        let m = MockAdapter::new(ProviderId::new(), "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO });
        let s = m
            .chat(
                ChatRequest::new("mock-echo", vec![ChatMessage::user("ping pong ping pong")]),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let items: Vec<_> = s.collect().await;
        let text: String = items
            .iter()
            .filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None })
            .collect();
        assert_eq!(text, "ping pong ping pong");
        assert!(matches!(items[items.len() - 2], ChatEvent::Usage(Usage { estimated: true, .. })));
        assert_eq!(items.last(), Some(&ChatEvent::Done { stop_reason: StopReason::EndTurn }));

        let s = m.chat(ChatRequest::new("mock-rate-limit", vec![]), CancellationToken::new()).await.unwrap();
        let items: Vec<_> = s.collect().await;
        assert_eq!(items, vec![ChatEvent::Error(ProviderError::RateLimited { retry_after_secs: Some(1) })]);
        assert!(m.descriptor().display_name.contains("Mock (offline demo)"));
        let named = MockAdapter::new(ProviderId::new(), "Demo", MockOptions::default());
        assert!(named.descriptor().display_name.contains("Mock (offline demo)"));
    }
}
