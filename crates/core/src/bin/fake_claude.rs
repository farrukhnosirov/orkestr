//! `orkestr-fake-claude` — a test double of the official `claude` CLI for
//! the Claude Code panel's end-to-end tests. It speaks the same stream-json
//! and control protocol as Claude Code 2.1.x (shapes copied from recordings of
//! the real binary, see `crates/providers/tests/fixtures/claude_code`).
//! Never shipped; never used outside tests.
//!
//! Behaviour by user message: contains `write` → asks to Write `fake.txt`
//! (writes it when allowed); contains `rm` → asks to run `rm -rf /`;
//! contains `slow` → streams until interrupted; anything else → echoes.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

const SESSION: &str = "7a0c9a52-5b3e-4a7e-9f3c-2f0f6c1d9e11";

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn ok(id: &str, response: Value) {
    out(
        &json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}}),
    );
}

struct Fake {
    rx: mpsc::Receiver<Value>,
    allow_bypass: bool,
    mode: String,
    model: String,
    started: bool,
    cwd: String,
    perm_seq: u32,
}

impl Fake {
    /// Handles a control request; returns true for `interrupt`.
    fn control(&mut self, v: &Value) -> bool {
        let id = v["request_id"].as_str().unwrap_or_default().to_owned();
        let req = &v["request"];
        match req["subtype"].as_str().unwrap_or_default() {
            "initialize" => ok(
                &id,
                json!({
                    "commands": [{"name": "compact", "description": "Compact", "argumentHint": ""}, {"name": "cost", "description": "Cost"}],
                    "models": [
                        {"value": "default", "displayName": "Default (recommended)", "description": "Opus"},
                        {"value": "sonnet", "displayName": "Sonnet"},
                        {"value": "opus", "displayName": "Opus"},
                        {"value": "haiku", "displayName": "Haiku"}
                    ],
                    "account": {"email": "dev@example.com", "subscriptionType": "Claude Max", "apiProvider": "firstParty"},
                    "current_permission_mode": self.mode,
                }),
            ),
            "interrupt" => {
                ok(&id, json!({"still_queued": []}));
                return true;
            }
            "set_permission_mode" => {
                let mode = req["mode"].as_str().unwrap_or_default().to_owned();
                if mode == "bypassPermissions" && !self.allow_bypass {
                    out(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": id,
                        "error": "Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions",
                        "error_code": "bypass_not_launched"}}));
                } else {
                    self.mode.clone_from(&mode);
                    ok(&id, json!({"mode": mode}));
                    out(
                        &json!({"type": "system", "subtype": "status", "status": null, "permissionMode": mode, "session_id": SESSION}),
                    );
                }
            }
            "set_model" => {
                self.model = req["model"].as_str().unwrap_or("default").to_owned();
                out(
                    &json!({"type": "user", "message": {"role": "user", "content": format!("<local-command-stdout>Set model to `{}`</local-command-stdout>", self.model)}, "isReplay": true, "session_id": SESSION}),
                );
                ok(&id, Value::Null);
            }
            other => out(
                &json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": format!("Unsupported control request subtype: {other}")}}),
            ),
        }
        false
    }

    fn next(&mut self, timeout: Duration) -> Option<Option<Value>> {
        match self.rx.recv_timeout(timeout) {
            Ok(v) => Some(Some(v)),
            Err(mpsc::RecvTimeoutError::Timeout) => Some(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Asks for a tool permission; returns the CLI-side `behavior` answer.
    fn ask(&mut self, tool: &str, tool_use_id: &str, input: &Value) -> Option<Value> {
        self.perm_seq += 1;
        let rid = format!("perm-{}", self.perm_seq);
        out(&json!({"type": "control_request", "request_id": rid, "request": {
            "subtype": "can_use_tool", "tool_name": tool, "display_name": tool, "input": input,
            "description": tool, "permission_suggestions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}],
            "tool_use_id": tool_use_id}}));
        loop {
            let v = self.next(Duration::from_secs(60))??;
            if v["type"] == "control_response" && v["response"]["request_id"] == rid.as_str() {
                return Some(v["response"]["response"].clone());
            }
            if v["type"] == "control_request" {
                self.control(&v);
            }
        }
    }

    fn assistant(content: Value) {
        out(
            &json!({"type": "assistant", "message": {"model": "fake", "role": "assistant", "content": [content]}, "parent_tool_use_id": null, "session_id": SESSION}),
        );
    }

    fn text(t: &str) {
        for chunk in t.split_inclusive(' ') {
            out(
                &json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": chunk}}, "parent_tool_use_id": null, "session_id": SESSION}),
            );
        }
        Self::assistant(json!({"type": "text", "text": t}));
    }

    fn result(is_error: bool, subtype: &str, text: &str) {
        out(&json!({"type": "result", "subtype": subtype, "is_error": is_error, "duration_ms": 12, "num_turns": 1,
            "result": text, "session_id": SESSION, "total_cost_usd": 0.0012,
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 0}}));
    }

    fn tool_result(id: &str, content: &str, is_error: bool) {
        out(
            &json!({"type": "user", "message": {"role": "user", "content": [{"tool_use_id": id, "type": "tool_result", "content": content, "is_error": is_error}]}, "parent_tool_use_id": null, "session_id": SESSION}),
        );
    }

    fn turn(&mut self, text: &str) -> Option<()> {
        if !self.started {
            self.started = true;
            out(
                &json!({"type": "system", "subtype": "init", "cwd": self.cwd, "session_id": SESSION, "tools": ["Bash", "Edit", "Read", "Write"], "model": self.model, "permissionMode": self.mode}),
            );
        }
        out(
            &json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}, "session_id": SESSION}),
        );
        if text.contains("write") {
            Self::text("I'll write it.");
            let path = std::path::Path::new(&self.cwd).join("fake.txt").display().to_string();
            let input = json!({"file_path": path, "content": "hello"});
            Self::assistant(json!({"type": "tool_use", "id": "toolu_w1", "name": "Write", "input": input}));
            let answer = self.ask("Write", "toolu_w1", &input)?;
            if answer["behavior"] == "allow" {
                let _ = std::fs::write(&path, "hello");
                Self::tool_result("toolu_w1", &format!("File created successfully at: {path}"), false);
                Self::text("Done.");
                Self::result(false, "success", "Done.");
            } else {
                Self::tool_result("toolu_w1", answer["message"].as_str().unwrap_or("denied"), true);
                Self::text("OK, not writing.");
                Self::result(false, "success", "OK, not writing.");
            }
        } else if text.contains("rm") {
            let input = json!({"command": "rm -rf /", "description": "wipe"});
            Self::assistant(json!({"type": "tool_use", "id": "toolu_b1", "name": "Bash", "input": input}));
            let answer = self.ask("Bash", "toolu_b1", &input)?;
            let ok = answer["behavior"] == "allow";
            Self::tool_result("toolu_b1", if ok { "ran" } else { answer["message"].as_str().unwrap_or("denied") }, !ok);
            Self::result(false, "success", "after rm");
        } else if text.contains("slow") {
            for i in 0..400 {
                match self.next(Duration::from_millis(50)) {
                    None => return None,
                    // `control` answers every request; only `interrupt` ends the turn.
                    Some(Some(v)) if v["type"] == "control_request" && self.control(&v) => {
                        out(
                            &json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}, "session_id": SESSION}),
                        );
                        Self::result(true, "error_during_execution", "");
                        return Some(());
                    }
                    _ => {}
                }
                out(
                    &json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": format!("{i} ")}}, "session_id": SESSION}),
                );
            }
            Self::result(false, "success", "counted");
        } else {
            Self::text(&format!("pong: {text}"));
            Self::result(false, "success", "pong");
        }
        Some(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("9.9.9 (Claude Code)");
        return;
    }
    if args.first().map(String::as_str) == Some("auth") {
        if args.get(1).map(String::as_str) == Some("status") {
            println!(
                "{}",
                json!({"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max", "email": "dev@example.com"})
            );
        } else {
            println!("Fake login: press Enter");
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
        }
        return;
    }
    let streaming = args.windows(2).any(|w| w[0] == "--input-format" && w[1] == "stream-json");
    if !streaming {
        // `claude -p` with the prompt on stdin (chat / plan calls): one answer.
        // A prompt offering Orkestr's emulated tools gets a tool call the
        // first time and a plain answer once a tool result is in it.
        let mut prompt = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
        out(
            &json!({"type": "system", "subtype": "init", "session_id": SESSION, "model": "claude-fake-1", "tools": []}),
        );
        let answer = if prompt.contains("\"tool_calls\"") && !prompt.contains("Tool result for call") {
            r#"{"tool_calls": [{"name": "list_tasks", "arguments": {}}]}"#.to_owned()
        } else {
            format!("pong: {}", prompt.chars().filter(|c| !c.is_control()).take(40).collect::<String>())
        };
        Fake::assistant(json!({"type": "text", "text": answer}));
        Fake::result(false, "success", &answer);
        return;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let arg_after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let mut fake = Fake {
        rx,
        allow_bypass: args.iter().any(|a| a == "--allow-dangerously-skip-permissions"),
        mode: arg_after("--permission-mode").unwrap_or_else(|| "default".into()),
        model: arg_after("--model").unwrap_or_else(|| "claude-fake-1".into()),
        started: false,
        cwd: std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        perm_seq: 0,
    };
    while let Some(next) = fake.next(Duration::from_secs(3600)) {
        let Some(v) = next else { continue };
        match v["type"].as_str() {
            Some("control_request") => {
                fake.control(&v);
            }
            Some("user") => {
                let text = v["message"]["content"][0]["text"].as_str().unwrap_or_default().to_owned();
                if fake.turn(&text).is_none() {
                    break;
                }
            }
            _ => {}
        }
    }
}
