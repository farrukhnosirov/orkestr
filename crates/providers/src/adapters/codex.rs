//! OpenAI Codex — the official `codex` CLI with the user's own sign-in
//! (ChatGPT plan or API key; `codex login`). Orkestr never reads
//! `~/.codex/auth.json`. ADR-0047 (supersedes the launch-time-only gating of
//! ADR-0012 for Codex):
//!
//! * coding agents run on `codex app-server` (`thread/start` + `turn/start`)
//!   with `approvalPolicy: untrusted` — every command / file change Codex
//!   wants to make is asked over the permission bridge and decided by the
//!   agent runtime (permission engine + Dangerous Action Guard + approvals);
//!   `danger-full-access` is never used;
//! * chat calls (Manager, reviews) use a persistent process pool
//!   ([`CodexChatBackend`] in the shared `claude_chat` pool);
//! * [`CodexStreamParser`] parses the legacy `codex exec --json` stream.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::claude_chat::{ChatPool, CliSpawner, PoolBackend, ProcSpawner, TurnStatus};
use super::claude_code;
use super::cli::{self, CliStreamParser};
use crate::adapter::ProviderAdapter;
use crate::catalog::default_models;
use crate::codex_app::{self, conn::AppServer, conn::Inbound, map};
use crate::http::Emitter;
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CodingAgentEvent, CodingAgentRequest, CodingAgentStream,
    CostModelKind, FileChangeKind, HealthReport, Locality, ProviderDescriptor, ProviderError, StopReason, ToolChoice,
    Usage,
};

/// Auth label shown for a logged-in CLI.
pub const SESSION_LABEL: &str = "cli session (managed by Codex)";

/// Parser for `codex exec --json` lines (pure; tested with recorded fixtures).
#[derive(Default)]
pub struct CodexStreamParser {
    started: HashSet<String>,
    usage: Option<Usage>,
    terminal: bool,
    last_error: Option<String>,
}

fn item_tool(item: &Value) -> Option<(String, String)> {
    let ty = item.get("type").and_then(Value::as_str)?;
    match ty {
        "command_execution" => Some(("shell".into(), item.get("command").map(cli::summarize).unwrap_or_default())),
        "file_change" => {
            let paths: Vec<&str> = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|c| c.iter().filter_map(|x| x.get("path").and_then(Value::as_str)).collect())
                .unwrap_or_default();
            Some(("apply_patch".into(), paths.join(", ")))
        }
        "mcp_tool_call" => {
            let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            Some((format!("{server}.{tool}"), String::new()))
        }
        "web_search" => {
            Some(("web_search".into(), item.get("query").and_then(Value::as_str).unwrap_or_default().to_owned()))
        }
        _ => None, // agent_message, reasoning (dropped), todo_list, unknown
    }
}

impl CodexStreamParser {
    fn item_completed(&mut self, item: &Value, out: &mut Vec<CodingAgentEvent>) {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "agent_message" => {
                if let Some(t) = item.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    out.push(CodingAgentEvent::Chat(ChatEvent::TextDelta(t.to_owned())));
                }
            }
            "reasoning" => {} // hidden reasoning: dropped by design
            "error" => {
                self.last_error = item.get("message").and_then(Value::as_str).map(str::to_owned);
            }
            _ => {
                let Some((tool, summary)) = item_tool(item) else { return };
                if !self.started.remove(&id) {
                    out.push(CodingAgentEvent::ToolStarted {
                        call_id: id.clone(),
                        tool: tool.clone(),
                        summary: summary.clone(),
                    });
                }
                let status = item.get("status").and_then(Value::as_str).unwrap_or("completed");
                let exit_ok = item.get("exit_code").and_then(Value::as_i64).is_none_or(|c| c == 0);
                let ok = status == "completed" && exit_ok;
                let detail = item
                    .get("aggregated_output")
                    .and_then(Value::as_str)
                    .map_or(summary, |o| cli::summarize(&Value::String(o.to_owned())));
                out.push(CodingAgentEvent::ToolFinished { call_id: id, ok, summary: detail });
                if ok && item.get("type").and_then(Value::as_str) == Some("file_change") {
                    for c in item.get("changes").and_then(Value::as_array).into_iter().flatten() {
                        let Some(path) = c.get("path").and_then(Value::as_str) else { continue };
                        let change = match c.get("kind").and_then(Value::as_str) {
                            Some("add") => FileChangeKind::Added,
                            Some("delete") => FileChangeKind::Deleted,
                            _ => FileChangeKind::Modified,
                        };
                        out.push(CodingAgentEvent::FileChanged { path: path.to_owned(), change });
                    }
                }
            }
        }
    }
}

impl CliStreamParser for CodexStreamParser {
    fn line(&mut self, line: &str) -> Vec<CodingAgentEvent> {
        let mut out = Vec::new();
        if self.terminal || line.trim().is_empty() {
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            tracing::debug!("ignoring non-JSON line from codex");
            return out;
        };
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "thread.started" => {
                if let Some(t) = v.get("thread_id").and_then(Value::as_str) {
                    out.push(CodingAgentEvent::Session { session_id: t.to_owned() });
                }
            }
            "item.started" => {
                if let Some(item) = v.get("item") {
                    if let Some((tool, summary)) = item_tool(item) {
                        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                        self.started.insert(id.clone());
                        out.push(CodingAgentEvent::ToolStarted { call_id: id, tool, summary });
                    }
                }
            }
            "item.completed" => {
                if let Some(item) = v.get("item").cloned() {
                    self.item_completed(&item, &mut out);
                }
            }
            "turn.completed" => {
                let get = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                let usage = Usage {
                    input_tokens: get("/usage/input_tokens"),
                    cached_input_tokens: get("/usage/cached_input_tokens"),
                    output_tokens: get("/usage/output_tokens"),
                    ..Usage::default()
                };
                self.usage = Some(usage);
                self.terminal = true;
                out.push(CodingAgentEvent::Chat(ChatEvent::Usage(usage)));
                out.push(CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn }));
            }
            "turn.failed" => {
                let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("turn failed").to_owned();
                self.terminal = true;
                out.push(cli::error_event(cli::classify_cli_message(&msg).unwrap_or(ProviderError::Server(msg))));
            }
            "error" => {
                // Non-fatal notices (e.g. reconnect attempts) also use `error`;
                // remember it and decide at exit.
                self.last_error = v.get("message").and_then(Value::as_str).map(str::to_owned);
            }
            "turn.started" | "item.updated" => {}
            other => tracing::debug!(kind = other, "ignoring unknown codex event"),
        }
        out
    }

    fn finish(&mut self, exit_code: Option<i32>, stderr_tail: &str) -> CodingAgentEvent {
        let combined = format!("{}\n{stderr_tail}", self.last_error.clone().unwrap_or_default());
        let err = cli::classify_cli_message(&combined).unwrap_or_else(|| {
            let detail = self.last_error.clone().unwrap_or_else(|| stderr_tail.trim().chars().take(300).collect());
            ProviderError::Server(format!(
                "codex exited with code {}: {detail}",
                exit_code.map_or_else(|| "signal".into(), |c| c.to_string())
            ))
        });
        cli::error_event(err)
    }
}

/// Persistent Codex app-server threads for chat-style calls (AI Manager,
/// reviews): one process + one ephemeral read-only thread per
/// conversation; later turns send only the new messages (`claude_chat`
/// pool logic).
pub struct CodexChatBackend {
    spawner: Arc<dyn ProcSpawner>,
}

impl CodexChatBackend {
    /// Creates the backend.
    #[must_use]
    pub fn new(spawner: Arc<dyn ProcSpawner>) -> Self {
        Self { spawner }
    }
}

/// A pooled Codex chat process.
pub struct CodexChatProc {
    server: Arc<AppServer>,
    inbound: tokio::sync::mpsc::UnboundedReceiver<Inbound>,
    thread_id: String,
    effort: Option<String>,
}

#[async_trait]
impl PoolBackend for CodexChatBackend {
    type Proc = CodexChatProc;

    async fn start(&self, program: &Path, cwd: &Path, model: &str) -> Result<CodexChatProc, ProviderError> {
        let (server, inbound) = AppServer::start(&*self.spawner, program, cwd).await?;
        let (model, effort) = codex_app::split_model(Some(model));
        let params =
            codex_app::thread_params(&cwd.to_string_lossy(), model.as_deref(), "read-only", "never", None, true);
        let r = server.request_with("thread/start", params, codex_app::conn::THREAD_TIMEOUT).await?;
        let thread_id = r
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Server("Codex did not return a thread id".into()))?
            .to_owned();
        Ok(CodexChatProc { server, inbound, thread_id, effort })
    }

    async fn turn(
        &self,
        proc: &mut CodexChatProc,
        text: &str,
        em: &Emitter<CodingAgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<TurnStatus, ProviderError> {
        if !proc.server.is_alive() {
            return Ok(TurnStatus::Died { output: false });
        }
        let params = codex_app::turn_params(&proc.thread_id, text, proc.effort.as_deref());
        if let Err(e) = proc.server.request("turn/start", params).await {
            return if proc.server.is_alive() { Err(e) } else { Ok(TurnStatus::Died { output: false }) };
        }
        let mut parser = map::TurnParser::default();
        let mut output = false;
        loop {
            let next = tokio::select! {
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                n = tokio::time::timeout(cli::CLI_IDLE_TIMEOUT, proc.inbound.recv()) => n.map_err(|_| {
                    ProviderError::Server(format!("Codex produced no output for {}s", cli::CLI_IDLE_TIMEOUT.as_secs()))
                })?,
            };
            match next {
                None | Some(Inbound::Closed) => return Ok(TurnStatus::Died { output }),
                Some(Inbound::Request { id, .. }) => {
                    // A read-only chat thread never asks; refuse anything.
                    proc.server.respond(&id, map::approval_answer(false, false, false));
                }
                Some(Inbound::Notification { method, params }) => {
                    if params.get("threadId").and_then(Value::as_str).is_some_and(|t| t != proc.thread_id) {
                        continue;
                    }
                    for ev in parser.notification(&method, &params) {
                        match &ev {
                            CodingAgentEvent::Chat(ChatEvent::Done { .. }) => {
                                em.emit(ev).await?;
                                return Ok(TurnStatus::Answered);
                            }
                            CodingAgentEvent::Chat(ChatEvent::Error(_)) => {
                                em.emit(ev).await?;
                                return Ok(TurnStatus::Failed);
                            }
                            CodingAgentEvent::Chat(ChatEvent::TextDelta(_)) => {
                                output = true;
                                em.emit(ev).await?;
                            }
                            CodingAgentEvent::Chat(_) => em.emit(ev).await?,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// Codex adapter (ADR-0047): agents run on `codex app-server` with
/// per-command approvals bridged to Orkestr; chat calls use a persistent
/// process pool.
pub struct CodexAdapter {
    id: ProviderId,
    name: String,
    binary_override: Option<String>,
    spawner: Arc<dyn ProcSpawner>,
    chats: Arc<ChatPool<CodexChatBackend>>,
}

impl std::fmt::Debug for CodexAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAdapter").field("id", &self.id).field("name", &self.name).finish_non_exhaustive()
    }
}

impl CodexAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(id: ProviderId, name: impl Into<String>, binary_override: Option<String>) -> Self {
        Self::with_spawner(id, name, binary_override, Arc::new(CliSpawner))
    }

    /// Creates the adapter with a process spawner (tests use a fake CLI).
    #[must_use]
    pub fn with_spawner(
        id: ProviderId,
        name: impl Into<String>,
        binary_override: Option<String>,
        spawner: Arc<dyn ProcSpawner>,
    ) -> Self {
        let chats = Arc::new(ChatPool::with_backend(CodexChatBackend::new(Arc::clone(&spawner))));
        Self { id, name: name.into(), binary_override, spawner, chats }
    }

    fn binary(&self) -> Option<PathBuf> {
        resolve_codex(self.binary_override.as_deref())
    }

    /// Argument vector of a one-shot `codex exec --json` run (pure; tested).
    /// Orkestr runs agents on the app-server; this is kept for diagnostics.
    #[must_use]
    pub fn exec_args(cwd: &str, model: Option<&str>, allow_writes: bool, skip_git_check: bool) -> Vec<String> {
        let mut args: Vec<String> =
            vec!["exec".into(), "--json".into(), "--cd".into(), cwd.to_owned(), "--sandbox".into()];
        args.push(if allow_writes { "workspace-write" } else { "read-only" }.into());
        if skip_git_check {
            args.push("--skip-git-repo-check".into());
        }
        if let Some(m) = model.filter(|m| !m.trim().is_empty() && *m != "default") {
            args.push("--model".into());
            args.push(m.to_owned());
        }
        args.push("-".into());
        args
    }

    /// The app-server run of a coding-agent request (pure; tested).
    #[must_use]
    pub fn agent_run(req: &CodingAgentRequest, cwd: PathBuf) -> codex_app::AgentRun {
        let bridged = req.permission_bridge.is_some();
        codex_app::AgentRun {
            cwd,
            model: req.model.clone(),
            instructions: req.system_append.clone(),
            prompt: req.prompt.clone(),
            resume: req.resume_session.clone(),
            sandbox: if req.tool_policy.allow_writes { "workspace-write" } else { "read-only" },
            // Bridged: Codex asks before every command / edit it does not
            // consider trivially safe, and Orkestr decides. Otherwise the
            // sandbox is the only gate (ADR-0012).
            approval_policy: if bridged { "untrusted" } else { "never" },
        }
    }
}

/// Resolves the Codex binary: a pinned path, else `PATH`, else Orkestr's own
/// installation (`<ORKESTR_HOME>/tools/codex`).
#[must_use]
pub fn resolve_codex(override_path: Option<&str>) -> Option<PathBuf> {
    if override_path.is_some_and(|p| !p.trim().is_empty()) {
        return cli::resolve_binary("codex", override_path);
    }
    cli::resolve_binary("codex", None).or_else(orkestr_tools_binary)
}

/// `<ORKESTR_HOME>/tools/codex/node_modules/.bin/codex(.cmd)` when it exists.
#[must_use]
pub fn orkestr_tools_binary() -> Option<PathBuf> {
    let home = std::env::var_os("ORKESTR_HOME").map(PathBuf::from)?;
    tools_binary(&home.join("tools").join("codex"))
}

/// The CLI inside an npm `--prefix` folder.
#[must_use]
pub fn tools_binary(prefix: &Path) -> Option<PathBuf> {
    let bin = prefix.join("node_modules").join(".bin");
    let names: &[&str] = if cfg!(windows) { &["codex.cmd", "codex.exe", "codex"] } else { &["codex"] };
    names.iter().map(|n| bin.join(n)).find(|p| p.is_file())
}

#[async_trait]
impl ProviderAdapter for CodexAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Codex,
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
            max_context_tokens: 272_000,
            coding_agent: true,
            cost_model: CostModelKind::Subscription,
            max_concurrency: 3,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        let Some(bin) = self.binary() else { return AuthStatus::NotInstalled { binary: "codex".into() } };
        // `codex login status` is the CLI's documented, token-free status command.
        match cli::probe(&bin, &["login", "status"]).await {
            Ok((true, _)) => AuthStatus::Ready { label: SESSION_LABEL.into() },
            Ok((false, _)) => {
                AuthStatus::Invalid { reason: "not logged in — sign in with ChatGPT from the Codex panel".into() }
            }
            Err(_) => AuthStatus::Unverified { label: SESSION_LABEL.into() },
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        let mut out = default_models(ProviderKind::Codex, self.id, false);
        let Some(bin) = self.binary() else { return Ok(out) };
        let live = async {
            let (srv, _rx) = AppServer::start(&*self.spawner, &bin, &std::env::temp_dir()).await?;
            srv.request("model/list", serde_json::json!({})).await
        };
        if let Ok(Ok(v)) = tokio::time::timeout(std::time::Duration::from_secs(20), live).await {
            let template = out.first().cloned();
            for m in map::models(&v) {
                if out.iter().any(|x| x.id == m.id) {
                    continue;
                }
                if let Some(t) = template.clone() {
                    out.push(Model { id: m.id.clone(), display_name: format!("Codex · {}", m.display_name), ..t });
                }
            }
        }
        Ok(out)
    }

    async fn health(&self) -> HealthReport {
        let Some(bin) = self.binary() else { return HealthReport::down("not installed (`codex` not found)") };
        let start = std::time::Instant::now();
        match cli::probe(&bin, &["--version"]).await {
            Ok((true, text)) => {
                let version = cli::parse_version(&text);
                HealthReport {
                    health: ProviderHealth::Healthy,
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                    detail: Some(format!("Codex {} · {SESSION_LABEL}", version.clone().unwrap_or_else(|| "?".into()))),
                    version,
                }
            }
            Ok((false, text)) => {
                HealthReport::down(format!("`codex --version` failed: {}", text.chars().take(200).collect::<String>()))
            }
            Err(e) => HealthReport::from_error(&e, None),
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        // Orkestr's own tools (Manager, reviews) are emulated exactly as for
        // Claude Code: the catalogue goes into the prompt and a
        // `{"tool_calls": […]}` answer becomes tool calls.
        let tools = !request.tools.is_empty() && request.tool_choice != ToolChoice::None;
        let protocol = if tools { claude_code::tool_protocol(&request) } else { String::new() };
        let bin = self.binary().ok_or_else(|| ProviderError::Auth("Codex CLI (`codex`) is not installed".into()))?;
        let first = cli::render_prompt(&request);
        let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        let stream = self.chats.turn(bin, cli::chat_scratch_dir()?, request, first, protocol, cancel.clone());
        if !tools {
            return Ok(cli::chat_only(stream));
        }
        Ok(claude_code::emulate_tool_calls(stream, names, cancel))
    }

    async fn run_coding_agent(
        &self,
        request: CodingAgentRequest,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        let cwd = cli::validate_cwd(&request.cwd)?;
        let bin = self.binary().ok_or_else(|| ProviderError::Auth("Codex CLI (`codex`) is not installed".into()))?;
        let replies = match &request.permission_bridge {
            Some(b) => Some(
                b.take().ok_or_else(|| ProviderError::BadRequest("the permission bridge was already used".into()))?,
            ),
            None => None,
        };
        let run = Self::agent_run(&request, cwd);
        Ok(codex_app::run_agent(Arc::clone(&self.spawner), bin, run, replies, cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded `codex exec --json` transcript (codex-cli 0.4x event schema).
    const FIXTURE: &str = r#"{"type":"thread.started","thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Planning** secret chain of thought"}}
{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"README.md\nsrc\n","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"src/new.rs","kind":"add"},{"path":"README.md","kind":"update"}],"status":"completed"}}
{"type":"item.started","item":{"id":"item_3","type":"command_execution","command":"cargo test","status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"cargo test","aggregated_output":"error","exit_code":101,"status":"failed"}}
{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"Added src/new.rs."}}
{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122}}"#;

    fn parse(fixture: &str, exit: Option<i32>, stderr: &str) -> Vec<CodingAgentEvent> {
        let mut p = CodexStreamParser::default();
        let mut out: Vec<CodingAgentEvent> = fixture.lines().flat_map(|l| p.line(l)).collect();
        if !out.iter().any(|e| matches!(e, CodingAgentEvent::Chat(c) if c.is_terminal())) {
            out.push(p.finish(exit, stderr));
        }
        out
    }

    #[test]
    fn parses_recorded_transcript() {
        let ev = parse(FIXTURE, Some(0), "");
        assert_eq!(ev[0], CodingAgentEvent::Session { session_id: "0199a213-81c0-7800-8aa1-bbab2a035a53".into() });
        assert!(
            matches!(&ev[1], CodingAgentEvent::ToolStarted { call_id, tool, summary } if call_id == "item_1" && tool == "shell" && summary == "bash -lc ls")
        );
        assert!(matches!(&ev[2], CodingAgentEvent::ToolFinished { ok: true, .. }));
        // file_change completed without a started event: both are emitted.
        assert!(matches!(&ev[3], CodingAgentEvent::ToolStarted { tool, .. } if tool == "apply_patch"));
        assert!(matches!(&ev[4], CodingAgentEvent::ToolFinished { ok: true, .. }));
        assert_eq!(ev[5], CodingAgentEvent::FileChanged { path: "src/new.rs".into(), change: FileChangeKind::Added });
        assert_eq!(ev[6], CodingAgentEvent::FileChanged { path: "README.md".into(), change: FileChangeKind::Modified });
        assert!(matches!(&ev[8], CodingAgentEvent::ToolFinished { ok: false, .. }));
        assert_eq!(ev[9], CodingAgentEvent::Chat(ChatEvent::TextDelta("Added src/new.rs.".into())));
        assert!(matches!(
            &ev[10],
            CodingAgentEvent::Chat(ChatEvent::Usage(Usage {
                input_tokens: 24763,
                cached_input_tokens: 24448,
                output_tokens: 122,
                ..
            }))
        ));
        assert_eq!(ev[11], CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn }));
        assert_eq!(ev.len(), 12);
        assert!(!format!("{ev:?}").contains("secret chain"), "reasoning must be dropped");
    }

    #[test]
    fn failures_are_classified() {
        let failed =
            r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Try again in 3 hours."}}"#;
        assert!(matches!(
            parse(failed, Some(1), "").last(),
            Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::QuotaExceeded(_))))
        ));
        let not_logged = parse("", Some(1), "Error: Not logged in. Run codex login");
        assert!(matches!(not_logged.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(_))))));
        let crash = parse(r#"{"type":"error","message":"stream disconnected"}"#, Some(2), "");
        assert!(
            matches!(crash.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Server(m)))) if m.contains("stream disconnected"))
        );
    }

    #[test]
    fn agent_runs_map_policy_to_sandbox_and_approvals() {
        use crate::types::{CliToolPolicy, PermissionBridge};
        let (bridge, _tx) = PermissionBridge::new();
        let mut req = CodingAgentRequest {
            prompt: "p".into(),
            system_append: None,
            cwd: ".".into(),
            model: Some("gpt-5-codex".into()),
            max_turns: None,
            resume_session: None,
            tool_policy: CliToolPolicy { allow_writes: true, ..CliToolPolicy::default() },
            subagent: None,
            permission_bridge: Some(bridge),
        };
        let r = CodexAdapter::agent_run(&req, PathBuf::from("."));
        assert_eq!((r.sandbox, r.approval_policy), ("workspace-write", "untrusted"));
        req.permission_bridge = None;
        req.tool_policy.allow_writes = false;
        let r = CodexAdapter::agent_run(&req, PathBuf::from("."));
        assert_eq!((r.sandbox, r.approval_policy), ("read-only", "never"));
    }

    #[tokio::test]
    async fn manager_chat_keeps_one_codex_process_per_conversation() {
        use crate::codex_app::conn::fake::FakeCodex;
        use crate::types::ChatMessage;
        use futures::StreamExt;
        use serde_json::json;
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let t2 = Arc::clone(&turns);
        let fake = FakeCodex::new(Arc::new(move |method, _p, id| match method {
            "thread/start" => vec![json!({"id": id, "result": {"thread": {"id": "th"}}})],
            "turn/start" => {
                let n = t2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                vec![
                    json!({"id": id, "result": {"turn": {"id": "u", "items": [], "status": "inProgress"}}}),
                    json!({"method":"item/agentMessage/delta","params":{"threadId":"th","turnId":"u","itemId":"m","delta": format!("reply {n}")}}),
                    json!({"method":"turn/completed","params":{"threadId":"th","turn":{"id":"u","items":[],"status":"completed"}}}),
                ]
            }
            _ => vec![],
        }));
        let adapter = CodexAdapter::with_spawner(
            ProviderId::new(),
            "Codex",
            Some(std::env::current_exe().unwrap().to_string_lossy().into_owned()),
            Arc::clone(&fake) as Arc<dyn ProcSpawner>,
        );
        let ask = |msgs: Vec<ChatMessage>| {
            let adapter = &adapter;
            async move {
                let s = adapter.chat(ChatRequest::new("default", msgs), CancellationToken::new()).await.unwrap();
                s.filter_map(|e| async move {
                    match e {
                        ChatEvent::TextDelta(t) => Some(t),
                        _ => None,
                    }
                })
                .collect::<String>()
                .await
            }
        };
        let mut convo = vec![ChatMessage::user("plan")];
        assert_eq!(ask(convo.clone()).await, "reply 1");
        convo.push(ChatMessage::assistant("reply 1"));
        convo.push(ChatMessage::user("next"));
        assert_eq!(ask(convo).await, "reply 2");
        assert_eq!(fake.spawned.load(std::sync::atomic::Ordering::SeqCst), 1, "one process for the conversation");
        let seen = fake.seen.lock().clone();
        let start = seen.iter().find(|v| v["method"] == "thread/start").unwrap();
        assert_eq!(
            (start["params"]["sandbox"].as_str(), start["params"]["approvalPolicy"].as_str()),
            (Some("read-only"), Some("never"))
        );
        let last_turn = seen.iter().rev().find(|v| v["method"] == "turn/start").unwrap();
        assert_eq!(last_turn["params"]["input"][0]["text"], "next", "only the new message");
    }

    #[test]
    fn exec_args_never_use_full_access() {
        let a = CodexAdapter::exec_args("/wt", Some("gpt-5-codex"), true, false);
        assert_eq!(a.join(" "), "exec --json --cd /wt --sandbox workspace-write --model gpt-5-codex -");
        let a = CodexAdapter::exec_args("/tmp", Some("default"), false, true);
        assert_eq!(a.join(" "), "exec --json --cd /tmp --sandbox read-only --skip-git-repo-check -");
        assert!(!a.iter().any(|x| x.contains("danger")));
    }
}
