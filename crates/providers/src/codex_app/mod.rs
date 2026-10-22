//! The official Codex CLI's app-server protocol (`codex app-server`,
//! JSON-RPC over stdio; verified against codex-cli 0.163.0) — ADR-0047.
//!
//! * [`conn`] — one app-server process: requests, notifications, the CLI's
//!   own requests (approvals);
//! * [`map`] — pure mapping onto Orkestr types;
//! * [`run_agent`] — one coding-agent run with per-command approvals
//!   bridged to Orkestr (`approvalPolicy: untrusted`): every
//!   `item/commandExecution/requestApproval` / `item/fileChange/requestApproval`
//!   becomes a [`CodingAgentEvent::PermissionRequest`] decided by the agent
//!   runtime (permission engine + Dangerous Action Guard + approvals).
//!
//! A model may carry a reasoning effort as `<model>@<effort>`
//! (`gpt-5-codex@high`); `default` = the CLI's default model.

pub mod conn;
pub mod map;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::adapters::claude_chat::ProcSpawner;
use crate::adapters::cli;
use crate::http::{drive, Emitter};
use crate::types::{CliPermissionReply, CodingAgentEvent, CodingAgentStream, ProviderError};
use conn::{AppServer, Inbound, THREAD_TIMEOUT};

/// Splits `<model>@<effort>`; `default` / empty = no model.
#[must_use]
pub fn split_model(model: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) else { return (None, None) };
    let (m, effort) = match m.split_once('@') {
        Some((m, e)) if is_token(e) => (m, Some(e.to_owned())),
        _ => (m, None),
    };
    let m = (m != "default" && is_token(m)).then(|| m.to_owned());
    (m, effort)
}

/// A model / effort / thread id token (no spaces, no option-like prefix).
#[must_use]
pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
}

/// `thread/start` / `thread/resume` params.
#[must_use]
pub fn thread_params(
    cwd: &str,
    model: Option<&str>,
    sandbox: &str,
    approval_policy: &str,
    developer_instructions: Option<&str>,
    ephemeral: bool,
) -> Value {
    let mut p = json!({
        "cwd": cwd,
        "sandbox": sandbox,
        "approvalPolicy": approval_policy,
        "serviceName": "orkestr",
    });
    if let Some(m) = model {
        p["model"] = json!(m);
    }
    if let Some(d) = developer_instructions.filter(|d| !d.trim().is_empty()) {
        p["developerInstructions"] = json!(d);
    }
    if ephemeral {
        p["ephemeral"] = json!(true);
    }
    p
}

/// `turn/start` params.
#[must_use]
pub fn turn_params(thread_id: &str, text: &str, effort: Option<&str>) -> Value {
    let mut p = json!({ "threadId": thread_id, "input": [{ "type": "text", "text": text }] });
    if let Some(e) = effort {
        p["effort"] = json!(e);
    }
    p
}

/// One coding-agent run.
#[derive(Debug, Clone)]
pub struct AgentRun {
    /// Working directory (the agent's worktree).
    pub cwd: PathBuf,
    /// `<model>@<effort>`.
    pub model: Option<String>,
    /// Developer instructions (Orkestr's system prompt).
    pub instructions: Option<String>,
    /// The task prompt.
    pub prompt: String,
    /// Resume this thread.
    pub resume: Option<String>,
    /// `workspace-write` (writes allowed) or `read-only`.
    pub sandbox: &'static str,
    /// `untrusted` (bridged: ask for everything) or `never`.
    pub approval_policy: &'static str,
}

/// Runs one turn on a fresh app-server process; approvals go out as
/// `PermissionRequest`s and come back on `replies` (`None` = decline all).
pub fn run_agent(
    spawner: Arc<dyn ProcSpawner>,
    program: PathBuf,
    run: AgentRun,
    replies: Option<tokio::sync::mpsc::UnboundedReceiver<CliPermissionReply>>,
    cancel: CancellationToken,
) -> CodingAgentStream {
    drive(cancel, move |em: Emitter<CodingAgentEvent>| async move {
        let mut replies = replies;
        let (srv, mut rx) = AppServer::start(&*spawner, &program, &run.cwd).await?;
        let (model, effort) = split_model(run.model.as_deref());
        let cwd = run.cwd.to_string_lossy().into_owned();
        let params =
            thread_params(&cwd, model.as_deref(), run.sandbox, run.approval_policy, run.instructions.as_deref(), false);
        let resumed = run.resume.as_deref().filter(|r| is_token(r));
        let started = match resumed {
            Some(id) => {
                let mut p = params;
                p["threadId"] = json!(id);
                p["excludeTurns"] = json!(true);
                srv.request_with("thread/resume", p, THREAD_TIMEOUT).await?
            }
            None => srv.request_with("thread/start", params, THREAD_TIMEOUT).await?,
        };
        let thread_id = started
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Server("Codex did not return a thread id".into()))?
            .to_owned();
        em.emit(CodingAgentEvent::Session { session_id: thread_id.clone() }).await?;
        srv.request("turn/start", turn_params(&thread_id, &run.prompt, effort.as_deref())).await?;
        let mut parser = map::TurnParser::default();
        loop {
            let next = tokio::time::timeout(cli::CLI_IDLE_TIMEOUT, rx.recv()).await.map_err(|_| {
                ProviderError::Server(format!("Codex produced no output for {}s", cli::CLI_IDLE_TIMEOUT.as_secs()))
            })?;
            match next {
                None | Some(Inbound::Closed) => {
                    em.emit(parser.died()).await?;
                    return Ok(());
                }
                Some(Inbound::Notification { method, params }) => {
                    let other = params.get("threadId").and_then(Value::as_str).is_some_and(|t| t != thread_id);
                    if other {
                        continue;
                    }
                    for ev in parser.notification(&method, &params) {
                        em.emit(ev).await?;
                    }
                    if parser.terminal {
                        return Ok(());
                    }
                }
                Some(Inbound::Request { id, method, params }) => {
                    let Some(a) = map::approval(&method, &params, &parser.file_items, &parser.command_items) else {
                        srv.respond_error(&id, "not supported by Orkestr");
                        continue;
                    };
                    let rid = match &id {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let Some(rep) = replies.as_mut() else {
                        srv.respond(&id, map::approval_answer(false, false, false));
                        continue;
                    };
                    em.emit(CodingAgentEvent::PermissionRequest {
                        request_id: rid.clone(),
                        tool: a.tool.clone(),
                        tool_use_id: Some(a.item_id.clone()),
                        input: a.input.clone(),
                        description: a.reason.clone(),
                        blocked_path: None,
                    })
                    .await?;
                    let reply = loop {
                        match rep.recv().await {
                            Some(r) if r.request_id == rid => break Some(r),
                            Some(_) => {}
                            None => break None,
                        }
                    };
                    let allow = reply.as_ref().is_some_and(|r| r.allow);
                    srv.respond(&id, map::approval_answer(allow, false, reply.is_none()));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::conn::fake::FakeCodex;
    use super::*;
    use crate::types::{ChatEvent, PermissionBridge};
    use futures::StreamExt;

    /// A fake app-server whose turn runs one command that needs approval.
    fn script() -> conn::fake::Script {
        Arc::new(|method, p, id| match method {
            "thread/start" | "thread/resume" => {
                let tid = p.get("threadId").and_then(Value::as_str).unwrap_or("th-1").to_owned();
                vec![json!({"id": id, "result": {"thread": {"id": tid}}})]
            }
            "turn/start" => vec![
                json!({"id": id, "result": {"turn": {"id": "u1", "items": [], "status": "inProgress"}}}),
                json!({"method":"item/started","params":{"threadId":"th-1","turnId":"u1","item":{"id":"c1","type":"commandExecution","command":"cargo test","cwd":"/wt","commandActions":[],"status":"inProgress"}}}),
                json!({"id": "approval-7", "method":"item/commandExecution/requestApproval","params":{"threadId":"th-1","turnId":"u1","itemId":"c1","command":"cargo test","cwd":"/wt","startedAtMs":1}}),
            ],
            "response" => {
                let accepted = p.pointer("/result/decision") == Some(&json!("accept"));
                vec![
                    json!({"method":"item/completed","params":{"threadId":"th-1","turnId":"u1","item":{"id":"c1","type":"commandExecution","command":"cargo test","cwd":"/wt","commandActions":[],"status": if accepted {"completed"} else {"declined"},"exitCode": if accepted {json!(0)} else {Value::Null},"aggregatedOutput":"ok"}}}),
                    json!({"method":"item/completed","params":{"threadId":"th-1","turnId":"u1","item":{"id":"m","type":"agentMessage","text":"done"}}}),
                    json!({"method":"turn/completed","params":{"threadId":"th-1","turn":{"id":"u1","items":[],"status":"completed"}}}),
                ]
            }
            _ => vec![],
        })
    }

    fn run(resume: Option<&str>) -> AgentRun {
        AgentRun {
            cwd: std::env::temp_dir(),
            model: Some("gpt-5-codex@high".into()),
            instructions: Some("You are Backend.".into()),
            prompt: "fix it".into(),
            resume: resume.map(str::to_owned),
            sandbox: "workspace-write",
            approval_policy: "untrusted",
        }
    }

    async fn go(allow: bool, resume: Option<&str>) -> (Vec<CodingAgentEvent>, Arc<FakeCodex>) {
        let fake = FakeCodex::new(script());
        let (bridge, tx) = PermissionBridge::new();
        let mut stream = run_agent(
            Arc::clone(&fake) as Arc<dyn ProcSpawner>,
            PathBuf::from("codex"),
            run(resume),
            bridge.take(),
            CancellationToken::new(),
        );
        let mut out = Vec::new();
        while let Some(ev) = stream.next().await {
            if let CodingAgentEvent::PermissionRequest { request_id, tool, input, .. } = &ev {
                assert_eq!((tool.as_str(), input["command"].as_str()), ("Bash", Some("cargo test")));
                tx.send(CliPermissionReply { request_id: request_id.clone(), allow, message: None }).unwrap();
            }
            out.push(ev);
        }
        (out, fake)
    }

    #[tokio::test]
    async fn approvals_are_bridged_and_answered() {
        let (ev, fake) = go(true, None).await;
        assert_eq!(ev[0], CodingAgentEvent::Session { session_id: "th-1".into() });
        assert!(ev.iter().any(|e| matches!(e, CodingAgentEvent::PermissionRequest { request_id, .. } if request_id == "approval-7")));
        assert!(ev.iter().any(|e| matches!(e, CodingAgentEvent::ToolFinished { ok: true, .. })));
        assert!(matches!(ev.last(), Some(CodingAgentEvent::Chat(ChatEvent::Done { .. }))));
        let seen = fake.seen.lock().clone();
        let start = seen.iter().find(|v| v["method"] == "thread/start").unwrap();
        assert_eq!(start["params"]["approvalPolicy"], "untrusted");
        assert_eq!(start["params"]["sandbox"], "workspace-write");
        assert_eq!(start["params"]["model"], "gpt-5-codex");
        assert_eq!(start["params"]["developerInstructions"], "You are Backend.");
        let turn = seen.iter().find(|v| v["method"] == "turn/start").unwrap();
        assert_eq!(turn["params"]["effort"], "high");
        let answer = seen.iter().find(|v| v.get("result").is_some()).unwrap();
        assert_eq!(answer["id"], "approval-7");
        assert_eq!(answer["result"]["decision"], "accept");
    }

    #[tokio::test]
    async fn a_denied_command_is_declined_and_resume_uses_the_thread() {
        let (ev, fake) = go(false, Some("th-1")).await;
        assert!(ev.iter().any(|e| matches!(e, CodingAgentEvent::ToolFinished { ok: false, .. })));
        let seen = fake.seen.lock().clone();
        assert!(seen.iter().any(|v| v["method"] == "thread/resume" && v["params"]["threadId"] == "th-1"));
        assert_eq!(seen.iter().find(|v| v.get("result").is_some()).unwrap()["result"]["decision"], "decline");
    }

    #[test]
    fn models_split_effort() {
        assert_eq!(split_model(Some("gpt-5-codex@high")), (Some("gpt-5-codex".into()), Some("high".into())));
        assert_eq!(split_model(Some("default")), (None, None));
        assert_eq!(split_model(Some("default@low")), (None, Some("low".into())));
        assert_eq!(split_model(Some("--evil")), (None, None));
        assert_eq!(split_model(None), (None, None));
    }
}
