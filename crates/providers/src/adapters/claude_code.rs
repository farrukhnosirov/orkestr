//! Claude Code — the official `claude` CLI, using the user's own logged-in
//! session (`claude -p --output-format stream-json --verbose`).
//!
//! Orkestr never reads the CLI's credential files or keychain entries. Login
//! state is unknown until a run succeeds or fails with an auth error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use parking_lot::Mutex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::claude_chat::{ChatPool, CliSpawner};
use super::cli::{self, CliStreamParser};
use crate::adapter::ProviderAdapter;
use crate::catalog::default_models;
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CodingAgentEvent, CodingAgentRequest, CodingAgentStream,
    CostModelKind, FileChangeKind, HealthReport, Locality, ProviderDescriptor, ProviderError, StopReason, ToolChoice,
    Usage,
};

/// Auth label shown for an installed CLI.
pub const SESSION_LABEL: &str = "cli session (managed by Claude Code)";
/// Tools disabled for chat-style runs (pure text answers).
const CHAT_DISALLOWED: &str = "Bash,PowerShell,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Read,Glob,Grep,Task,Agent,TodoWrite,Skill,SlashCommand,ToolSearch,BashOutput,KillShell,ExitPlanMode";

/// Parser for `stream-json` lines (pure; tested with recorded fixtures).
#[derive(Default)]
pub struct ClaudeStreamParser {
    session_sent: bool,
    pending_files: HashMap<String, (String, FileChangeKind)>,
    terminal: bool,
    /// Last CLI-synthesised error text (e.g. "API Error: 401 ..."). Ordinary
    /// assistant text is model output about the user's code and must never
    /// be classified as a provider error.
    last_cli_error: Option<String>,
}

/// True when an assistant event is an error the CLI synthesised itself
/// (API failure, login prompt, usage limit) rather than model output.
fn is_cli_error_message(event: &Value, text: &str) -> bool {
    if event.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
        || event.get("error").is_some_and(|e| !e.is_null())
    {
        return true;
    }
    let t = text.trim_start();
    ["API Error", "Invalid API key", "Claude AI usage limit reached"].iter().any(|p| t.starts_with(p))
        || t.contains("Please run /login")
}

impl ClaudeStreamParser {
    fn tool_use(&mut self, block: &Value, out: &mut Vec<CodingAgentEvent>) {
        let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let name = block.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        if let Some(path) = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str) {
            let kind = match name.as_str() {
                "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Some(FileChangeKind::Modified),
                _ => None,
            };
            if let Some(k) = kind {
                self.pending_files.insert(id.clone(), (path.to_owned(), k));
            }
        }
        out.push(CodingAgentEvent::ToolStarted { call_id: id, tool: name, summary: cli::summarize(&input) });
    }
}

impl CliStreamParser for ClaudeStreamParser {
    fn line(&mut self, line: &str) -> Vec<CodingAgentEvent> {
        let mut out = Vec::new();
        if self.terminal || line.trim().is_empty() {
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            tracing::debug!("ignoring non-JSON line from claude");
            return out;
        };
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "system" => {
                if let (false, Some(sid)) = (self.session_sent, v.get("session_id").and_then(Value::as_str)) {
                    self.session_sent = true;
                    out.push(CodingAgentEvent::Session { session_id: sid.to_owned() });
                }
            }
            "assistant" => {
                for block in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                if is_cli_error_message(&v, t) {
                                    self.last_cli_error = Some(t.to_owned());
                                }
                                out.push(CodingAgentEvent::Chat(ChatEvent::TextDelta(t.to_owned())));
                            }
                        }
                        Some("tool_use") => self.tool_use(block, &mut out),
                        _ => {} // thinking / redacted_thinking: dropped (no-CoT rule)
                    }
                }
            }
            "user" => {
                for block in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let ok = !block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let content = match block.get("content") {
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                        Some(other) => cli::summarize(other),
                        None => String::new(),
                    };
                    let file = self.pending_files.remove(&id);
                    out.push(CodingAgentEvent::ToolFinished {
                        call_id: id,
                        ok,
                        summary: cli::summarize(&Value::String(content)),
                    });
                    if let (true, Some((path, change))) = (ok, file) {
                        out.push(CodingAgentEvent::FileChanged { path, change });
                    }
                }
            }
            "result" => {
                let get = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                let cache_read = get("/usage/cache_read_input_tokens");
                let cache_write = get("/usage/cache_creation_input_tokens");
                out.push(CodingAgentEvent::Chat(ChatEvent::Usage(Usage {
                    input_tokens: get("/usage/input_tokens") + cache_read + cache_write,
                    output_tokens: get("/usage/output_tokens"),
                    cached_input_tokens: cache_read,
                    cache_write_tokens: cache_write,
                    reasoning_tokens: 0,
                    reported_cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                    estimated: false,
                })));
                let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("success");
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let result_text = v.get("result").and_then(Value::as_str).unwrap_or_default();
                self.terminal = true;
                out.push(CodingAgentEvent::Chat(if subtype == "success" && !is_error {
                    ChatEvent::Done { stop_reason: StopReason::EndTurn }
                } else if subtype == "error_max_turns" {
                    ChatEvent::Done { stop_reason: StopReason::Other("max_turns".into()) }
                } else {
                    ChatEvent::Error(cli::classify_cli_message(result_text).unwrap_or_else(|| {
                        ProviderError::Server(format!(
                            "claude run failed ({subtype}): {}",
                            result_text.chars().take(300).collect::<String>()
                        ))
                    }))
                }));
            }
            other => tracing::debug!(kind = other, "ignoring unknown claude stream event"),
        }
        out
    }

    fn finish(&mut self, exit_code: Option<i32>, stderr_tail: &str) -> CodingAgentEvent {
        // Only CLI-originated text is classified: the stderr tail and an
        // error message the CLI itself emitted, never the model's answer.
        let combined = format!("{stderr_tail}\n{}", self.last_cli_error.as_deref().unwrap_or_default());
        let err = cli::classify_cli_message(&combined).unwrap_or_else(|| match exit_code {
            Some(0) => ProviderError::Server("claude exited without a result event".into()),
            code => ProviderError::Server(format!(
                "claude exited with code {}: {}",
                code.map_or_else(|| "signal".to_owned(), |c| c.to_string()),
                stderr_tail.trim().chars().rev().take(300).collect::<String>().chars().rev().collect::<String>()
            )),
        });
        cli::error_event(err)
    }
}

/// Claude Code adapter.
#[derive(Debug)]
pub struct ClaudeCodeAdapter {
    id: ProviderId,
    name: String,
    binary_override: Option<String>,
    last_auth_error: Arc<Mutex<Option<String>>>,
    verified: Arc<Mutex<bool>>,
    /// Persistent processes of chat-style calls (AI Manager, reviews).
    chats: Arc<ChatPool>,
}

impl ClaudeCodeAdapter {
    /// Creates the adapter. `binary_override` = absolute path pinned by the user.
    #[must_use]
    pub fn new(id: ProviderId, name: impl Into<String>, binary_override: Option<String>) -> Self {
        let chat_args = [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--disallowedTools",
            CHAT_DISALLOWED,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        Self {
            id,
            name: name.into(),
            binary_override,
            last_auth_error: Arc::default(),
            verified: Arc::default(),
            chats: Arc::new(ChatPool::new(Arc::new(CliSpawner), chat_args)),
        }
    }

    /// Learns the login state from real runs (never from credential files).
    fn observe(&self, stream: CodingAgentStream) -> CodingAgentStream {
        use futures::StreamExt;
        let verified = Arc::clone(&self.verified);
        let last_err = Arc::clone(&self.last_auth_error);
        stream
            .inspect(move |ev| match ev {
                CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(m))) => *last_err.lock() = Some(m.clone()),
                CodingAgentEvent::Chat(ChatEvent::Done { .. }) => {
                    *verified.lock() = true;
                    *last_err.lock() = None;
                }
                _ => {}
            })
            .boxed()
    }

    fn binary(&self) -> Option<PathBuf> {
        cli::resolve_binary("claude", self.binary_override.as_deref())
    }

    fn base_args(model: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> =
            ["-p", "--output-format", "stream-json", "--verbose"].iter().map(|s| (*s).to_owned()).collect();
        if let Some(m) = model.filter(|m| !m.trim().is_empty() && *m != "default") {
            args.push("--model".into());
            args.push(m.to_owned());
        }
        args
    }

    /// Builds the argument vector for a coding-agent run (pure; tested).
    #[must_use]
    pub fn agent_args(req: &CodingAgentRequest) -> (Vec<String>, String) {
        let mut args = Self::base_args(req.model.as_deref());
        if let Some(n) = req.max_turns {
            args.push("--max-turns".into());
            args.push(n.to_string());
        }
        if let Some(s) = req.resume_session.as_deref().filter(|s| !s.is_empty()) {
            args.push("--resume".into());
            args.push(s.to_owned());
        }
        args.push("--permission-mode".into());
        args.push(if req.tool_policy.allow_writes { "acceptEdits" } else { "default" }.into());
        if !req.tool_policy.allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.push(req.tool_policy.allowed_tools.join(","));
        }
        if !req.tool_policy.disallowed_tools.is_empty() {
            args.push("--disallowedTools".into());
            args.push(req.tool_policy.disallowed_tools.join(","));
        }
        let mut prompt = String::new();
        if let Some(sys) = req.system_append.as_deref().filter(|s| !s.trim().is_empty()) {
            // Passed in the prompt (not as an argument) so multi-line text never
            // needs shell escaping (npm `.cmd` shims on Windows).
            prompt.push_str("Additional instructions:\n");
            prompt.push_str(sys);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&req.prompt);
        (args, prompt)
    }

    /// Argument vector of a coding-agent run with the per-tool permission
    /// bridge (pure; tested): stream-json in both directions and
    /// `--permission-prompt-tool stdio`, so every tool call the CLI would
    /// prompt for in a terminal becomes a `can_use_tool` control request.
    /// The mode is `default` (never `acceptEdits`): edits are asked too.
    #[must_use]
    pub fn bridged_args(req: &CodingAgentRequest, agents_file: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "default",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        if let Some(m) =
            req.model.as_deref().filter(|m| *m != "default" && crate::claude_code_session::manager::is_valid_model(m))
        {
            args.push("--model".into());
            args.push(m.to_owned());
        }
        if let Some(n) = req.max_turns {
            args.push("--max-turns".into());
            args.push(n.to_string());
        }
        if let Some(s) = req.resume_session.as_deref().filter(|s| crate::claude_code_session::history::is_session_id(s))
        {
            args.push("--resume".into());
            args.push(s.to_owned());
        }
        if !req.tool_policy.allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.push(req.tool_policy.allowed_tools.join(","));
        }
        if !req.tool_policy.disallowed_tools.is_empty() {
            args.push("--disallowedTools".into());
            args.push(req.tool_policy.disallowed_tools.join(","));
        }
        if let Some(sub) = req.subagent.as_ref().filter(|s| is_valid_agent_name(&s.name)) {
            if let Some(file) = agents_file {
                args.push("--agents".into());
                args.push(file.to_owned());
            }
            args.push("--agent".into());
            args.push(sub.name.clone());
        }
        args
    }

    fn spawn_bridged(
        &self,
        req: CodingAgentRequest,
        cwd: PathBuf,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        use futures::StreamExt;
        let bin =
            self.binary().ok_or_else(|| ProviderError::Auth("Claude Code CLI (`claude`) is not installed".into()))?;
        let bridge =
            req.permission_bridge.clone().ok_or_else(|| ProviderError::BadRequest("no permission bridge".into()))?;
        let replies =
            bridge.take().ok_or_else(|| ProviderError::BadRequest("the permission bridge was already used".into()))?;
        // `--agents` takes a file with `--print`: the definition is written
        // to a private temp file that lives as long as the run.
        let agents_file = match req.subagent.as_ref().and_then(|s| s.definition.as_ref().map(|d| (s, d))) {
            Some((sub, def)) if is_valid_agent_name(&sub.name) => {
                let dir = std::env::temp_dir().join("orkestr-claude-agents");
                std::fs::create_dir_all(&dir)
                    .map_err(|e| ProviderError::Server(format!("creating the agents file: {e}")))?;
                let path = dir.join(format!("{}.json", uuid::Uuid::now_v7()));
                let body = serde_json::json!({ sub.name.clone(): def }).to_string();
                std::fs::write(&path, body)
                    .map_err(|e| ProviderError::Server(format!("writing the agents file: {e}")))?;
                Some(path)
            }
            _ => None,
        };
        let args = Self::bridged_args(&req, agents_file.as_ref().and_then(|p| p.to_str()));
        let (_, prompt) = Self::agent_args(&req);
        let stream = run_bridged(bin, args, cwd, prompt, replies, agents_file, cancel);
        let verified = Arc::clone(&self.verified);
        let last_err = Arc::clone(&self.last_auth_error);
        Ok(stream
            .inspect(move |ev| match ev {
                CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(m))) => *last_err.lock() = Some(m.clone()),
                CodingAgentEvent::Chat(ChatEvent::Done { .. }) => {
                    *verified.lock() = true;
                    *last_err.lock() = None;
                }
                _ => {}
            })
            .boxed())
    }

    fn spawn(
        &self,
        args: Vec<String>,
        cwd: PathBuf,
        prompt: String,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        use futures::StreamExt;
        let bin =
            self.binary().ok_or_else(|| ProviderError::Auth("Claude Code CLI (`claude`) is not installed".into()))?;
        let stream = cli::run_cli(bin, args, cwd, prompt, ClaudeStreamParser::default(), cancel);
        let verified = Arc::clone(&self.verified);
        let last_err = Arc::clone(&self.last_auth_error);
        // Learn the login state from real runs (never from credential files).
        Ok(stream
            .inspect(move |ev| match ev {
                CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(m))) => *last_err.lock() = Some(m.clone()),
                CodingAgentEvent::Chat(ChatEvent::Done { .. }) => {
                    *verified.lock() = true;
                    *last_err.lock() = None;
                }
                _ => {}
            })
            .boxed())
    }
}

#[async_trait]
impl ProviderAdapter for ClaudeCodeAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::ClaudeCode,
            display_name: self.name.clone(),
            is_local: false,
            locality: Locality::Cloud,
            capabilities: ModelCapabilities {
                tools: false,
                vision: false,
                structured_output: false,
                streaming: true,
                reasoning: true,
                coding_agent: true,
            },
            max_context_tokens: 200_000,
            coding_agent: true,
            cost_model: CostModelKind::Subscription,
            max_concurrency: 2,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        if self.binary().is_none() {
            return AuthStatus::NotInstalled { binary: "claude".into() };
        }
        if let Some(reason) = self.last_auth_error.lock().clone() {
            return AuthStatus::Invalid { reason };
        }
        if *self.verified.lock() {
            AuthStatus::Ready { label: SESSION_LABEL.into() }
        } else {
            AuthStatus::Unverified { label: SESSION_LABEL.into() }
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(default_models(ProviderKind::ClaudeCode, self.id, false))
    }

    async fn health(&self) -> HealthReport {
        let Some(bin) = self.binary() else { return HealthReport::down("not installed (`claude` not found on PATH)") };
        let start = std::time::Instant::now();
        match cli::probe(&bin, &["--version"]).await {
            Ok((true, text)) => {
                let version = cli::parse_version(&text);
                HealthReport {
                    health: ProviderHealth::Healthy,
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                    detail: Some(format!(
                        "Claude Code {} · {SESSION_LABEL}",
                        version.clone().unwrap_or_else(|| "?".into())
                    )),
                    version,
                }
            }
            Ok((false, text)) => {
                HealthReport::down(format!("`claude --version` failed: {}", text.chars().take(200).collect::<String>()))
            }
            Err(e) => HealthReport::from_error(&e, None),
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        // Orkestr's own tools (AI Manager, reviews) are emulated: the CLI
        // runs with all of its tools disabled, the tool catalogue goes into
        // the prompt and a `{"tool_calls": […]}` answer becomes tool calls.
        // One persistent stream-json process per conversation (see
        // `claude_chat`): later turns send only the new messages.
        let tools = !request.tools.is_empty() && request.tool_choice != ToolChoice::None;
        let protocol = if tools { tool_protocol(&request) } else { String::new() };
        let bin =
            self.binary().ok_or_else(|| ProviderError::Auth("Claude Code CLI (`claude`) is not installed".into()))?;
        let first = cli::render_prompt(&request);
        let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        let stream = self.chats.turn(bin, cli::chat_scratch_dir()?, request, first, protocol, cancel.clone());
        let stream = self.observe(stream);
        if !tools {
            return Ok(cli::chat_only(stream));
        }
        Ok(emulate_tool_calls(stream, names, cancel))
    }

    async fn run_coding_agent(
        &self,
        request: CodingAgentRequest,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        let cwd = cli::validate_cwd(&request.cwd)?;
        if request.permission_bridge.is_some() {
            return self.spawn_bridged(request, cwd, cancel);
        }
        let (args, prompt) = Self::agent_args(&request);
        self.spawn(args, cwd, prompt, cancel)
    }
}

/// Whether a Claude agent name is a plain token (`code-reviewer`, `Explore`).
#[must_use]
pub fn is_valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// Tool-call instructions appended to a chat prompt (emulated tools).
fn tool_protocol(req: &ChatRequest) -> String {
    let mut s = String::from(
        "\n\n## Tools\nYou cannot run tools yourself. To call one or more of the tools below, answer with ONLY this JSON \
         object (no prose, no code fences): {\"tool_calls\": [{\"name\": \"<tool>\", \"arguments\": {…}}]}. \
         The results come back in the next message. When you need no tool, answer normally in plain text.\n",
    );
    if let ToolChoice::Required = req.tool_choice {
        s.push_str("You must call at least one tool now.\n");
    }
    if let ToolChoice::Specific(name) = &req.tool_choice {
        s.push_str(&format!("You must call the tool `{name}` now.\n"));
    }
    for t in &req.tools {
        s.push_str(&format!("- `{}`: {} — arguments schema: {}\n", t.name, t.description.trim(), t.parameters));
    }
    s
}

/// Extracts `{"tool_calls": [{name, arguments}]}` from an answer (pure; tested).
#[must_use]
pub fn parse_emulated_tool_calls(text: &str, known: &[String]) -> Option<Vec<(String, Value)>> {
    let t = text.trim();
    let t =
        t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).map_or(t, |r| r.trim_end().trim_end_matches("```"));
    let candidates = [t.trim().to_owned(), {
        let (a, b) = (t.find('{'), t.rfind('}'));
        match (a, b) {
            (Some(a), Some(b)) if b > a => t[a..=b].to_owned(),
            _ => String::new(),
        }
    }];
    for c in candidates.iter().filter(|c| !c.is_empty()) {
        let Ok(v) = serde_json::from_str::<Value>(c) else { continue };
        let Some(calls) = v.get("tool_calls").and_then(Value::as_array) else { continue };
        let out: Vec<(String, Value)> = calls
            .iter()
            .filter_map(|c| {
                let name = c.get("name").and_then(Value::as_str)?.to_owned();
                known.contains(&name).then_some(())?;
                let args = match c.get("arguments") {
                    Some(Value::String(s)) => {
                        serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({"__invalid_json": s}))
                    }
                    Some(v) => v.clone(),
                    None => serde_json::json!({}),
                };
                Some((name, args))
            })
            .collect();
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

fn emulate_tool_calls(inner: CodingAgentStream, known: Vec<String>, cancel: CancellationToken) -> ChatStream {
    use futures::StreamExt;
    crate::http::drive(cancel, move |em: crate::http::Emitter<ChatEvent>| async move {
        let mut inner = inner;
        let mut text = String::new();
        while let Some(ev) = inner.next().await {
            match ev {
                CodingAgentEvent::Chat(ChatEvent::TextDelta(t)) => text.push_str(&t),
                CodingAgentEvent::Chat(ChatEvent::Done { stop_reason }) => {
                    if let Some(calls) = parse_emulated_tool_calls(&text, &known) {
                        for (name, arguments) in calls {
                            let id = format!("cc_{}", uuid::Uuid::now_v7().simple());
                            em.emit(ChatEvent::ToolCall { id, name, arguments }).await?;
                        }
                        em.emit(ChatEvent::Done { stop_reason: StopReason::ToolUse }).await?;
                    } else {
                        if !text.is_empty() {
                            em.emit(ChatEvent::TextDelta(std::mem::take(&mut text))).await?;
                        }
                        em.emit(ChatEvent::Done { stop_reason }).await?;
                    }
                    return Ok(());
                }
                CodingAgentEvent::Chat(other) => em.emit(other).await?,
                _ => {}
            }
        }
        Ok(())
    })
}

async fn write_line(stdin: &mut Option<tokio::process::ChildStdin>, line: &str) {
    use tokio::io::AsyncWriteExt;
    let Some(si) = stdin.as_mut() else { return };
    let ok =
        si.write_all(line.as_bytes()).await.is_ok() && si.write_all(b"\n").await.is_ok() && si.flush().await.is_ok();
    if !ok {
        *stdin = None;
    }
}

/// Removes the temporary `--agents` file when the run ends.
struct TempFile(Option<PathBuf>);

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Runs the CLI with the stdio permission bridge: the prompt goes in as a
/// stream-json user message, every `can_use_tool` request is surfaced as
/// [`CodingAgentEvent::PermissionRequest`] and answered with the caller's
/// reply; stdin is closed after the `result` so the CLI exits.
fn run_bridged(
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    prompt: String,
    mut replies: tokio::sync::mpsc::UnboundedReceiver<crate::types::CliPermissionReply>,
    agents_file: Option<PathBuf>,
    cancel: CancellationToken,
) -> CodingAgentStream {
    use crate::claude_code_session::control;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    crate::http::drive(cancel, move |em: crate::http::Emitter<CodingAgentEvent>| async move {
        let _cleanup = TempFile(agents_file);
        let mut cmd = crate::claude_code_session::manager::sanitized_command(&program, &args);
        cmd.current_dir(&cwd);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProviderError::Auth(format!("`{}` not found — install the official CLI", program.display()))
            } else {
                ProviderError::Server(format!("failed to start `{}`: {e}", program.display()))
            }
        })?;
        let mut guard = cli::TreeGuard(child.id());
        tracing::info!(program = %program.display(), cwd = %cwd.display(), "Claude Code agent started (permission bridge)");
        let mut stdin = child.stdin.take();
        write_line(&mut stdin, &control::control_request("orkestr-init-1", control::initialize())).await;
        write_line(&mut stdin, &control::user_message(&prompt)).await;
        let stderr_task = child.stderr.take().map(|mut se| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while let Ok(n) = se.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 8192 {
                        buf.drain(..buf.len() - 4096);
                    }
                }
                let start = buf.len().saturating_sub(4096);
                String::from_utf8_lossy(&buf[start..]).into_owned()
            })
        });
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Server("CLI stdout unavailable".into()))?;
        let mut lines = BufReader::new(stdout).lines();
        let mut parser = ClaudeStreamParser::default();
        loop {
            let next = tokio::time::timeout(cli::CLI_IDLE_TIMEOUT, lines.next_line())
                .await
                .map_err(|_| {
                    ProviderError::Server(format!("CLI produced no output for {}s", cli::CLI_IDLE_TIMEOUT.as_secs()))
                })?
                .map_err(|e| ProviderError::Server(format!("reading CLI output: {e}")))?;
            let Some(line) = next else { break };
            let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            match v.get("type").and_then(Value::as_str) {
                Some("control_request") => {
                    let rid = v.get("request_id").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let req = v.get("request").cloned().unwrap_or(Value::Null);
                    if req.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
                        write_line(&mut stdin, &control::error_response(&rid, "not supported by Orkestr")).await;
                        continue;
                    }
                    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
                    let input = req.get("input").cloned().unwrap_or(Value::Null);
                    em.emit(CodingAgentEvent::PermissionRequest {
                        request_id: rid.clone(),
                        tool: str_of("tool_name").unwrap_or_default(),
                        tool_use_id: str_of("tool_use_id"),
                        input: input.clone(),
                        description: str_of("description"),
                        blocked_path: str_of("blocked_path"),
                    })
                    .await?;
                    let reply = loop {
                        match replies.recv().await {
                            Some(r) if r.request_id == rid => break Some(r),
                            Some(_) => {}
                            None => break None,
                        }
                    };
                    let answer = match reply {
                        Some(r) if r.allow => control::allow(&rid, &input, None),
                        Some(r) => control::deny(&rid, r.message.as_deref().unwrap_or("Orkestr denied this action.")),
                        None => control::deny(&rid, "Orkestr stopped this run."),
                    };
                    write_line(&mut stdin, &answer).await;
                }
                Some("control_response" | "control_cancel_request" | "stream_event") => {}
                _ => {
                    for ev in parser.line(&line) {
                        em.emit(ev).await?;
                    }
                    if parser.terminal {
                        stdin = None; // EOF: the CLI exits after the turn
                    }
                }
            }
        }
        drop(stdin);
        let status = child.wait().await.map_err(|e| ProviderError::Server(format!("waiting for CLI: {e}")))?;
        guard.0 = None;
        let stderr_tail = match stderr_task {
            Some(t) => t.await.unwrap_or_default(),
            None => String::new(),
        };
        let terminal = parser.finish(status.code(), &stderr_tail);
        em.emit(terminal).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CliToolPolicy;

    /// Recorded `claude -p --output-format stream-json --verbose` transcript
    /// (shape of Claude Code 1.x/2.x; content shortened).
    pub const FIXTURE: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp/wt","session_id":"9f1c-session","tools":["Bash","Edit","Read"],"model":"claude-sonnet-4-5","permissionMode":"acceptEdits"}
{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"internal reasoning that must never surface"},{"type":"text","text":"I'll create the file."}],"usage":{"input_tokens":10,"output_tokens":5}},"session_id":"9f1c-session"}
{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use","id":"toolu_1","name":"Write","input":{"file_path":"/tmp/wt/hello.txt","content":"hi"}}]},"session_id":"9f1c-session"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"File created successfully at: /tmp/wt/hello.txt"}]},"session_id":"9f1c-session"}
{"type":"assistant","message":{"id":"msg_2","content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"rm -rf /","description":"bad"}}]},"session_id":"9f1c-session"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_2","type":"tool_result","content":"Permission denied","is_error":true}]},"session_id":"9f1c-session"}
{"type":"some_future_event","data":1}
{"type":"assistant","message":{"id":"msg_3","content":[{"type":"text","text":"Done."}]},"session_id":"9f1c-session"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":4200,"num_turns":3,"result":"Done.","session_id":"9f1c-session","total_cost_usd":0.0123,"usage":{"input_tokens":20,"cache_creation_input_tokens":100,"cache_read_input_tokens":300,"output_tokens":42}}"#;

    fn parse(fixture: &str) -> Vec<CodingAgentEvent> {
        let mut p = ClaudeStreamParser::default();
        let mut out: Vec<CodingAgentEvent> = fixture.lines().flat_map(|l| p.line(l)).collect();
        if !out.iter().any(|e| matches!(e, CodingAgentEvent::Chat(c) if c.is_terminal())) {
            out.push(p.finish(Some(1), "boom"));
        }
        out
    }

    #[test]
    fn parses_recorded_transcript() {
        let ev = parse(FIXTURE);
        assert_eq!(ev[0], CodingAgentEvent::Session { session_id: "9f1c-session".into() });
        assert_eq!(ev[1], CodingAgentEvent::Chat(ChatEvent::TextDelta("I'll create the file.".into())));
        assert!(
            matches!(&ev[2], CodingAgentEvent::ToolStarted { call_id, tool, summary } if call_id == "toolu_1" && tool == "Write" && summary == "/tmp/wt/hello.txt")
        );
        assert!(matches!(&ev[3], CodingAgentEvent::ToolFinished { ok: true, .. }));
        assert_eq!(
            ev[4],
            CodingAgentEvent::FileChanged { path: "/tmp/wt/hello.txt".into(), change: FileChangeKind::Modified }
        );
        assert!(matches!(&ev[6], CodingAgentEvent::ToolFinished { ok: false, call_id, .. } if call_id == "toolu_2"));
        assert!(
            matches!(&ev[8], CodingAgentEvent::Chat(ChatEvent::Usage(Usage { input_tokens: 420, cached_input_tokens: 300, output_tokens: 42, reported_cost_usd: Some(c), .. })) if (*c - 0.0123).abs() < 1e-9)
        );
        assert_eq!(ev[9], CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn }));
        assert_eq!(ev.len(), 10);
        assert!(!format!("{ev:?}").contains("internal reasoning"), "thinking must be dropped");
    }

    #[test]
    fn error_results_are_classified() {
        let limit = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude AI usage limit reached|1760000000","usage":{}}"#;
        let ev = parse(limit);
        assert!(matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::QuotaExceeded(_))))));
        let login = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Invalid API key · Please run /login"}]}}"#;
        let ev = parse(login);
        assert!(matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(_))))));
        let crash = parse("not json at all");
        assert!(
            matches!(crash.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Server(m)))) if m.contains("code 1"))
        );
    }

    #[test]
    fn crash_after_model_text_is_not_classified_from_model_output() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Now I'll add the authentication check that returns 401 when the quota is exceeded"}]}}"#;
        let ev = parse(text);
        assert!(
            matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Server(m)))) if m.contains("code 1") && m.contains("boom")),
            "got {:?}",
            ev.last()
        );
        let api = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\"}}"}]}}"#;
        let ev = parse(api);
        assert!(matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(_))))));
    }

    #[test]
    fn agent_args_encode_policy() {
        let req = CodingAgentRequest {
            prompt: "do it".into(),
            system_append: Some("line1\nline2".into()),
            cwd: "/tmp".into(),
            model: Some("opus".into()),
            max_turns: Some(7),
            resume_session: Some("s1".into()),
            tool_policy: CliToolPolicy {
                allowed_tools: vec!["Read".into(), "Edit".into()],
                disallowed_tools: vec!["Bash".into()],
                allow_writes: true,
            },
            subagent: None,
            permission_bridge: None,
        };
        let (args, prompt) = ClaudeCodeAdapter::agent_args(&req);
        let joined = args.join(" ");
        assert!(joined.starts_with("-p --output-format stream-json --verbose --model opus --max-turns 7 --resume s1 --permission-mode acceptEdits"));
        assert!(joined.ends_with("--allowedTools Read,Edit --disallowedTools Bash"));
        assert!(args.iter().all(|a| !a.contains('\n')), "multi-line text goes through stdin");
        assert!(prompt.starts_with("Additional instructions:\nline1\nline2") && prompt.ends_with("do it"));
    }

    #[test]
    fn bridged_args_route_permissions_through_stdio() {
        let (bridge, _tx) = crate::types::PermissionBridge::new();
        let req = CodingAgentRequest {
            prompt: "p".into(),
            system_append: None,
            cwd: "/tmp".into(),
            model: Some("haiku".into()),
            max_turns: Some(5),
            resume_session: Some("not-a-uuid".into()),
            tool_policy: CliToolPolicy {
                allowed_tools: vec!["Read(./**)".into()],
                disallowed_tools: vec!["Read(**/.env)".into()],
                allow_writes: true,
            },
            subagent: Some(crate::types::CliSubagent { name: "code-reviewer".into(), definition: None }),
            permission_bridge: Some(bridge),
        };
        let a = ClaudeCodeAdapter::bridged_args(&req, Some("/tmp/agents.json")).join(" ");
        assert!(a.starts_with("--print --input-format stream-json --output-format stream-json --verbose --permission-prompt-tool stdio --permission-mode default"), "{a}");
        assert!(!a.contains("acceptEdits") && !a.contains("dangerously"), "edits are asked too: {a}");
        assert!(a.contains("--model haiku --max-turns 5"));
        assert!(!a.contains("--resume"), "invalid session ids are dropped");
        assert!(a.ends_with("--agents /tmp/agents.json --agent code-reviewer"), "{a}");
        let mut bad = req.clone();
        bad.subagent = Some(crate::types::CliSubagent { name: "--evil".into(), definition: None });
        assert!(!ClaudeCodeAdapter::bridged_args(&bad, None).join(" ").contains("--agent"));
    }

    #[test]
    fn emulated_tool_calls_are_parsed_strictly() {
        let known = vec!["list_tasks".to_owned(), "create_task".to_owned()];
        let calls = parse_emulated_tool_calls(
            "```json\n{\"tool_calls\": [{\"name\": \"create_task\", \"arguments\": {\"title\": \"x\"}}, {\"name\": \"rm_rf\", \"arguments\": {}}]}\n```",
            &known,
        )
        .unwrap();
        assert_eq!(calls, vec![("create_task".to_owned(), serde_json::json!({"title": "x"}))]);
        let s = parse_emulated_tool_calls(
            "Sure: {\"tool_calls\": [{\"name\": \"list_tasks\", \"arguments\": \"{}\"}]}",
            &known,
        );
        assert_eq!(s.unwrap()[0].1, serde_json::json!({}));
        assert!(parse_emulated_tool_calls("Here is the plan: no tools needed.", &known).is_none());
        assert!(parse_emulated_tool_calls("{\"tool_calls\": [{\"name\": \"unknown\"}]}", &known).is_none());
    }
}
