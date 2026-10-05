//! Supervision of official CLI coding agents (Claude Code, Codex) —
//! agent-runtime-spec §4.1, provider-adapter-spec §6.3.
//!
//! The CLI owns its inner tool loop, so Orkestr cannot route each of its tool
//! calls through the permission engine. What the runtime **does** enforce:
//!
//! * the CLI runs with `cwd` = the agent's worktree and a static tool policy
//!   derived from the template (writes only when the template has write tools
//!   and the worktree is writable; shell (`Bash`) only in Full Auto with
//!   `run_command` allowed; web tools always disallowed);
//! * Codex (sandbox-only gating) needs a one-time explicit user approval per
//!   run unless the project is in Full Auto;
//! * every file change the CLI reports is confined to the worktree — a change
//!   outside it cancels the run immediately (process tree killed);
//! * cancellation (stop / emergency / wall-clock) kills the CLI process tree;
//! * usage is recorded; the turn limit is passed as `--max-turns`.
//!
//! Known limits (honest): per-command approvals are **not** possible for CLI
//! agents in Phase 1 (no permission-prompt bridge yet); file writes inside the
//! worktree are not individually approved; the post-hoc budget check cannot
//! stop spend that already happened inside one CLI run.

use std::time::Instant;

use futures::StreamExt;
use orkestr_domain::{
    now, AgentState, ApprovalMode, Capability, ProviderKind, RiskLevel, ToolCall, ToolCallId, ToolCallStatus,
};
use orkestr_protocol::event::{
    EventKind, FileChangeActor, FileChangeKind, FileChangedPayload, MessageDeltaPayload, MessageRole,
    ToolCallFinishedPayload, ToolCallStartedPayload,
};
use orkestr_protocol::NewEvent;
use orkestr_providers::{
    ChatEvent, CliToolPolicy, CodingAgentEvent, CodingAgentRequest, FileChangeKind as CliChange, Usage,
};
use orkestr_security::ApprovalRequest;

use crate::run::{CliCalls, Outcome, Run, SubmitResult, SubmitStatus};
use crate::tools::{one_line, truncate_middle};

/// Extracts the final fenced JSON result block (`summary` required).
#[must_use]
pub fn parse_result_block(text: &str) -> Option<SubmitResult> {
    let mut best = None;
    let mut rest = text;
    while let Some(start) = rest.find("```json") {
        let after = &rest[start + 7..];
        let Some(end) = after.find("```") else { break };
        if let Ok(r) = serde_json::from_str::<SubmitResult>(after[..end].trim()) {
            best = Some(r);
        }
        rest = &after[end + 3..];
    }
    best
}

fn cli_capability(tool: &str) -> Capability {
    match tool.to_ascii_lowercase().as_str() {
        "edit" | "write" | "multiedit" | "notebookedit" | "apply_patch" | "file_change" => Capability::Write,
        "bash" | "shell" | "exec" | "command_execution" | "local_shell" => Capability::Terminal,
        "webfetch" | "websearch" | "web_search" => Capability::Network,
        _ => Capability::Read,
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_cli(run: &mut Run) -> Outcome {
    let Some(model) = run.agent.model.clone() else {
        return Outcome::Failed("no model selected".into());
    };
    let Some(info) = run.deps.models.provider(model.provider_id) else {
        return Outcome::Failed(format!("provider {} is not configured", model.provider_id));
    };
    let Some(adapter) = run.deps.models.adapter(model.provider_id) else {
        return Outcome::Failed(format!("provider {} is not available", model.provider_id));
    };
    let Some(root) = run.env.root.clone() else {
        return Outcome::Failed("CLI coding agents need a workspace (worktree policy None is not supported)".into());
    };
    let writes = run.tools.iter().any(|t| t == "write_file" || t == "edit_file");
    let shell = run.tools.iter().any(|t| t == "run_command") && run.approval_mode == ApprovalMode::FullAuto;

    // Codex gates only at launch (its own sandbox): explicit opt-in per run.
    if info.kind == ProviderKind::Codex && run.approval_mode != ApprovalMode::FullAuto {
        let mut req = ApprovalRequest::new(
            Capability::Terminal,
            format!("Let {} run Codex in its sandbox", run.agent.name),
            "Codex runs commands inside its own workspace-write sandbox; Orkestr cannot approve each command. \
             Approve to let it work in the agent worktree."
                .to_owned(),
            RiskLevel::Medium,
        );
        req.project_id = Some(run.agent.project_id);
        req.task_id = Some(run.task.id);
        req.agent_id = Some(run.agent.id);
        req.command_preview = Some(format!("codex exec --sandbox workspace-write --cd {}", root.display()));
        let approval = match run.deps.security.approvals.request(req).await {
            Ok(a) => a,
            Err(e) => return Outcome::Failed(format!("could not request the Codex approval: {e}")),
        };
        let mut placeholder = cli_call(run, "codex_sandbox", Capability::Terminal);
        let _ = run.store.save_tool_call(&placeholder, None, None).await;
        run.set_state(AgentState::Waiting, Some("Waiting for approval to start Codex".into())).await;
        let status =
            run.deps.security.approvals.wait(approval.id, run.deps.config.approval_timeout, &run.control.cancel).await;
        if run.control.cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        placeholder.finished_at = Some(now());
        if status.ok() != Some(orkestr_domain::ApprovalStatus::Approved) {
            placeholder.status = ToolCallStatus::Denied;
            let _ = run.store.save_tool_call(&placeholder, Some("not approved"), None).await;
            return Outcome::Failed("the user did not approve running Codex in its sandbox".into());
        }
        placeholder.status = ToolCallStatus::Succeeded;
        let _ = run.store.save_tool_call(&placeholder, Some("approved"), None).await;
    }

    let mut allowed = vec!["Read".to_owned(), "Glob".into(), "Grep".into(), "LS".into()];
    let mut disallowed = vec!["WebFetch".to_owned(), "WebSearch".into()];
    if writes {
        allowed.extend(["Edit".to_owned(), "Write".into(), "MultiEdit".into()]);
    }
    if shell {
        allowed.push("Bash".into());
    } else {
        disallowed.push("Bash".into());
    }
    let previous = run.store.latest_task_handoff(run.task.id).await.ok().flatten().map(|h| crate::handoff::render(&h));
    let prompt =
        crate::prompt::initial_message(&run.deps, &run.template, &run.project, &run.task, Some(&root), previous).await;
    let mut prompt = crate::prompt::with_briefing(prompt, run.briefing.as_deref());
    prompt.push_str(
        "\nWhen you are completely done, end your final message with a fenced ```json block containing \
         {\"status\": \"done|partial|blocked|failed\", \"summary\": \"…\", \"files_changed\": [], \"tests_run\": \
         [{\"command\": \"…\", \"passed\": true}], \"open_issues\": []}.\n",
    );
    let system = crate::prompt::system_prompt(
        &run.template,
        &run.project,
        &run.task,
        &run.agent,
        &run.tools,
        &run.deps.config.ui_language.get(),
    );
    run.agent.execution_style = Some("cli".into());
    let req = CodingAgentRequest {
        prompt: prompt.clone(),
        system_append: Some(system),
        cwd: root.to_string_lossy().into_owned(),
        model: Some(model.model.clone()),
        max_turns: Some(run.agent.max_turns),
        resume_session: None,
        tool_policy: CliToolPolicy { allowed_tools: allowed, disallowed_tools: disallowed, allow_writes: writes },
    };
    let user_id = uuid::Uuid::now_v7().to_string();
    let user_text = orkestr_security::redact::redact(&truncate_middle(&prompt, 64_000));
    let user_msg =
        run.store.add_message(&user_id, run.agent.id, Some(run.execution.id), MessageRole::User, &user_text, &[]);
    if let Err(e) = user_msg.await {
        tracing::warn!(error = %e, "failed to persist CLI prompt");
    }
    if let Err(o) = run.check_budget(u64::try_from(prompt.len() / 4).unwrap_or(0)).await {
        return o;
    }
    run.set_state(AgentState::Thinking, Some(format!("Running {:?} CLI", info.kind))).await;
    let started = Instant::now();
    let mut stream = match adapter.run_coding_agent(req, run.control.cancel.child_token()).await {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(format!("could not start the CLI agent: {e}")),
    };
    let message_id = uuid::Uuid::now_v7().to_string();
    let mut text = String::new();
    let mut usage: Option<Usage> = None;
    let mut calls: CliCalls = CliCalls::new();
    let mut outcome: Option<Outcome> = None;
    while let Some(ev) = stream.next().await {
        match ev {
            CodingAgentEvent::Chat(ChatEvent::TextDelta(t)) => {
                text.push_str(&t);
                run.deps
                    .publish(
                        NewEvent::new(EventKind::AgentMessage(MessageDeltaPayload {
                            agent_id: Some(run.agent.id),
                            message_id: message_id.clone(),
                            role: MessageRole::Assistant,
                            delta: orkestr_security::redact::redact(&t),
                            done: false,
                        }))
                        .agent(run.agent.id),
                    )
                    .await;
            }
            CodingAgentEvent::Chat(ChatEvent::Usage(u)) => usage = Some(u),
            CodingAgentEvent::Chat(ChatEvent::Done { .. }) => break,
            CodingAgentEvent::Chat(ChatEvent::Error(e)) => {
                outcome = Some(if run.control.cancel.is_cancelled() {
                    Outcome::Cancelled
                } else {
                    Outcome::Failed(format!("CLI agent failed: {}", orkestr_security::redact::redact(&e.to_string())))
                });
                break;
            }
            CodingAgentEvent::Chat(_) => {}
            CodingAgentEvent::ToolStarted { call_id, tool, summary } => {
                let cap = cli_capability(&tool);
                let mut call = cli_call(run, &tool, cap);
                call.input = serde_json::json!({ "summary": orkestr_security::redact::redact(&summary) });
                let _ = run.store.save_tool_call(&call, None, None).await;
                run.deps
                    .publish(
                        NewEvent::new(EventKind::ToolCallStarted(ToolCallStartedPayload { tool_call: call.clone() }))
                            .agent(run.agent.id),
                    )
                    .await;
                let state = match cap {
                    Capability::Write => AgentState::Coding,
                    Capability::Terminal
                        if crate::run::classify_command(&summary) == crate::tools::ToolClass::Verify =>
                    {
                        AgentState::Testing
                    }
                    Capability::Terminal => AgentState::Coding,
                    _ => AgentState::Thinking,
                };
                run.set_state(state, Some(format!("{tool} {}", one_line(&summary, 80)))).await;
                calls.insert(call_id, (call, Instant::now()));
            }
            CodingAgentEvent::ToolFinished { call_id, ok, summary } => {
                if let Some((mut call, t0)) = calls.remove(&call_id) {
                    call.status = if ok { ToolCallStatus::Succeeded } else { ToolCallStatus::Failed };
                    call.output_summary = Some(orkestr_security::redact::redact(&one_line(&summary, 200)));
                    call.finished_at = Some(now());
                    let ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let _ = run
                        .store
                        .save_tool_call(&call, Some(&orkestr_security::redact::redact(&summary)), Some(ms))
                        .await;
                    run.deps
                        .publish(
                            NewEvent::new(EventKind::ToolCallFinished(ToolCallFinishedPayload {
                                tool_call_id: call.id,
                                status: call.status,
                                output_summary: call.output_summary.clone(),
                                duration_ms: ms,
                            }))
                            .agent(run.agent.id),
                        )
                        .await;
                    run.progress.lock().step(format!("{} {}", call.tool, one_line(&summary, 120)));
                }
                run.set_state(AgentState::Thinking, None).await;
            }
            CodingAgentEvent::FileChanged { path, change } => {
                match orkestr_security::confine_relative(&root, &path) {
                    Ok(rel) => {
                        run.progress.lock().file(&rel);
                        let kind = match change {
                            CliChange::Added => FileChangeKind::Created,
                            CliChange::Modified => FileChangeKind::Modified,
                            CliChange::Deleted => FileChangeKind::Deleted,
                        };
                        run.deps
                            .publish(
                                NewEvent::new(EventKind::FileChanged(FileChangedPayload {
                                    path: rel,
                                    old_path: None,
                                    change: kind,
                                    actor: FileChangeActor::Agent,
                                    agent_id: Some(run.agent.id),
                                }))
                                .project(run.agent.project_id)
                                .agent(run.agent.id),
                            )
                            .await;
                    }
                    Err(e) => {
                        // Confinement violation: kill the CLI immediately.
                        run.control.cancel.cancel();
                        outcome = Some(Outcome::Failed(format!(
                            "the CLI agent changed a file outside its worktree ({e}); the run was stopped"
                        )));
                        break;
                    }
                }
            }
            CodingAgentEvent::Session { session_id } => {
                run.agent.cli_session = Some(session_id);
            }
        }
    }
    // Close dangling tool calls.
    for (_, (mut call, t0)) in calls.drain() {
        call.status = ToolCallStatus::Cancelled;
        call.finished_at = Some(now());
        let ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = run.store.save_tool_call(&call, None, Some(ms)).await;
    }
    run.deps
        .publish(
            NewEvent::new(EventKind::AgentMessage(MessageDeltaPayload {
                agent_id: Some(run.agent.id),
                message_id: message_id.clone(),
                role: MessageRole::Assistant,
                delta: String::new(),
                done: true,
            }))
            .agent(run.agent.id),
        )
        .await;
    let latency = u64::try_from(started.elapsed().as_millis()).ok();
    let u = usage.unwrap_or_else(|| Usage::estimate(prompt.len(), text.len()));
    run.record_usage(&model, u, latency, outcome.is_none()).await;
    run.agent.turns_used = run.agent.turns_used.saturating_add(1);
    if let Err(e) = run
        .store
        .add_message(
            &message_id,
            run.agent.id,
            Some(run.execution.id),
            MessageRole::Assistant,
            &orkestr_security::redact::redact(&truncate_middle(&text, 64_000)),
            &[],
        )
        .await
    {
        tracing::warn!(error = %e, "failed to persist CLI output");
    }
    run.persist().await;
    if let Some(o) = outcome {
        return o;
    }
    if run.control.cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    let mut result = parse_result_block(&text).unwrap_or_else(|| SubmitResult {
        status: SubmitStatus::Partial,
        summary: if text.trim().is_empty() {
            "The CLI agent finished without output.".into()
        } else {
            truncate_middle(text.trim(), 2_000)
        },
        files_changed: Vec::new(),
        tests_run: Vec::new(),
        open_issues: vec!["the CLI agent did not return a structured result block".into()],
    });
    if let Some(max) = run.agent.budget.as_ref().and_then(|b| b.max_cost_usd) {
        if run.agent.usage.cost_usd > max {
            result.open_issues.push(format!(
                "cost budget exceeded during the CLI run (${:.4} of ${max:.2}) — CLI spend can only be checked afterwards",
                run.agent.usage.cost_usd
            ));
        }
    }
    Outcome::Submitted(result)
}

fn cli_call(run: &Run, tool: &str, capability: Capability) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(),
        execution_id: run.execution.id,
        agent_id: run.agent.id,
        task_id: Some(run.task.id),
        tool: tool.chars().take(64).collect(),
        capability,
        input: serde_json::Value::Null,
        output_summary: None,
        status: ToolCallStatus::Running,
        approval_id: None,
        started_at: now(),
        finished_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_last_result_block() {
        let text = "work…\n```json\n{\"summary\": \"first\"}\n```\nmore\n```json\n{\"status\": \"partial\", \"summary\": \"final\", \"open_issues\": [\"x\"]}\n```\n";
        let r = parse_result_block(text).unwrap();
        assert_eq!(r.summary, "final");
        assert_eq!(r.status, SubmitStatus::Partial);
        assert_eq!(r.open_issues, vec!["x".to_owned()]);
        assert!(parse_result_block("no block").is_none());
        assert!(parse_result_block("```json\n{not json}\n```").is_none());
    }

    #[test]
    fn cli_tools_map_to_capabilities() {
        assert_eq!(cli_capability("Edit"), Capability::Write);
        assert_eq!(cli_capability("Bash"), Capability::Terminal);
        assert_eq!(cli_capability("Grep"), Capability::Read);
        assert_eq!(cli_capability("WebFetch"), Capability::Network);
    }
}
