//! Pure mapping of `codex app-server` messages (codex-cli 0.163 protocol)
//! onto Orkestr's types: coding-agent events (agents, Manager chat), panel
//! events, plan limits, models and threads. Tested with recorded messages.

use std::collections::{HashMap, HashSet};

use chrono::{TimeZone, Utc};
use orkestr_domain::Timestamp;
use orkestr_protocol::methods::codex::{
    CodexApprovalKind, CodexEvent, CodexLimitWindow, CodexLimits, CodexModelOption, CodexThread, CodexTokenUsage,
};
use serde_json::{json, Value};

use crate::adapters::cli;
use crate::types::{ChatEvent, CodingAgentEvent, FileChangeKind, ProviderError, StopReason, Usage};

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn ts(secs: Option<i64>) -> Option<Timestamp> {
    secs.and_then(|t| Utc.timestamp_opt(t, 0).single())
}

fn command_text(item: &Value) -> String {
    match item.get("command") {
        Some(Value::String(c)) => c.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

fn change_kind(c: &Value) -> &str {
    c.pointer("/kind/type").and_then(Value::as_str).or_else(|| s(c, "kind")).unwrap_or("update")
}

/// A tool item as `(name, input, summary)` in the Claude Code vocabulary
/// (`Bash`, `Edit`, `WebSearch`, `mcp__server__tool`, `Read`, `Task`).
#[must_use]
pub fn tool_of(item: &Value) -> Option<(String, Value, String)> {
    match s(item, "type")? {
        "commandExecution" => {
            let cmd = command_text(item);
            Some(("Bash".into(), json!({ "command": cmd, "cwd": item.get("cwd") }), cmd))
        }
        "fileChange" => {
            let changes: Vec<Value> = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .map(|x| json!({ "path": s(x, "path"), "kind": change_kind(x), "diff": x.get("diff") }))
                        .collect()
                })
                .unwrap_or_default();
            let paths: Vec<&str> = changes.iter().filter_map(|c| s(c, "path")).collect();
            let first = paths.first().copied().unwrap_or_default().to_owned();
            let summary = paths.join(", ");
            Some(("Edit".into(), json!({ "file_path": first, "changes": changes }), summary))
        }
        "mcpToolCall" => {
            let name = format!("mcp__{}__{}", s(item, "server").unwrap_or("mcp"), s(item, "tool").unwrap_or("tool"));
            let args = item.get("arguments").cloned().unwrap_or(Value::Null);
            let summary = cli::summarize(&args);
            Some((name, args, summary))
        }
        "dynamicToolCall" => {
            let args = item.get("arguments").cloned().unwrap_or(Value::Null);
            let summary = cli::summarize(&args);
            Some((s(item, "tool").unwrap_or("tool").to_owned(), args, summary))
        }
        "webSearch" => {
            let q = s(item, "query").unwrap_or_default().to_owned();
            Some(("WebSearch".into(), json!({ "query": q }), q))
        }
        "imageView" => {
            let p = s(item, "path").unwrap_or_default().to_owned();
            Some(("Read".into(), json!({ "file_path": p }), p))
        }
        "collabAgentToolCall" => {
            let p = s(item, "prompt").unwrap_or_default().to_owned();
            Some(("Task".into(), json!({ "prompt": p }), cli::summarize(&Value::String(p))))
        }
        _ => None,
    }
}

/// `(output, failed)` of a finished tool item.
#[must_use]
pub fn tool_result(item: &Value) -> (String, bool) {
    let status = s(item, "status").unwrap_or("completed");
    let exit_bad = item.get("exitCode").and_then(Value::as_i64).is_some_and(|c| c != 0);
    let failed = matches!(status, "failed" | "declined") || exit_bad || item.get("success") == Some(&json!(false));
    let out = match s(item, "type") {
        Some("commandExecution") => s(item, "aggregatedOutput").unwrap_or_default().to_owned(),
        Some("mcpToolCall") => match item.get("error") {
            Some(e) if !e.is_null() => e.to_string(),
            _ => item.get("result").map(Value::to_string).unwrap_or_default(),
        },
        Some("fileChange") => status.to_owned(),
        _ => status.to_owned(),
    };
    let out: String = if out.chars().count() > 8000 {
        let tail: String = out.chars().rev().take(8000).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    } else {
        out
    };
    (out, failed)
}

/// The error of a failed turn / `error` notification (`TurnError`).
#[must_use]
pub fn turn_error(err: &Value) -> ProviderError {
    let msg = s(err, "message").unwrap_or("Codex turn failed").to_owned();
    let info = err.get("codexErrorInfo").and_then(Value::as_str).unwrap_or_default();
    match info {
        "usageLimitExceeded" | "sessionBudgetExceeded" => ProviderError::QuotaExceeded(msg),
        "rateLimitExceeded" => ProviderError::RateLimited { retry_after_secs: None },
        "unauthorized" => ProviderError::Auth(msg),
        "contextWindowExceeded" | "badRequest" => ProviderError::BadRequest(msg),
        _ => cli::classify_cli_message(&msg).unwrap_or(ProviderError::Server(msg)),
    }
}

fn usage_of(b: &Value) -> CodexTokenUsage {
    let n = |k: &str| b.get(k).and_then(Value::as_u64).unwrap_or(0);
    CodexTokenUsage {
        input_tokens: n("inputTokens"),
        cached_input_tokens: n("cachedInputTokens"),
        output_tokens: n("outputTokens"),
        reasoning_tokens: n("reasoningOutputTokens"),
    }
}

/// Orkestr usage of a token breakdown.
#[must_use]
pub fn to_usage(u: CodexTokenUsage) -> Usage {
    Usage {
        input_tokens: u.input_tokens,
        cached_input_tokens: u.cached_input_tokens,
        output_tokens: u.output_tokens,
        reasoning_tokens: u.reasoning_tokens,
        ..Usage::default()
    }
}

/// Coding-agent events of one turn on one thread (agents, Manager chat).
#[derive(Debug, Default)]
pub struct TurnParser {
    started: HashSet<String>,
    streamed: HashSet<String>,
    /// Paths of file-change items (for approvals).
    pub file_items: HashMap<String, Vec<String>>,
    /// Commands of command items (for approvals without a command).
    pub command_items: HashMap<String, String>,
    usage: Option<Usage>,
    last_error: Option<ProviderError>,
    /// The turn ended (terminal event emitted).
    pub terminal: bool,
}

impl TurnParser {
    /// Maps one notification.
    pub fn notification(&mut self, method: &str, p: &Value) -> Vec<CodingAgentEvent> {
        let mut out = Vec::new();
        if self.terminal {
            return out;
        }
        match method {
            "item/started" => {
                let Some(item) = p.get("item") else { return out };
                let id = s(item, "id").unwrap_or_default().to_owned();
                if let Some((tool, input, summary)) = tool_of(item) {
                    if tool == "Edit" {
                        let paths = input["changes"]
                            .as_array()
                            .map(|c| c.iter().filter_map(|x| s(x, "path").map(str::to_owned)).collect())
                            .unwrap_or_default();
                        self.file_items.insert(id.clone(), paths);
                    }
                    if tool == "Bash" {
                        self.command_items.insert(id.clone(), summary.clone());
                    }
                    self.started.insert(id.clone());
                    out.push(CodingAgentEvent::ToolStarted { call_id: id, tool, summary });
                }
            }
            "item/agentMessage/delta" => {
                if let Some(d) = s(p, "delta").filter(|d| !d.is_empty()) {
                    self.streamed.insert(s(p, "itemId").unwrap_or_default().to_owned());
                    out.push(CodingAgentEvent::Chat(ChatEvent::TextDelta(d.to_owned())));
                }
            }
            "item/completed" => {
                let Some(item) = p.get("item") else { return out };
                let id = s(item, "id").unwrap_or_default().to_owned();
                if s(item, "type") == Some("agentMessage") {
                    if !self.streamed.contains(&id) {
                        if let Some(t) = s(item, "text").filter(|t| !t.is_empty()) {
                            out.push(CodingAgentEvent::Chat(ChatEvent::TextDelta(t.to_owned())));
                        }
                    }
                    return out;
                }
                let Some((tool, _, summary)) = tool_of(item) else { return out };
                if !self.started.remove(&id) {
                    out.push(CodingAgentEvent::ToolStarted { call_id: id.clone(), tool: tool.clone(), summary });
                }
                let (output, failed) = tool_result(item);
                out.push(CodingAgentEvent::ToolFinished {
                    call_id: id,
                    ok: !failed,
                    summary: cli::summarize(&Value::String(output)),
                });
                if !failed && tool == "Edit" {
                    for c in item.get("changes").and_then(Value::as_array).into_iter().flatten() {
                        let Some(path) = s(c, "path") else { continue };
                        let change = match change_kind(c) {
                            "add" => FileChangeKind::Added,
                            "delete" => FileChangeKind::Deleted,
                            _ => FileChangeKind::Modified,
                        };
                        out.push(CodingAgentEvent::FileChanged { path: path.to_owned(), change });
                    }
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(last) = p.pointer("/tokenUsage/last") {
                    self.usage = Some(to_usage(usage_of(last)));
                }
            }
            "error" => {
                if p.get("willRetry") != Some(&json!(true)) {
                    if let Some(e) = p.get("error") {
                        self.last_error = Some(turn_error(e));
                    }
                }
            }
            "turn/completed" => {
                self.terminal = true;
                let turn = p.get("turn").cloned().unwrap_or(Value::Null);
                match s(&turn, "status").unwrap_or("completed") {
                    "completed" => {
                        out.push(CodingAgentEvent::Chat(ChatEvent::Usage(self.usage.unwrap_or_default())));
                        out.push(CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn }));
                    }
                    "interrupted" => out.push(cli::error_event(ProviderError::Cancelled)),
                    _ => {
                        let err = turn
                            .get("error")
                            .filter(|e| !e.is_null())
                            .map(turn_error)
                            .or_else(|| self.last_error.clone())
                            .unwrap_or_else(|| ProviderError::Server("the Codex turn failed".into()));
                        out.push(cli::error_event(err));
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// The terminal event when the process ended before the turn did.
    #[must_use]
    pub fn died(&self) -> CodingAgentEvent {
        cli::error_event(
            self.last_error
                .clone()
                .unwrap_or_else(|| ProviderError::Server("the Codex app-server ended during the turn".into())),
        )
    }
}

/// An approval request of the CLI, normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    /// Kind.
    pub kind: CodexApprovalKind,
    /// Item id.
    pub item_id: String,
    /// Thread.
    pub thread_id: String,
    /// Tool (`Bash` / `Edit`).
    pub tool: String,
    /// Input (`{command, cwd}` / `{file_path, paths}`).
    pub input: Value,
    /// Reason.
    pub reason: Option<String>,
}

/// Parses an approval request (`item/commandExecution/requestApproval`,
/// `item/fileChange/requestApproval`); `None` for other requests.
#[must_use]
pub fn approval(
    method: &str,
    p: &Value,
    files: &HashMap<String, Vec<String>>,
    commands: &HashMap<String, String>,
) -> Option<Approval> {
    let item_id = s(p, "itemId").unwrap_or_default().to_owned();
    let thread_id = s(p, "threadId").unwrap_or_default().to_owned();
    let reason = s(p, "reason").map(str::to_owned);
    match method {
        "item/commandExecution/requestApproval" => {
            let cmd =
                s(p, "command").map(str::to_owned).or_else(|| commands.get(&item_id).cloned()).unwrap_or_default();
            Some(Approval {
                kind: CodexApprovalKind::Command,
                tool: "Bash".into(),
                input: json!({ "command": cmd, "cwd": p.get("cwd") }),
                item_id,
                thread_id,
                reason,
            })
        }
        "item/fileChange/requestApproval" => {
            let mut paths = files.get(&item_id).cloned().unwrap_or_default();
            if paths.is_empty() {
                if let Some(root) = s(p, "grantRoot") {
                    paths.push(root.to_owned());
                }
            }
            let first = paths.first().cloned().unwrap_or_default();
            Some(Approval {
                kind: CodexApprovalKind::FileChange,
                tool: "Edit".into(),
                input: json!({ "file_path": first, "paths": paths }),
                item_id,
                thread_id,
                reason,
            })
        }
        _ => None,
    }
}

/// The CLI's answer to an approval (`accept`, `acceptForSession`, `decline`, `cancel`).
#[must_use]
pub fn approval_answer(allow: bool, session: bool, abort: bool) -> Value {
    let d = match (allow, session, abort) {
        (true, true, _) => "acceptForSession",
        (true, false, _) => "accept",
        (false, _, true) => "cancel",
        (false, _, false) => "decline",
    };
    json!({ "decision": d })
}

/// Panel events of one session (stateful: which messages streamed).
#[derive(Debug, Default)]
pub struct PanelMapper {
    streamed: HashSet<String>,
    started: HashSet<String>,
    /// Paths of file-change items.
    pub file_items: HashMap<String, Vec<String>>,
    /// Commands of command items.
    pub command_items: HashMap<String, String>,
    /// The running turn.
    pub turn_id: Option<String>,
}

impl PanelMapper {
    /// Maps one notification of the session's thread.
    pub fn notification(&mut self, method: &str, p: &Value) -> Vec<CodexEvent> {
        let mut out = Vec::new();
        match method {
            "turn/started" => {
                let id = p.pointer("/turn/id").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.turn_id = Some(id.clone());
                out.push(CodexEvent::TurnStarted { turn_id: id });
            }
            "item/agentMessage/delta" => {
                if let Some(d) = s(p, "delta").filter(|d| !d.is_empty()) {
                    let id = s(p, "itemId").unwrap_or_default().to_owned();
                    self.streamed.insert(id.clone());
                    out.push(CodexEvent::TextDelta { item_id: id, text: d.to_owned() });
                }
            }
            "item/started" => {
                let Some(item) = p.get("item") else { return out };
                let id = s(item, "id").unwrap_or_default().to_owned();
                if let Some((name, input, summary)) = tool_of(item) {
                    if name == "Edit" {
                        let paths = input["changes"]
                            .as_array()
                            .map(|c| c.iter().filter_map(|x| s(x, "path").map(str::to_owned)).collect())
                            .unwrap_or_default();
                        self.file_items.insert(id.clone(), paths);
                    }
                    if name == "Bash" {
                        self.command_items.insert(id.clone(), summary);
                    }
                    self.started.insert(id.clone());
                    out.push(CodexEvent::ToolUse { id, name, input });
                }
            }
            "item/completed" => {
                if let Some(item) = p.get("item") {
                    out.extend(self.item_completed(item));
                }
            }
            "turn/plan/updated" => {
                let steps: Vec<String> = p
                    .get("plan")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|x| {
                                let mark = match s(x, "status") {
                                    Some("completed") => "[x]",
                                    Some("inProgress") => "[~]",
                                    _ => "[ ]",
                                };
                                format!("{mark} {}", s(x, "step").unwrap_or_default())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !steps.is_empty() {
                    out.push(CodexEvent::Plan { text: steps.join("\n") });
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(u) = p.get("tokenUsage") {
                    out.push(CodexEvent::TokenUsage {
                        last: usage_of(&u["last"]),
                        total: usage_of(&u["total"]),
                        context_window: u.get("modelContextWindow").and_then(Value::as_u64),
                    });
                }
            }
            "turn/completed" => {
                self.turn_id = None;
                let t = p.get("turn").cloned().unwrap_or(Value::Null);
                let error = t.get("error").filter(|e| !e.is_null());
                if let Some(e) = error {
                    out.push(error_event(&turn_error(e), false));
                }
                out.push(CodexEvent::TurnCompleted {
                    turn_id: s(&t, "id").unwrap_or_default().to_owned(),
                    status: s(&t, "status").unwrap_or("completed").to_owned(),
                    error: error.and_then(|e| s(e, "message")).map(str::to_owned),
                    duration_ms: t.get("durationMs").and_then(Value::as_u64),
                });
            }
            "error" => {
                if let Some(e) = p.get("error") {
                    let retry = p.get("willRetry") == Some(&json!(true));
                    out.push(error_event(&turn_error(e), retry));
                }
            }
            "thread/compacted" => out.push(CodexEvent::Info { text: "context compacted".into() }),
            "model/rerouted" => out.push(CodexEvent::ModelChanged {
                model: s(p, "toModel").or_else(|| s(p, "model")).map(str::to_owned),
                effort: None,
            }),
            "warning" | "configWarning" | "deprecationNotice" => {
                if let Some(m) = s(p, "message").or_else(|| s(p, "summary")) {
                    out.push(CodexEvent::Info { text: m.to_owned() });
                }
            }
            _ => {}
        }
        out
    }

    fn item_completed(&mut self, item: &Value) -> Vec<CodexEvent> {
        let mut out = Vec::new();
        let id = s(item, "id").unwrap_or_default().to_owned();
        match s(item, "type").unwrap_or_default() {
            "agentMessage" => {
                let text = s(item, "text").unwrap_or_default().to_owned();
                self.streamed.remove(&id);
                out.push(CodexEvent::Text { item_id: id, text });
            }
            "reasoning" => {
                let text = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    out.push(CodexEvent::Reasoning { text });
                }
            }
            "plan" => out.push(CodexEvent::Plan { text: s(item, "text").unwrap_or_default().to_owned() }),
            "userMessage" => {}
            _ => {
                let Some((name, input, _)) = tool_of(item) else { return out };
                if !self.started.remove(&id) {
                    out.push(CodexEvent::ToolUse { id: id.clone(), name: name.clone(), input });
                }
                let (content, is_error) = tool_result(item);
                out.push(CodexEvent::ToolResult { tool_use_id: id, content, is_error });
                if !is_error && name == "Edit" {
                    for c in item.get("changes").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(p) = s(c, "path") {
                            out.push(CodexEvent::FileChanged { path: p.to_owned() });
                        }
                    }
                }
            }
        }
        out
    }
}

/// A panel error event.
#[must_use]
pub fn error_event(e: &ProviderError, will_retry: bool) -> CodexEvent {
    CodexEvent::Error {
        message: match e {
            ProviderError::QuotaExceeded(m) | ProviderError::Auth(m) | ProviderError::Server(m) => m.clone(),
            other => other.to_string(),
        },
        auth_required: matches!(e, ProviderError::Auth(_)),
        usage_limit: matches!(e, ProviderError::QuotaExceeded(_) | ProviderError::RateLimited { .. }),
        will_retry,
    }
}

/// The items of a saved thread (`thread/read` with turns) as panel events.
#[must_use]
pub fn transcript(thread: &Value) -> Vec<CodexEvent> {
    let mut out = Vec::new();
    let mut m = PanelMapper::default();
    for turn in thread.get("turns").and_then(Value::as_array).into_iter().flatten() {
        for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
            if s(item, "type") == Some("userMessage") {
                let text: Vec<&str> = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|c| c.iter().filter_map(|x| s(x, "text")).collect())
                    .unwrap_or_default();
                out.push(CodexEvent::UserMessage { text: text.join("\n") });
            } else {
                out.extend(m.item_completed(item));
            }
        }
    }
    out
}

fn window(w: Option<&Value>) -> Option<CodexLimitWindow> {
    let w = w.filter(|w| !w.is_null())?;
    Some(CodexLimitWindow {
        percent: w.get("usedPercent").and_then(Value::as_f64).unwrap_or(0.0),
        window_minutes: w.get("windowDurationMins").and_then(Value::as_u64),
        resets_at: ts(w.get("resetsAt").and_then(Value::as_i64)),
    })
}

/// Plan limits of `account/rateLimits/read` (or a `RateLimitSnapshot`).
#[must_use]
pub fn limits(v: &Value, fetched_at: Timestamp) -> CodexLimits {
    let snap =
        v.pointer("/rateLimitsByLimitId/codex").or_else(|| v.get("rateLimits")).filter(|x| !x.is_null()).unwrap_or(v);
    let primary = window(snap.get("primary"));
    let secondary = window(snap.get("secondary"));
    CodexLimits {
        available: primary.is_some() || secondary.is_some(),
        plan: s(snap, "planType").map(str::to_owned),
        primary,
        secondary,
        credits: snap
            .pointer("/credits/hasCredits")
            .and_then(Value::as_bool)
            .filter(|h| *h)
            .and_then(|_| snap.pointer("/credits/balance"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        limit_reached: s(snap, "rateLimitReachedType").map(str::to_owned),
        usage_allowed: v.get("ordinaryUsageAllowed").and_then(Value::as_bool),
        fetched_at,
        error: None,
    }
}

/// Models of `model/list` (hidden ones skipped).
#[must_use]
pub fn models(v: &Value) -> Vec<CodexModelOption> {
    v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("hidden") != Some(&json!(true)))
        .filter_map(|m| {
            let id = s(m, "id").or_else(|| s(m, "model"))?.to_owned();
            Some(CodexModelOption {
                display_name: s(m, "displayName").unwrap_or(&id).to_owned(),
                description: s(m, "description").unwrap_or_default().to_owned(),
                efforts: m
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|e| s(e, "reasoningEffort").map(str::to_owned)).collect())
                    .unwrap_or_default(),
                default_effort: s(m, "defaultReasoningEffort").map(str::to_owned),
                is_default: m.get("isDefault") == Some(&json!(true)),
                id,
            })
        })
        .collect()
}

/// Built-in model list (when the CLI cannot be asked).
#[must_use]
pub fn fallback_models() -> Vec<CodexModelOption> {
    let efforts = || vec!["low".to_owned(), "medium".into(), "high".into()];
    [
        ("gpt-5-codex", "GPT-5 Codex", "Codex-tuned GPT-5 for agentic coding", true),
        ("gpt-5.1-codex", "GPT-5.1 Codex", "Newer Codex model", false),
        ("gpt-5.1-codex-mini", "GPT-5.1 Codex mini", "Faster, cheaper Codex model", false),
        ("gpt-5", "GPT-5", "General GPT-5", false),
    ]
    .into_iter()
    .map(|(id, name, d, def)| CodexModelOption {
        id: id.into(),
        display_name: name.into(),
        description: d.into(),
        efforts: efforts(),
        default_effort: Some("medium".into()),
        is_default: def,
    })
    .collect()
}

/// A thread of `thread/list` / `thread/read`.
#[must_use]
pub fn thread(t: &Value) -> Option<CodexThread> {
    let id = s(t, "id")?.to_owned();
    let preview = s(t, "preview").unwrap_or_default();
    let title = s(t, "name").filter(|n| !n.trim().is_empty()).unwrap_or(preview);
    let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
    Some(CodexThread {
        id,
        title,
        cwd: s(t, "cwd").map(str::to_owned),
        model: s(t, "model").map(str::to_owned),
        created_at: ts(t.get("createdAt").and_then(Value::as_i64)),
        updated_at: ts(t.get("updatedAt").and_then(Value::as_i64)),
        forked_from: s(t, "forkedFromId").map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(method: &str, params: Value) -> (String, Value) {
        (method.to_owned(), params)
    }

    /// A recorded turn (codex-cli 0.163 app-server, trimmed).
    fn turn() -> Vec<(String, Value)> {
        vec![
            n("turn/started", json!({"threadId":"t1","turn":{"id":"u1","items":[],"status":"inProgress"}})),
            n(
                "item/completed",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"r0","type":"reasoning","summary":["**Plan** look at files"],"content":["secret chain"]}}),
            ),
            n(
                "item/started",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"c1","type":"commandExecution","command":"ls","cwd":"/wt","commandActions":[],"status":"inProgress"}}),
            ),
            n(
                "item/completed",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"c1","type":"commandExecution","command":"ls","cwd":"/wt","commandActions":[],"status":"completed","exitCode":0,"aggregatedOutput":"README.md\n"}}),
            ),
            n(
                "item/started",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"f1","type":"fileChange","status":"inProgress","changes":[{"path":"/wt/src/a.rs","kind":{"type":"add"},"diff":"+x"}]}}),
            ),
            n(
                "item/completed",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"f1","type":"fileChange","status":"completed","changes":[{"path":"/wt/src/a.rs","kind":{"type":"add"},"diff":"+x"}]}}),
            ),
            n("item/agentMessage/delta", json!({"threadId":"t1","turnId":"u1","itemId":"m1","delta":"Added "})),
            n("item/agentMessage/delta", json!({"threadId":"t1","turnId":"u1","itemId":"m1","delta":"a.rs."})),
            n(
                "item/completed",
                json!({"threadId":"t1","turnId":"u1","item":{"id":"m1","type":"agentMessage","text":"Added a.rs."}}),
            ),
            n(
                "thread/tokenUsage/updated",
                json!({"threadId":"t1","turnId":"u1","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":40,"outputTokens":7,"reasoningOutputTokens":3,"totalTokens":107},"total":{"inputTokens":100,"cachedInputTokens":40,"outputTokens":7,"reasoningOutputTokens":3,"totalTokens":107},"modelContextWindow":272000}}),
            ),
            n(
                "turn/completed",
                json!({"threadId":"t1","turn":{"id":"u1","items":[],"status":"completed","durationMs":1234}}),
            ),
        ]
    }

    #[test]
    fn agent_turn_maps_to_coding_events() {
        let mut p = TurnParser::default();
        let ev: Vec<CodingAgentEvent> = turn().iter().flat_map(|(m, v)| p.notification(m, v)).collect();
        assert!(
            matches!(&ev[0], CodingAgentEvent::ToolStarted { tool, summary, .. } if tool == "Bash" && summary == "ls")
        );
        assert!(matches!(&ev[1], CodingAgentEvent::ToolFinished { ok: true, .. }));
        assert!(matches!(&ev[2], CodingAgentEvent::ToolStarted { tool, .. } if tool == "Edit"));
        assert!(matches!(&ev[3], CodingAgentEvent::ToolFinished { ok: true, .. }));
        assert_eq!(ev[4], CodingAgentEvent::FileChanged { path: "/wt/src/a.rs".into(), change: FileChangeKind::Added });
        assert_eq!(ev[5], CodingAgentEvent::Chat(ChatEvent::TextDelta("Added ".into())));
        assert_eq!(ev[6], CodingAgentEvent::Chat(ChatEvent::TextDelta("a.rs.".into())));
        assert!(
            matches!(&ev[7], CodingAgentEvent::Chat(ChatEvent::Usage(u)) if u.input_tokens == 100 && u.output_tokens == 7)
        );
        assert_eq!(ev[8], CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn }));
        assert_eq!(ev.len(), 9, "the completed message is not repeated after its deltas");
        assert!(!format!("{ev:?}").contains("secret chain"));
        assert_eq!(p.file_items["f1"], vec!["/wt/src/a.rs".to_owned()]);
    }

    #[test]
    fn panel_turn_maps_to_panel_events() {
        let mut m = PanelMapper::default();
        let ev: Vec<CodexEvent> = turn().iter().flat_map(|(k, v)| m.notification(k, v)).collect();
        assert_eq!(ev[0], CodexEvent::TurnStarted { turn_id: "u1".into() });
        assert_eq!(ev[1], CodexEvent::Reasoning { text: "**Plan** look at files".into() });
        assert!(
            matches!(&ev[2], CodexEvent::ToolUse { name, input, .. } if name == "Bash" && input["command"] == "ls")
        );
        assert!(matches!(&ev[3], CodexEvent::ToolResult { content, is_error: false, .. } if content == "README.md\n"));
        assert!(
            matches!(&ev[4], CodexEvent::ToolUse { name, input, .. } if name == "Edit" && input["file_path"] == "/wt/src/a.rs")
        );
        assert_eq!(ev[6], CodexEvent::FileChanged { path: "/wt/src/a.rs".into() });
        assert_eq!(ev[9], CodexEvent::Text { item_id: "m1".into(), text: "Added a.rs.".into() });
        assert!(matches!(ev[10], CodexEvent::TokenUsage { context_window: Some(272_000), .. }));
        assert!(
            matches!(&ev[11], CodexEvent::TurnCompleted { status, duration_ms: Some(1234), .. } if status == "completed")
        );
        assert!(!format!("{ev:?}").contains("secret chain"));
    }

    #[test]
    fn failures_and_usage_limits_are_classified() {
        let mut p = TurnParser::default();
        let ev = p.notification(
            "turn/completed",
            &json!({"turn":{"id":"u","status":"failed","error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}),
        );
        assert!(matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::QuotaExceeded(_))))));
        let mut p = TurnParser::default();
        p.notification(
            "error",
            &json!({"willRetry": false, "error":{"message":"401 Unauthorized","codexErrorInfo":"unauthorized"}}),
        );
        assert!(matches!(p.died(), CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Auth(_)))));
        let mut m = PanelMapper::default();
        let ev = m.notification(
            "error",
            &json!({"willRetry": false, "error":{"message":"limit","codexErrorInfo":"usageLimitExceeded"}}),
        );
        assert!(matches!(&ev[0], CodexEvent::Error { usage_limit: true, auth_required: false, .. }));
    }

    #[test]
    fn approvals_and_answers() {
        let mut files = HashMap::new();
        files.insert("f1".to_owned(), vec!["/wt/a.rs".to_owned()]);
        let a = approval(
            "item/fileChange/requestApproval",
            &json!({"itemId":"f1","threadId":"t","turnId":"u","startedAtMs":1}),
            &files,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!((a.tool.as_str(), a.input["file_path"].as_str()), ("Edit", Some("/wt/a.rs")));
        let c = approval(
            "item/commandExecution/requestApproval",
            &json!({"itemId":"c","threadId":"t","turnId":"u","command":"rm -rf /","cwd":"/wt","startedAtMs":1}),
            &files,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(c.input["command"], "rm -rf /");
        assert!(approval("item/tool/requestUserInput", &json!({}), &files, &HashMap::new()).is_none());
        assert_eq!(approval_answer(true, false, false), json!({"decision":"accept"}));
        assert_eq!(approval_answer(true, true, false), json!({"decision":"acceptForSession"}));
        assert_eq!(approval_answer(false, false, true), json!({"decision":"cancel"}));
        assert_eq!(approval_answer(false, false, false), json!({"decision":"decline"}));
    }

    #[test]
    fn limits_models_threads() {
        // Recorded `account/rateLimits/read` (ChatGPT prolite plan).
        let v = json!({"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"codex","primary":{"usedPercent":0,"windowDurationMins":10080,"resetsAt":1793302592},"secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"planType":"prolite","rateLimitReachedType":null},"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1793302592},"secondary":{"usedPercent":7,"windowDurationMins":10080,"resetsAt":null},"planType":"prolite"}}});
        let l = limits(&v, Utc::now());
        assert!(l.available);
        assert_eq!(l.plan.as_deref(), Some("prolite"));
        assert_eq!(l.primary.as_ref().unwrap().percent, 42.0);
        assert_eq!(l.secondary.as_ref().unwrap().window_minutes, Some(10080));
        assert_eq!(l.usage_allowed, Some(true));
        assert!(!limits(&json!({"rateLimits":{}}), Utc::now()).available);

        let m = models(&json!({"data":[
            {"id":"gpt-5-codex","model":"gpt-5-codex","displayName":"GPT-5 Codex","description":"d","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]},
            {"id":"hidden","model":"hidden","displayName":"H","description":"","hidden":true,"isDefault":false,"defaultReasoningEffort":"low","supportedReasoningEfforts":[]}
        ]}));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].efforts, vec!["low", "high"]);
        assert!(fallback_models().iter().any(|m| m.id == "gpt-5-codex"));

        let t = thread(&json!({"id":"01a","preview":"fix  the\nbug","name":null,"cwd":"C:\\p","createdAt":1792625915,"updatedAt":1792625949})).unwrap();
        assert_eq!(t.title, "fix the bug");
        assert!(t.updated_at.is_some());
        let tr = transcript(&json!({"turns":[{"items":[
            {"id":"u","type":"userMessage","content":[{"type":"text","text":"hi"}]},
            {"id":"m","type":"agentMessage","text":"hello"}
        ]}]}));
        assert_eq!(
            tr,
            vec![
                CodexEvent::UserMessage { text: "hi".into() },
                CodexEvent::Text { item_id: "m".into(), text: "hello".into() }
            ]
        );
    }
}
