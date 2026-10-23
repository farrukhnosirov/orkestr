//! `orkestr-fake-codex` — a scripted stand-in for the official `codex` CLI
//! (ADR-0047 tests): `--version`, `login status` and `app-server` (JSON-RPC
//! over stdio, the codex-cli 0.163 message shapes Orkestr uses).
//!
//! Turn behaviour by the message text:
//! * contains `write` → a `fileChange` of `<cwd>/fake.txt` that asks for
//!   approval; on `accept` the file is written;
//! * contains `rm -rf` → a command approval for `rm -rf /`;
//! * contains `LIMIT` → the turn fails with `usageLimitExceeded`;
//! * otherwise → the answer `pong: <text>`.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn note(method: &str, params: Value) {
    out(&json!({ "method": method, "params": params }));
}

struct Pending {
    thread: String,
    item: String,
    path: Option<std::path::PathBuf>,
}

fn finish(thread: &str, text: &str) {
    note("item/agentMessage/delta", json!({"threadId": thread, "turnId": "u", "itemId": "m", "delta": text}));
    note(
        "item/completed",
        json!({"threadId": thread, "turnId": "u", "item": {"id": "m", "type": "agentMessage", "text": text}}),
    );
    let usage = json!({"inputTokens": 120, "cachedInputTokens": 20, "outputTokens": 9, "reasoningOutputTokens": 2, "totalTokens": 129});
    note(
        "thread/tokenUsage/updated",
        json!({"threadId": thread, "turnId": "u", "tokenUsage": {"last": usage, "total": usage, "modelContextWindow": 272000}}),
    );
    note(
        "turn/completed",
        json!({"threadId": thread, "turn": {"id": "u", "items": [], "status": "completed", "durationMs": 5}}),
    );
}

#[allow(clippy::too_many_lines)]
fn app_server() {
    let mut cwds: HashMap<String, String> = HashMap::new();
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut threads = 0u32;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let p = v.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            // A response to one of our approval requests.
            let key = match &id {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if let Some(pd) = pending.remove(&key) {
                let accepted = v.pointer("/result/decision") == Some(&json!("accept"))
                    || v.pointer("/result/decision") == Some(&json!("acceptForSession"));
                if accepted {
                    if let Some(path) = &pd.path {
                        let _ = std::fs::write(path, "hello from fake codex\n");
                    }
                }
                let item = if pd.path.is_some() {
                    json!({"id": pd.item, "type": "fileChange", "status": if accepted {"completed"} else {"declined"}, "changes": [{"path": pd.path.as_ref().map(|p| p.display().to_string()), "kind": {"type": "add"}, "diff": "+hello"}]})
                } else {
                    json!({"id": pd.item, "type": "commandExecution", "command": "rm -rf /", "cwd": ".", "commandActions": [], "status": "declined"})
                };
                note("item/completed", json!({"threadId": pd.thread, "turnId": "u", "item": item}));
                finish(&pd.thread, "Done.\n```json\n{\"status\": \"done\", \"summary\": \"wrote fake.txt\"}\n```\n");
            }
            continue;
        };
        let res = |r: Value| out(&json!({ "id": id, "result": r }));
        match method {
            "initialize" => res(json!({"userAgent": "fake-codex/9.9.9", "codexHome": "fake"})),
            "initialized" => {}
            "account/read" => res(
                json!({"account": {"type": "chatgpt", "email": "dev@example.com", "planType": "plus"}, "requiresOpenaiAuth": true}),
            ),
            "account/rateLimits/read" => res(
                json!({"ordinaryUsageAllowed": true, "rateLimits": {"limitId": "codex", "primary": {"usedPercent": 35, "windowDurationMins": 300, "resetsAt": 1893456000}, "secondary": {"usedPercent": 12, "windowDurationMins": 10080, "resetsAt": 1893456000}, "planType": "plus"}}),
            ),
            "account/login/start" => {
                res(
                    json!({"type": "chatgpt", "loginId": "L1", "authUrl": "https://auth.openai.com/oauth/authorize?fake=1"}),
                );
                note("account/login/completed", json!({"loginId": "L1", "success": true}));
            }
            "account/login/cancel"
            | "account/logout"
            | "turn/interrupt"
            | "thread/unsubscribe"
            | "thread/compact/start" => res(json!({})),
            "model/list" => res(json!({"data": [
                {"id": "gpt-5-codex", "model": "gpt-5-codex", "displayName": "GPT-5 Codex", "description": "Codex", "hidden": false, "isDefault": true, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "medium"}, {"reasoningEffort": "high"}]},
                {"id": "gpt-5", "model": "gpt-5", "displayName": "GPT-5", "description": "General", "hidden": false, "isDefault": false, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [{"reasoningEffort": "minimal"}, {"reasoningEffort": "high"}]}
            ]})),
            "thread/list" => {
                let data: Vec<Value> = cwds
                    .iter()
                    .map(|(t, c)| json!({"id": t, "preview": "earlier chat", "cwd": c, "createdAt": 1_790_000_000, "updatedAt": 1_790_000_100}))
                    .collect();
                res(json!({"data": data}));
            }
            "thread/read" => res(json!({"thread": {"id": p["threadId"], "turns": [{"id": "u", "items": [
                {"id": "q", "type": "userMessage", "content": [{"type": "text", "text": "hello"}]},
                {"id": "m", "type": "agentMessage", "text": "pong: hello"}
            ]}]}})),
            "thread/start" | "thread/resume" | "thread/fork" => {
                let tid = if method == "thread/resume" {
                    p["threadId"].as_str().unwrap_or("t0").to_owned()
                } else {
                    threads += 1;
                    format!("01fake-thread-{threads}")
                };
                cwds.insert(tid.clone(), p["cwd"].as_str().unwrap_or(".").to_owned());
                res(
                    json!({"thread": {"id": tid, "preview": ""}, "model": p.get("model").cloned().unwrap_or(json!("gpt-5-codex")), "reasoningEffort": "medium"}),
                );
                note("thread/started", json!({"thread": {"id": tid}}));
            }
            "turn/start" => {
                let thread = p["threadId"].as_str().unwrap_or_default().to_owned();
                let text = p.pointer("/input/0/text").and_then(Value::as_str).unwrap_or_default().to_owned();
                res(json!({"turn": {"id": "u", "items": [], "status": "inProgress"}}));
                note(
                    "turn/started",
                    json!({"threadId": thread, "turn": {"id": "u", "items": [], "status": "inProgress"}}),
                );
                if text.contains("LIMIT") {
                    note(
                        "turn/completed",
                        json!({"threadId": thread, "turn": {"id": "u", "items": [], "status": "failed", "error": {"message": "You've hit your usage limit. Upgrade to Pro or try again later.", "codexErrorInfo": "usageLimitExceeded"}}}),
                    );
                } else if text.contains("write") {
                    let cwd = cwds.get(&thread).cloned().unwrap_or_else(|| ".".into());
                    let path = std::path::Path::new(&cwd).join("fake.txt");
                    note(
                        "item/started",
                        json!({"threadId": thread, "turnId": "u", "item": {"id": "f1", "type": "fileChange", "status": "inProgress", "changes": [{"path": path.display().to_string(), "kind": {"type": "add"}, "diff": "+hello"}]}}),
                    );
                    let rid = format!("approval-{}", pending.len() + 1);
                    pending
                        .insert(rid.clone(), Pending { thread: thread.clone(), item: "f1".into(), path: Some(path) });
                    out(
                        &json!({"id": rid, "method": "item/fileChange/requestApproval", "params": {"threadId": thread, "turnId": "u", "itemId": "f1", "startedAtMs": 1}}),
                    );
                } else if text.contains("rm -rf") {
                    note(
                        "item/started",
                        json!({"threadId": thread, "turnId": "u", "item": {"id": "c1", "type": "commandExecution", "command": "rm -rf /", "cwd": ".", "commandActions": [], "status": "inProgress"}}),
                    );
                    let rid = format!("approval-{}", pending.len() + 1);
                    pending.insert(rid.clone(), Pending { thread: thread.clone(), item: "c1".into(), path: None });
                    out(
                        &json!({"id": rid, "method": "item/commandExecution/requestApproval", "params": {"threadId": thread, "turnId": "u", "itemId": "c1", "command": "rm -rf /", "cwd": ".", "startedAtMs": 1}}),
                    );
                } else {
                    let short: String = text.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
                    finish(&thread, &format!("pong: {}", short.trim()));
                }
            }
            _ => out(&json!({"id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}})),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => println!("codex-cli 9.9.9"),
        Some("login") => println!("Logged in using ChatGPT"),
        Some("app-server") => app_server(),
        _ => {
            eprintln!("orkestr-fake-codex: unsupported arguments {args:?}");
            std::process::exit(2);
        }
    }
}
