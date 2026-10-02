//! OpenAI Codex — the official `codex` CLI (`codex exec --json`), using the
//! user's own login (`codex login`). Orkestr never reads `~/.codex/auth.json`.
//!
//! Gating is launch-time only (ADR-0012): Codex runs inside its own sandbox
//! (`read-only` or `workspace-write`); `danger-full-access` is never used.

use std::collections::HashSet;
use std::path::PathBuf;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::cli::{self, CliStreamParser};
use crate::adapter::ProviderAdapter;
use crate::catalog::default_models;
use crate::types::{
    AuthStatus, ChatEvent, ChatRequest, ChatStream, CodingAgentEvent, CodingAgentRequest, CodingAgentStream,
    CostModelKind, FileChangeKind, HealthReport, Locality, ProviderDescriptor, ProviderError, StopReason, Usage,
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

/// Codex adapter.
#[derive(Debug)]
pub struct CodexAdapter {
    id: ProviderId,
    name: String,
    binary_override: Option<String>,
}

impl CodexAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(id: ProviderId, name: impl Into<String>, binary_override: Option<String>) -> Self {
        Self { id, name: name.into(), binary_override }
    }

    fn binary(&self) -> Option<PathBuf> {
        cli::resolve_binary("codex", self.binary_override.as_deref())
    }

    /// Argument vector for a run (pure; tested). The prompt goes over stdin (`-`).
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

    fn spawn(
        &self,
        args: Vec<String>,
        cwd: PathBuf,
        prompt: String,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        let bin = self.binary().ok_or_else(|| ProviderError::Auth("Codex CLI (`codex`) is not installed".into()))?;
        Ok(cli::run_cli(bin, args, cwd, prompt, CodexStreamParser::default(), cancel))
    }
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
            max_context_tokens: 200_000,
            coding_agent: true,
            cost_model: CostModelKind::Subscription,
            max_concurrency: 2,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        let Some(bin) = self.binary() else { return AuthStatus::NotInstalled { binary: "codex".into() } };
        // `codex login status` is the CLI's documented, token-free status command.
        match cli::probe(&bin, &["login", "status"]).await {
            Ok((true, _)) => AuthStatus::Ready { label: SESSION_LABEL.into() },
            Ok((false, _)) => {
                AuthStatus::Invalid { reason: "not logged in — run `codex login` in a terminal".into() }
            }
            Err(_) => AuthStatus::Unverified { label: SESSION_LABEL.into() },
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(default_models(ProviderKind::Codex, self.id, false))
    }

    async fn health(&self) -> HealthReport {
        let Some(bin) = self.binary() else { return HealthReport::down("not installed (`codex` not found on PATH)") };
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
        if !request.tools.is_empty() {
            return Err(ProviderError::Unsupported(
                "Codex runs its own tools; use run_coding_agent for tool loops".into(),
            ));
        }
        let dir = cli::chat_scratch_dir()?;
        let args = Self::exec_args(&dir.to_string_lossy(), Some(request.model.as_str()), false, true);
        let stream = self.spawn(args, dir, cli::render_prompt(&request), cancel)?;
        Ok(cli::chat_only(stream))
    }

    async fn run_coding_agent(
        &self,
        request: CodingAgentRequest,
        cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        if request.resume_session.is_some() {
            return Err(ProviderError::Unsupported("resuming Codex sessions is not supported yet".into()));
        }
        let cwd = cli::validate_cwd(&request.cwd)?;
        let args = Self::exec_args(&request.cwd, request.model.as_deref(), request.tool_policy.allow_writes, false);
        let mut prompt = String::new();
        if let Some(sys) = request.system_append.as_deref().filter(|s| !s.trim().is_empty()) {
            prompt.push_str("Additional instructions:\n");
            prompt.push_str(sys);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&request.prompt);
        self.spawn(args, cwd, prompt, cancel)
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
    fn exec_args_never_use_full_access() {
        let a = CodexAdapter::exec_args("/wt", Some("gpt-5-codex"), true, false);
        assert_eq!(a.join(" "), "exec --json --cd /wt --sandbox workspace-write --model gpt-5-codex -");
        let a = CodexAdapter::exec_args("/tmp", Some("default"), false, true);
        assert_eq!(a.join(" "), "exec --json --cd /tmp --sandbox read-only --skip-git-repo-check -");
        assert!(!a.iter().any(|x| x.contains("danger")));
    }
}
