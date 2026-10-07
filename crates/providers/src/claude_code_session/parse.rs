//! Parser for the Claude Code CLI's `stream-json` output (and its
//! per-conversation transcript files, which use the same message shapes).
//!
//! Pure: one line in, zero or more [`Inbound`] items out. Verified against
//! recordings of the real binary (`tests/fixtures/claude_code/*.jsonl`,
//! Claude Code 2.1.292). Hidden reasoning (`thinking` blocks / deltas) is
//! never surfaced — only a [`ClaudeCodeEvent::Thinking`] marker.

use std::collections::HashMap;

use orkestr_protocol::methods::claude_code::{ClaudeAuthInfo, ClaudeCodeEvent, ClaudeModelOption, ClaudeSlashCommand, ClaudeUsage};
use serde_json::Value;

/// Longest tool output kept per result (characters).
pub const MAX_TOOL_OUTPUT_CHARS: usize = 20_000;

/// One parsed item of the CLI's output.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// A chat item for the UI.
    Event(ClaudeCodeEvent),
    /// The CLI asks the host something (`can_use_tool`, hook callbacks, …).
    ControlRequest {
        /// Id to answer with.
        request_id: String,
        /// The request object (`subtype` + fields).
        request: Value,
    },
    /// Answer to one of Orkestr's control requests.
    ControlResponse {
        /// Id of Orkestr's request.
        request_id: String,
        /// `Ok(response)` or `Err(error message)`.
        result: Result<Value, String>,
    },
    /// The CLI withdrew one of its control requests (e.g. after an interrupt).
    ControlCancel {
        /// Id of the withdrawn request.
        request_id: String,
    },
}

/// Tools whose successful result means a file was written.
const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// Stateful line parser.
#[derive(Debug, Default)]
pub struct StreamParser {
    /// Transcript mode: user text blocks are user messages (in the live
    /// stream Orkestr echoes its own messages instead).
    transcript: bool,
    /// tool_use id → (tool name, written file path).
    tools: HashMap<String, (String, Option<String>)>,
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn parent(v: &Value) -> Option<String> {
    v.get("parent_tool_use_id").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned)
}

/// Truncates text to `max` characters (keeps the head, marks the cut).
#[must_use]
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n…");
    out
}

/// Text of a `tool_result` content (string or array of blocks).
fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Whether CLI-written text means the user must sign in.
#[must_use]
pub fn is_auth_message(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("not logged in")
        || l.contains("please run /login")
        || l.contains("invalid api key")
        || l.contains("oauth token has expired")
        || l.contains("authentication_error")
}

/// Strips `<tag>…</tag>` wrappers the CLI puts around local command output.
fn unwrap_local_command(text: &str) -> Option<String> {
    for tag in ["local-command-stdout", "local-command-stderr"] {
        let open = format!("<{tag}>");
        if let Some(start) = text.find(&open) {
            let rest = &text[start + open.len()..];
            let end = rest.find(&format!("</{tag}>")).unwrap_or(rest.len());
            return Some(rest[..end].trim().to_owned());
        }
    }
    None
}

/// `/name args` from a transcript's `<command-name>` wrapper.
fn unwrap_command_name(text: &str) -> Option<String> {
    let start = text.find("<command-name>")? + "<command-name>".len();
    let end = text[start..].find("</command-name>")? + start;
    let name = text[start..end].trim();
    let args = text
        .find("<command-args>")
        .and_then(|a| {
            let a = a + "<command-args>".len();
            text[a..].find("</command-args>").map(|e| text[a..a + e].trim().to_owned())
        })
        .unwrap_or_default();
    let name = if name.starts_with('/') { name.to_owned() } else { format!("/{name}") };
    Some(if args.is_empty() { name } else { format!("{name} {args}") })
}

/// Account info from the `initialize` answer.
fn account(v: &Value) -> Option<ClaudeAuthInfo> {
    let a = v.get("account")?;
    let email = s(a, "email");
    let token_source = s(a, "tokenSource");
    let logged_in = email.is_some()
        || s(a, "subscriptionType").is_some()
        || s(a, "apiKeySource").is_some_and(|k| k != "none")
        || token_source.as_deref().is_some_and(|t| t != "none");
    Some(ClaudeAuthInfo {
        logged_in,
        auth_method: token_source.filter(|t| t != "none").or_else(|| s(a, "apiProvider").filter(|_| logged_in)),
        subscription_type: s(a, "subscriptionType"),
        email,
    })
}

/// Capabilities from the CLI's `initialize` control response.
#[must_use]
pub fn capabilities(response: &Value) -> ClaudeCodeEvent {
    let models = response
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ClaudeModelOption {
                value: s(m, "value")?,
                display_name: s(m, "displayName").or_else(|| s(m, "value"))?,
                description: s(m, "description"),
            })
        })
        .collect();
    let commands = response
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(ClaudeSlashCommand {
                name: s(c, "name")?,
                description: s(c, "description").unwrap_or_default(),
                argument_hint: s(c, "argumentHint").filter(|h| !h.is_empty()),
            })
        })
        .collect();
    ClaudeCodeEvent::Capabilities { models, commands, account: account(response) }
}

/// Parses `claude auth status` JSON output.
#[must_use]
pub fn auth_status(text: &str) -> Option<(ClaudeAuthInfo, Option<String>)> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let v: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let logged_in = v.get("loggedIn").and_then(Value::as_bool)?;
    let info = ClaudeAuthInfo {
        logged_in,
        auth_method: s(&v, "authMethod").filter(|m| m != "none"),
        subscription_type: s(&v, "subscriptionType"),
        email: s(&v, "email"),
    };
    Some((info, s(&v, "projectsDirectory")))
}

impl StreamParser {
    /// Parser for the live stream.
    #[must_use]
    pub fn live() -> Self {
        Self::default()
    }

    /// Parser for transcript files.
    #[must_use]
    pub fn transcript() -> Self {
        Self { transcript: true, ..Self::default() }
    }

    /// Parses one line.
    pub fn line(&mut self, line: &str) -> Vec<Inbound> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            tracing::debug!("ignoring a non-JSON line from claude");
            return Vec::new();
        };
        self.value(&v)
    }

    /// Parses one JSON message.
    pub fn value(&mut self, v: &Value) -> Vec<Inbound> {
        let mut out = Vec::new();
        let ev = |out: &mut Vec<Inbound>, e: ClaudeCodeEvent| out.push(Inbound::Event(e));
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "control_request" => {
                if let (Some(id), Some(req)) = (s(v, "request_id"), v.get("request")) {
                    out.push(Inbound::ControlRequest { request_id: id, request: req.clone() });
                }
            }
            "control_response" => {
                let r = v.get("response").cloned().unwrap_or(Value::Null);
                if let Some(id) = s(&r, "request_id") {
                    let result = if r.get("subtype").and_then(Value::as_str) == Some("error") {
                        Err(s(&r, "error").unwrap_or_else(|| "control request failed".into()))
                    } else {
                        Ok(r.get("response").cloned().unwrap_or(Value::Null))
                    };
                    out.push(Inbound::ControlResponse { request_id: id, result });
                }
            }
            "control_cancel_request" => {
                if let Some(id) = s(v, "request_id") {
                    out.push(Inbound::ControlCancel { request_id: id });
                }
            }
            "system" => match v.get("subtype").and_then(Value::as_str) {
                Some("init") => {
                    if let Some(id) = s(v, "session_id") {
                        ev(
                            &mut out,
                            ClaudeCodeEvent::Init {
                                claude_session_id: id,
                                model: s(v, "model"),
                                cwd: s(v, "cwd"),
                                tools: v
                                    .get("tools")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|t| t.as_str().map(str::to_owned))
                                    .collect(),
                                permission_mode: s(v, "permissionMode"),
                            },
                        );
                    }
                }
                Some("status") => {
                    if let Some(mode) = s(v, "permissionMode") {
                        ev(&mut out, ClaudeCodeEvent::ModeChanged { mode });
                    }
                }
                _ => {} // hooks, thinking token estimates, commands_changed, compact boundaries
            },
            "stream_event" => {
                let e = v.get("event").cloned().unwrap_or(Value::Null);
                match e.get("type").and_then(Value::as_str) {
                    Some("content_block_start") => {
                        let kind = e.pointer("/content_block/type").and_then(Value::as_str);
                        if matches!(kind, Some("thinking" | "redacted_thinking")) {
                            ev(&mut out, ClaudeCodeEvent::Thinking);
                        }
                    }
                    Some("content_block_delta") => {
                        if e.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") {
                            if let Some(t) = e.pointer("/delta/text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                ev(&mut out, ClaudeCodeEvent::TextDelta { text: t.to_owned(), parent_tool_use_id: parent(v) });
                            }
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => self.assistant(v, &mut out),
            "user" => self.user(v, &mut out),
            "result" => {
                let get = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                ev(
                    &mut out,
                    ClaudeCodeEvent::Result {
                        is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        subtype: s(v, "subtype").unwrap_or_else(|| "success".into()),
                        result: s(v, "result").map(|r| truncate_chars(&r, MAX_TOOL_OUTPUT_CHARS)),
                        duration_ms: v.get("duration_ms").and_then(Value::as_u64),
                        num_turns: v.get("num_turns").and_then(Value::as_u64).map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
                        total_cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                        usage: ClaudeUsage {
                            input_tokens: get("/usage/input_tokens"),
                            output_tokens: get("/usage/output_tokens"),
                            cache_read_tokens: get("/usage/cache_read_input_tokens"),
                            cache_write_tokens: get("/usage/cache_creation_input_tokens"),
                        },
                    },
                );
            }
            "rate_limit_event" => {
                let info = v.get("rate_limit_info").cloned().unwrap_or(Value::Null);
                if let Some(status) = s(&info, "status") {
                    let window = s(&info, "rateLimitType");
                    let utilization = window
                        .as_deref()
                        .and_then(|w| info.pointer(&format!("/unifiedWindows/{w}/utilization")))
                        .and_then(Value::as_f64);
                    ev(
                        &mut out,
                        ClaudeCodeEvent::RateLimit {
                            status,
                            resets_at: info.get("resetsAt").and_then(Value::as_i64),
                            window,
                            utilization,
                        },
                    );
                }
            }
            _ => {}
        }
        out
    }

    fn assistant(&mut self, v: &Value, out: &mut Vec<Inbound>) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let synthetic = v.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>");
        let auth_flag = v.get("error").and_then(Value::as_str).is_some_and(|e| e.contains("auth"));
        let parent_id = parent(v);
        for block in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let Some(text) = block.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) else {
                        continue;
                    };
                    if synthetic && (auth_flag || is_auth_message(text)) {
                        out.push(Inbound::Event(ClaudeCodeEvent::Error {
                            message: text.to_owned(),
                            auth_required: true,
                        }));
                    } else {
                        out.push(Inbound::Event(ClaudeCodeEvent::Text {
                            text: text.to_owned(),
                            parent_tool_use_id: parent_id.clone(),
                            synthetic,
                        }));
                    }
                }
                Some("tool_use") => {
                    let id = s(block, "id").unwrap_or_default();
                    let name = s(block, "name").unwrap_or_default();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let path = input
                        .get("file_path")
                        .or_else(|| input.get("notebook_path"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.tools.insert(id.clone(), (name.clone(), path));
                    out.push(Inbound::Event(ClaudeCodeEvent::ToolUse {
                        id,
                        name,
                        input,
                        parent_tool_use_id: parent_id.clone(),
                    }));
                }
                _ => {} // thinking / redacted_thinking: never surfaced
            }
        }
    }

    fn user(&mut self, v: &Value, out: &mut Vec<Inbound>) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("isMeta").and_then(Value::as_bool) == Some(true)
            || v.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        {
            return;
        }
        let parent_id = parent(v);
        match v.pointer("/message/content") {
            Some(Value::String(text)) => self.user_text(text, out),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_result") => {
                            let id = s(block, "tool_use_id").unwrap_or_default();
                            let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            let content = truncate_chars(&tool_result_text(block.get("content")), MAX_TOOL_OUTPUT_CHARS);
                            let written = self.tools.remove(&id).and_then(|(name, path)| {
                                (WRITE_TOOLS.contains(&name.as_str()) && !is_error).then_some(path).flatten()
                            });
                            out.push(Inbound::Event(ClaudeCodeEvent::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error,
                                parent_tool_use_id: parent_id.clone(),
                            }));
                            if let Some(path) = written {
                                out.push(Inbound::Event(ClaudeCodeEvent::FileChanged { path }));
                            }
                        }
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                self.user_text(t, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn user_text(&mut self, text: &str, out: &mut Vec<Inbound>) {
        let trimmed = text.trim();
        if trimmed.starts_with("[Request interrupted by user") {
            out.push(Inbound::Event(ClaudeCodeEvent::Interrupted));
            return;
        }
        if let Some(output) = unwrap_local_command(trimmed) {
            if !output.is_empty() {
                out.push(Inbound::Event(ClaudeCodeEvent::CommandOutput { text: output }));
            }
            return;
        }
        if !self.transcript {
            return; // Orkestr echoes its own messages when sending them
        }
        if let Some(cmd) = unwrap_command_name(trimmed) {
            out.push(Inbound::Event(ClaudeCodeEvent::UserMessage { text: cmd }));
            return;
        }
        if trimmed.starts_with("<command-message>") || trimmed.starts_with("<system-reminder>") || trimmed.is_empty() {
            return;
        }
        out.push(Inbound::Event(ClaudeCodeEvent::UserMessage { text: text.to_owned() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT: &str = include_str!("../../tests/fixtures/claude_code/initialize.jsonl");
    const WRITE: &str = include_str!("../../tests/fixtures/claude_code/turn_write_allow.jsonl");
    const DENY: &str = include_str!("../../tests/fixtures/claude_code/turn_deny_and_session_allow.jsonl");
    const INTERRUPT: &str = include_str!("../../tests/fixtures/claude_code/interrupt_mode_model.jsonl");
    const NO_LOGIN: &str = include_str!("../../tests/fixtures/claude_code/not_logged_in.jsonl");

    fn parse(fixture: &str) -> Vec<Inbound> {
        let mut p = StreamParser::live();
        fixture.lines().flat_map(|l| p.line(l)).collect()
    }

    fn events(items: &[Inbound]) -> Vec<&ClaudeCodeEvent> {
        items
            .iter()
            .filter_map(|i| match i {
                Inbound::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn initialize_response_yields_models_commands_and_account() {
        let items = parse(INIT);
        let Some(Inbound::ControlResponse { request_id, result: Ok(resp) }) =
            items.iter().find(|i| matches!(i, Inbound::ControlResponse { .. }))
        else {
            panic!("no control response in {items:?}");
        };
        assert_eq!(request_id, "init_1");
        let ClaudeCodeEvent::Capabilities { models, commands, account } = capabilities(resp) else { unreachable!() };
        assert!(models.iter().any(|m| m.value == "default"));
        assert!(models.iter().any(|m| m.value == "sonnet"));
        assert!(models.iter().any(|m| m.value == "haiku"));
        assert!(commands.iter().any(|c| c.name == "compact"));
        let account = account.expect("account");
        assert!(account.logged_in);
        assert_eq!(account.email.as_deref(), Some("user@example.com"));
    }

    #[test]
    fn write_turn_permission_request_and_streaming() {
        let items = parse(WRITE);
        // The CLI asks before the Write tool runs.
        let req = items
            .iter()
            .find_map(|i| match i {
                Inbound::ControlRequest { request_id, request } => Some((request_id, request)),
                _ => None,
            })
            .expect("can_use_tool");
        assert_eq!(req.1["subtype"], "can_use_tool");
        assert_eq!(req.1["tool_name"], "Write");
        assert!(req.1["tool_use_id"].as_str().unwrap().starts_with("toolu_"));
        let ev = events(&items);
        assert!(matches!(ev.first(), Some(ClaudeCodeEvent::Init { claude_session_id, .. }) if !claude_session_id.is_empty()));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::Thinking)));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::TextDelta { .. })));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::ToolUse { name, .. } if name == "Write")));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::ToolUse { name, .. } if name == "Bash")));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::FileChanged { path } if path.ends_with("hello.txt"))));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::ToolResult { content, is_error: false, .. } if content == "done")));
        let deltas: String = ev
            .iter()
            .filter_map(|e| match e {
                ClaudeCodeEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let final_text = ev
            .iter()
            .find_map(|e| match e {
                ClaudeCodeEvent::Text { text, synthetic: false, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(deltas, final_text, "partial deltas add up to the final block");
        assert!(matches!(ev.last(), Some(ClaudeCodeEvent::Result { is_error: false, .. })));
        assert!(!format!("{ev:?}").contains("signature"), "no reasoning artefacts");
    }

    #[test]
    fn denied_tool_and_session_mode_change() {
        let items = parse(DENY);
        let reqs: Vec<&Value> = items
            .iter()
            .filter_map(|i| match i {
                Inbound::ControlRequest { request, .. } => Some(request),
                _ => None,
            })
            .collect();
        assert_eq!(reqs.len(), 2, "Bash then Write asked; the second Write ran under acceptEdits");
        assert_eq!(reqs[0]["tool_name"], "Bash");
        assert!(reqs[0]["permission_suggestions"].as_array().unwrap().len() >= 2);
        let ev = events(&items);
        assert!(ev
            .iter()
            .any(|e| matches!(e, ClaudeCodeEvent::ToolResult { is_error: true, content, .. } if content.contains("User denied"))));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::ModeChanged { mode } if mode == "acceptEdits")));
        assert_eq!(ev.iter().filter(|e| matches!(e, ClaudeCodeEvent::FileChanged { .. })).count(), 2);
    }

    #[test]
    fn interrupt_mode_and_model_answers() {
        let items = parse(INTERRUPT);
        let ok_ids: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                Inbound::ControlResponse { request_id, result: Ok(_) } => Some(request_id.as_str()),
                _ => None,
            })
            .collect();
        for id in ["int_1", "mode_1", "model_1"] {
            assert!(ok_ids.contains(&id), "{id} answered: {ok_ids:?}");
        }
        let ev = events(&items);
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::Interrupted)));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::Result { is_error: true, subtype, .. } if subtype == "error_during_execution")));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::ModeChanged { mode } if mode == "plan")));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::CommandOutput { text } if text.contains("sonnet"))));
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::RateLimit { status, .. } if status == "allowed")));
    }

    #[test]
    fn not_logged_in_is_an_auth_error() {
        let items = parse(NO_LOGIN);
        let bad = items.iter().find_map(|i| match i {
            Inbound::ControlResponse { request_id, result: Err(e) } => Some((request_id.clone(), e.clone())),
            _ => None,
        });
        assert_eq!(bad.unwrap().0, "bad_1", "unknown control subtypes are answered with an error");
        let ev = events(&items);
        assert!(ev.iter().any(|e| matches!(e, ClaudeCodeEvent::Error { auth_required: true, .. })));
        let init = items.iter().find_map(|i| match i {
            Inbound::ControlResponse { request_id, result: Ok(r) } if request_id == "init_1" => Some(r.clone()),
            _ => None,
        });
        let ClaudeCodeEvent::Capabilities { account, .. } = capabilities(&init.unwrap()) else { unreachable!() };
        assert!(!account.unwrap().logged_in);
    }

    #[test]
    fn auth_status_output() {
        let (info, dir) = auth_status(
            "{\n \"loggedIn\": true, \"authMethod\": \"claude.ai\", \"subscriptionType\": \"max\", \"projectsDirectory\": \"C:\\\\x\\\\projects\" }",
        )
        .unwrap();
        assert!(info.logged_in);
        assert_eq!(info.subscription_type.as_deref(), Some("max"));
        assert_eq!(dir.as_deref(), Some("C:\\x\\projects"));
        let (info, _) = auth_status("warning\n{\"loggedIn\": false, \"authMethod\": \"none\"}").unwrap();
        assert!(!info.logged_in && info.auth_method.is_none());
        assert!(auth_status("garbage").is_none());
    }

    #[test]
    fn transcript_mode_keeps_user_text_and_skips_meta() {
        let mut p = StreamParser::transcript();
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Fix the bug"},"isSidechain":false}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/cost</command-name>\n<command-message>cost</command-message>\n<command-args></command-args>"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"secret"},{"type":"text","text":"Done"}]}}"#,
        ];
        let ev: Vec<Inbound> = lines.iter().flat_map(|l| p.line(l)).collect();
        assert_eq!(
            ev,
            vec![
                Inbound::Event(ClaudeCodeEvent::UserMessage { text: "Fix the bug".into() }),
                Inbound::Event(ClaudeCodeEvent::UserMessage { text: "/cost".into() }),
                Inbound::Event(ClaudeCodeEvent::Text { text: "Done".into(), parent_tool_use_id: None, synthetic: false }),
            ]
        );
    }

    #[test]
    fn long_tool_output_is_truncated() {
        let big = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 50);
        let line = serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":big}]}});
        let mut p = StreamParser::live();
        let ev = p.value(&line);
        let Inbound::Event(ClaudeCodeEvent::ToolResult { content, .. }) = &ev[0] else { panic!() };
        assert!(content.chars().count() <= MAX_TOOL_OUTPUT_CHARS + 2);
    }
}
