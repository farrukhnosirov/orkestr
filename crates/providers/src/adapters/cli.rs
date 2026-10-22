//! Shared machinery for official CLI coding agents (Claude Code, Codex).
//!
//! Rules (ADR-0012): official binaries only, spawned **without a shell** with
//! an explicit argument vector, minimal allow-listed environment, prompt over
//! stdin, stdout parsed line by line. Orkestr never reads the CLIs' credential
//! files. Cancellation kills the whole process tree.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use orkestr_platform::{NativeProcessControl, ProcessControl};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::http::{drive, Emitter};
use crate::types::{
    ChatEvent, ChatMessage, ChatRequest, ChatStream, CodingAgentEvent, CodingAgentStream, ProviderError,
    ResponseFormat, Role,
};

/// Environment variables passed through to CLI agents. Everything else
/// (project secrets, provider API keys of Orkestr) is withheld.
pub const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "USER",
    "LOGNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "COMMONPROGRAMFILES",
    "TEMP",
    "TMP",
    "TMPDIR",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "OS",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "SHELL",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
    // The CLIs' own documented config locations.
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
];

/// Idle limit for CLI output (spec §5.4).
pub const CLI_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Timeout for `--version` / status probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_TAIL: usize = 4096;

/// Resolves a CLI binary: explicit absolute path from settings, else PATH.
#[must_use]
pub fn resolve_binary(name: &str, override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = override_path.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(p);
        return (path.is_absolute() && path.is_file()).then_some(path);
    }
    orkestr_platform::process::which(name)
}

pub(crate) fn command(program: &Path, args: &[String]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        if let Some(key) = k.to_str() {
            if ENV_ALLOWLIST.iter().any(|a| a.eq_ignore_ascii_case(key)) {
                cmd.env(&k, v);
            }
        }
    }
    cmd.kill_on_drop(true);
    cmd
}

/// Runs a short probe (`--version`, `login status`) and returns
/// `(success, stdout+stderr)`.
pub async fn probe(program: &Path, args: &[&str]) -> Result<(bool, String), ProviderError> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let mut cmd = command(program, &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            ProviderError::Network(format!(
                "`{}` did not answer within {}s",
                program.display(),
                PROBE_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| ProviderError::Server(format!("failed to run `{}`: {e}", program.display())))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text.trim().to_owned()))
}

/// Extracts a `x.y.z` version from `--version` output.
#[must_use]
pub fn parse_version(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == 'v')
        .find(|tok| {
            let parts: Vec<&str> = tok.split('.').collect();
            parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_owned)
}

/// Parses CLI JSON lines into events.
pub trait CliStreamParser: Send + 'static {
    /// One stdout line.
    fn line(&mut self, line: &str) -> Vec<CodingAgentEvent>;
    /// Process exited; returns the terminal event if none was emitted yet.
    fn finish(&mut self, exit_code: Option<i32>, stderr_tail: &str) -> CodingAgentEvent;
}

/// Kills the process tree when dropped (cancellation / consumer gone).
pub(crate) struct TreeGuard(pub(crate) Option<u32>);

impl Drop for TreeGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            // taskkill / kill are short-lived; run off the async thread.
            std::thread::spawn(move || {
                if let Err(e) = NativeProcessControl.kill_tree(pid) {
                    tracing::debug!(pid, error = %e, "kill_tree after CLI cancel");
                }
            });
        }
    }
}

/// Spawns a CLI run and streams parsed events.
pub fn run_cli<P: CliStreamParser>(
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    stdin: String,
    mut parser: P,
    cancel: CancellationToken,
) -> CodingAgentStream {
    drive(cancel, move |em: Emitter<CodingAgentEvent>| async move {
        let mut cmd = command(&program, &args);
        cmd.current_dir(&cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProviderError::Auth(format!("`{}` not found — install the official CLI", program.display()))
            } else {
                ProviderError::Server(format!("failed to start `{}`: {e}", program.display()))
            }
        })?;
        let mut guard = TreeGuard(child.id());
        tracing::info!(program = %program.display(), cwd = %cwd.display(), "CLI agent started");
        if let Some(mut si) = child.stdin.take() {
            si.write_all(stdin.as_bytes())
                .await
                .map_err(|e| ProviderError::Server(format!("writing prompt to CLI: {e}")))?;
            drop(si);
        }
        let stderr_task = child.stderr.take().map(|mut se| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while let Ok(n) = se.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > STDERR_TAIL * 2 {
                        buf.drain(..buf.len() - STDERR_TAIL);
                    }
                }
                let start = buf.len().saturating_sub(STDERR_TAIL);
                String::from_utf8_lossy(&buf[start..]).into_owned()
            })
        });
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Server("CLI stdout unavailable".into()))?;
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let next = tokio::time::timeout(CLI_IDLE_TIMEOUT, lines.next_line())
                .await
                .map_err(|_| {
                    ProviderError::Server(format!("CLI produced no output for {}s", CLI_IDLE_TIMEOUT.as_secs()))
                })?
                .map_err(|e| ProviderError::Server(format!("reading CLI output: {e}")))?;
            let Some(line) = next else { break };
            for ev in parser.line(&line) {
                em.emit(ev).await?;
            }
        }
        let status = child.wait().await.map_err(|e| ProviderError::Server(format!("waiting for CLI: {e}")))?;
        guard.0 = None; // exited normally: nothing to kill
        let stderr_tail = match stderr_task {
            Some(t) => t.await.unwrap_or_default(),
            None => String::new(),
        };
        let terminal = parser.finish(status.code(), &stderr_tail);
        em.emit(terminal).await?;
        Ok(())
    })
}

/// Coding-agent stream → chat stream (text/usage/terminal only).
pub fn chat_only(stream: CodingAgentStream) -> ChatStream {
    stream
        .filter_map(|ev| async move {
            if let CodingAgentEvent::Chat(c) = ev {
                Some(c)
            } else {
                None
            }
        })
        .boxed()
}

/// Renders a chat request into a single CLI prompt (CLI chat style).
#[must_use]
pub fn render_prompt(req: &ChatRequest) -> String {
    let convo: Vec<&ChatMessage> = req.conversation().collect();
    let mut out = String::new();
    if let Some(sys) = req.system_prompt() {
        out.push_str("Instructions:\n");
        out.push_str(&sys);
        out.push_str("\n\n");
    }
    if convo.len() == 1 && convo[0].role == Role::User {
        out.push_str(&convo[0].content);
    } else {
        out.push_str("Conversation so far:\n");
        for m in convo {
            let who = match m.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool result",
                Role::System => "System",
            };
            match (&m.role, m.tool_call_id.as_deref()) {
                (Role::Tool, Some(id)) => {
                    let err = if m.is_error { " (error)" } else { "" };
                    out.push_str(&format!("\nTool result for call {id}{err}: {}\n", m.content));
                }
                _ => out.push_str(&format!("\n{who}: {}\n", m.content)),
            }
            for c in &m.tool_calls {
                out.push_str(&format!("[Assistant called tool `{}` (call {}) with {}]\n", c.name, c.id, c.arguments));
            }
        }
        out.push_str("\nReply as the Assistant to the last User message.");
    }
    match &req.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject => out.push_str("\n\nRespond with a single JSON object only, no prose."),
        ResponseFormat::JsonSchema { schema, .. } => {
            out.push_str(&format!("\n\nRespond with a single JSON document only (no prose, no code fences) matching this JSON schema:\n{schema}"));
        }
    }
    out
}

/// A scratch directory for chat-style CLI runs (no project files visible).
pub fn chat_scratch_dir() -> Result<PathBuf, ProviderError> {
    let dir = std::env::temp_dir().join("orkestr-cli-chat");
    std::fs::create_dir_all(&dir).map_err(|e| ProviderError::Server(format!("creating CLI scratch dir: {e}")))?;
    Ok(dir)
}

/// Validates a coding-agent working directory.
pub fn validate_cwd(cwd: &str) -> Result<PathBuf, ProviderError> {
    let p = PathBuf::from(cwd);
    if !p.is_absolute() || !p.is_dir() {
        return Err(ProviderError::BadRequest(format!(
            "coding-agent cwd must be an existing absolute directory: {cwd}"
        )));
    }
    Ok(p)
}

/// True when `lower` reports HTTP status `code` in an explicit context
/// ("API Error: 401", "HTTP 429", "status 500", or a message starting with the
/// code). A bare number elsewhere in the text is not a status.
fn has_http_status(lower: &str, code: u16) -> bool {
    let c = code.to_string();
    lower.trim_start().starts_with(&format!("{c} "))
        || ["error: ", "error ", "http ", "http/1.1 ", "http/2 ", "status ", "status: ", "status code "]
            .iter()
            .any(|p| lower.contains(&format!("{p}{c}")))
}

/// Classifies a CLI error / result message (spec §5.3). `None` = unknown.
#[must_use]
pub fn classify_cli_message(text: &str) -> Option<ProviderError> {
    let lower = text.to_ascii_lowercase();
    let msg = || text.trim().chars().take(300).collect::<String>();
    if [
        "usage limit",
        "limit reached",
        "limit will reset",
        "out of extra usage",
        "credit balance is too low",
        "insufficient_quota",
        "quota",
    ]
    .iter()
    .any(|m| lower.contains(m))
    {
        return Some(ProviderError::QuotaExceeded(msg()));
    }
    if lower.contains("rate limit")
        || lower.contains("rate_limit")
        || has_http_status(&lower, 429)
        || lower.contains("too many requests")
    {
        return Some(ProviderError::RateLimited { retry_after_secs: None });
    }
    if [
        "/login",
        "not logged in",
        "invalid api key",
        "authentication",
        "unauthorized",
        "please log in",
        "login required",
        "codex login",
    ]
    .iter()
    .any(|m| lower.contains(m))
        || has_http_status(&lower, 401)
    {
        return Some(ProviderError::Auth(msg()));
    }
    if lower.contains("overloaded")
        || has_http_status(&lower, 529)
        || has_http_status(&lower, 500)
        || lower.contains("internal server error")
    {
        return Some(ProviderError::Server(msg()));
    }
    None
}

/// Short single-line summary of a tool input.
#[must_use]
pub fn summarize(value: &serde_json::Value) -> String {
    let s = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(m) => ["command", "file_path", "path", "pattern", "url", "query", "description"]
            .iter()
            .find_map(|k| m.get(*k).and_then(|v| v.as_str()).map(str::to_owned))
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    };
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 160 {
        format!("{}…", one_line.chars().take(160).collect::<String>())
    } else {
        one_line
    }
}

/// Terminal chat error event.
#[must_use]
pub fn error_event(err: ProviderError) -> CodingAgentEvent {
    CodingAgentEvent::Chat(ChatEvent::Error(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.0.83 (Claude Code)").as_deref(), Some("1.0.83"));
        assert_eq!(parse_version("codex-cli 0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(parse_version("v2.1").as_deref(), Some("2.1"));
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn cli_message_classification() {
        assert!(matches!(
            classify_cli_message("Claude AI usage limit reached|1760000000"),
            Some(ProviderError::QuotaExceeded(_))
        ));
        assert!(matches!(classify_cli_message("Invalid API key · Please run /login"), Some(ProviderError::Auth(_))));
        assert!(matches!(classify_cli_message("429 Too Many Requests"), Some(ProviderError::RateLimited { .. })));
        assert!(matches!(classify_cli_message("API Error: 529 overloaded"), Some(ProviderError::Server(_))));
        assert_eq!(classify_cli_message("something odd"), None);
        assert!(matches!(classify_cli_message("API Error: 401 bad token"), Some(ProviderError::Auth(_))));
        assert!(matches!(classify_cli_message("HTTP 429 slow down"), Some(ProviderError::RateLimited { .. })));
        // Bare numbers are not HTTP statuses.
        assert_eq!(classify_cli_message("processed 401 files, 500 lines, 4290 bytes"), None);
    }

    #[test]
    fn prompt_rendering() {
        let mut r = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        assert_eq!(render_prompt(&r), "hi");
        r.system = Some("be brief".into());
        r.messages.push(ChatMessage::assistant("hello"));
        r.messages.push(ChatMessage::user("more"));
        r.response_format = ResponseFormat::JsonObject;
        let p = render_prompt(&r);
        assert!(p.starts_with("Instructions:\nbe brief"));
        assert!(p.contains("User: more"));
        assert!(p.ends_with("no prose."));
    }

    #[test]
    fn summaries_are_short() {
        assert_eq!(summarize(&serde_json::json!({"command": "ls   -la\nfoo"})), "ls -la foo");
        assert!(summarize(&serde_json::json!("x".repeat(500))).chars().count() <= 161);
    }

    #[test]
    fn env_allowlist_excludes_keys() {
        assert!(!ENV_ALLOWLIST.iter().any(|k| k.contains("API_KEY")));
    }
}
