//! Supervision of official CLI coding agents (Claude Code, Codex) —
//! agent-runtime-spec §4.1, provider-adapter-spec §6.3.
//!
//! **Claude Code** runs with the stdio permission bridge
//! (`--permission-prompt-tool stdio`, permission mode `default`): every tool
//! call the CLI would prompt for (edits, shell, web, MCP) arrives as a
//! `PermissionRequest` and is decided exactly like a native tool call —
//! template tools → worktree confinement → `PermissionEngine` (with the
//! `DangerousActionGuard`) → `ApprovalService` when asked → Before
//! checkpoint before the first change. Reads inside the worktree are
//! pre-approved; a call the CLI ran without asking (its own allow rules) is
//! classified when it finishes and a violation stops the run. A template
//! mirroring a Claude Code sub-agent runs it with `--agent` (+ `--agents`).
//!
//! **Codex** (sandbox-only gating) keeps the static model:
//!
//! * the CLI runs with `cwd` = the agent's worktree and a static tool policy
//!   derived from the template; it needs a one-time explicit user approval
//!   per run unless the project is in Full Auto;
//! * the task's automatic Before checkpoint is taken before the CLI starts
//!   (ADR-0012 §4 / ADR-0021) — no checkpoint, no start;
//! * every command the CLI reports is classified after the fact by the
//!   dangerous-action guard; Dangerous / Forbidden stops the run at once
//!   (process tree killed) and is audited (`agent.cliDangerousCommand`);
//! * every file change the CLI reports is confined to the worktree — a change
//!   outside it cancels the run immediately (process tree killed).
//!
//! Both: cancellation (stop / emergency / wall-clock) kills the CLI process
//! tree; usage is recorded; the turn limit is passed as `--max-turns`.
//!
//! Known limits (honest): Codex commands are not individually approved; the
//! post-hoc budget check cannot stop spend that already happened inside one
//! CLI run.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use futures::StreamExt;
use orkestr_domain::{
    now, Actor, AgentState, AgentTemplate, ApprovalMode, Capability, ProviderKind, RiskLevel, TemplateSource, ToolCall,
    ToolCallId, ToolCallStatus,
};
use orkestr_protocol::event::{
    EventKind, FileChangeActor, FileChangeKind, FileChangedPayload, MessageDeltaPayload, MessageRole,
    ToolCallFinishedPayload, ToolCallStartedPayload,
};
use orkestr_protocol::NewEvent;
use orkestr_providers::{
    ChatEvent, CliPermissionReply, CliSubagent, CliToolPolicy, CodingAgentEvent, CodingAgentRequest,
    FileChangeKind as CliChange, PermissionBridge, Usage,
};
use orkestr_security::{ApprovalRequest, Decision, PermissionEngine, Resource, ShellKind};
use serde_json::Value;

use crate::run::{ApprovalOutcome, CliCalls, Outcome, Run, SubmitResult, SubmitStatus};
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
    // Claude Code: every tool call the CLI would prompt for is decided by
    // Orkestr (permission engine + guard + approvals) over the stdio
    // permission bridge — per command, not only after the fact.
    let bridged = info.kind == ProviderKind::ClaudeCode;
    let shell = shell || (bridged && run.tools.iter().any(|t| t == "run_command"));

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

    let policy = if bridged { cli_tool_policy_bridged() } else { cli_tool_policy(writes) };
    // The CLI owns its tool loop: its changes (including Codex sandbox
    // commands) need the task's Before checkpoint first (ADR-0012 §4,
    // ADR-0021), or `rollbackTask` has nothing to restore.
    if writes || shell || info.kind == ProviderKind::Codex {
        if let Err(e) = run.checkpoint().await {
            return Outcome::Failed(format!(
                "could not take the checkpoint before starting the CLI agent ({e}); the run was not started"
            ));
        }
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
    let (bridge, replies) = if bridged {
        let (b, tx) = PermissionBridge::new();
        (Some(b), Some(tx))
    } else {
        (None, None)
    };
    let subagent = if bridged { claude_subagent(&run.template) } else { None };
    let req = CodingAgentRequest {
        prompt: prompt.clone(),
        system_append: Some(system),
        cwd: root.to_string_lossy().into_owned(),
        model: Some(model.model.clone()),
        max_turns: Some(run.agent.max_turns),
        resume_session: None,
        tool_policy: policy,
        subagent,
        permission_bridge: bridge,
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
    let mut live = orkestr_security::StreamRedactor::new();
    let mut usage: Option<Usage> = None;
    let mut calls: CliCalls = CliCalls::new();
    let mut outcome: Option<Outcome> = None;
    // Bridged runs: tool calls the CLI ran without asking (its own allow
    // rules) are checked when they finish; denied calls end as `Denied`.
    let mut unchecked: HashMap<String, (String, String)> = HashMap::new();
    let mut denied: HashSet<String> = HashSet::new();
    while let Some(ev) = stream.next().await {
        match ev {
            CodingAgentEvent::Chat(ChatEvent::TextDelta(t)) => {
                text.push_str(&t);
                // Redacted over the stream: a secret split across chunks
                // would pass a per-chunk check.
                let delta = live.push(&t);
                if !delta.is_empty() {
                    run.deps
                        .publish(
                            NewEvent::new(EventKind::AgentMessage(MessageDeltaPayload {
                                agent_id: Some(run.agent.id),
                                message_id: message_id.clone(),
                                role: MessageRole::Assistant,
                                delta,
                                done: false,
                            }))
                            .agent(run.agent.id),
                        )
                        .await;
                }
            }
            CodingAgentEvent::Chat(ChatEvent::Usage(u)) => usage = Some(u),
            CodingAgentEvent::Chat(ChatEvent::Done { .. }) => break,
            CodingAgentEvent::Chat(ChatEvent::Error(e)) => {
                run.cli_error = Some(e.clone());
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
                // A file tool outside the worktree / on a secret / in .git:
                // stopped at once and audited (the CLI's scoped allow and
                // deny rules should have refused it).
                if bridged {
                    unchecked.insert(call_id.clone(), (tool.clone(), summary.clone()));
                }
                if let Some(why) = (!bridged).then(|| cli_path_violation(&root, &tool, &summary)).flatten() {
                    run.control.cancel.cancel();
                    let msg = format!("the CLI agent's {why}; the run was stopped for review");
                    let msg = orkestr_security::redact::redact(&msg);
                    call.status = ToolCallStatus::Denied;
                    call.output_summary = Some(one_line(&msg, 200));
                    call.finished_at = Some(now());
                    let _ = run.store.save_tool_call(&call, Some(&msg), Some(0)).await;
                    run.deps
                        .publish(
                            NewEvent::new(EventKind::ToolCallFinished(ToolCallFinishedPayload {
                                tool_call_id: call.id,
                                status: call.status,
                                output_summary: call.output_summary.clone(),
                                duration_ms: 0,
                            }))
                            .agent(run.agent.id),
                        )
                        .await;
                    run.deps
                        .security
                        .audit
                        .record(
                            orkestr_security::NewAudit::new(
                                orkestr_domain::Actor::Agent(run.agent.id),
                                "agent.cliPathViolation",
                                one_line(&msg, 300),
                                orkestr_domain::AuditOutcome::Denied,
                            )
                            .capability(cap)
                            .project(Some(run.agent.project_id))
                            .details(serde_json::json!({ "taskId": run.task.id, "tool": tool })),
                        )
                        .await;
                    outcome = Some(Outcome::Failed(msg));
                    break;
                }
                // Post-hoc classification (ADR-0012): the CLI already runs the
                // command, but a Dangerous / Forbidden one stops the run at
                // once (process tree killed) and is audited as a finding —
                // Full Auto never overrides the hard policies (§23).
                if cap == Capability::Terminal && !bridged {
                    let class = run.deps.security.guard.classify_in(
                        &summary,
                        orkestr_security::ShellKind::Bash,
                        Some(root.as_path()),
                    );
                    if !class.is_safe() {
                        run.control.cancel.cancel();
                        let reason = class.reason().unwrap_or("dangerous command").to_owned();
                        let level = if class.is_forbidden() { "forbidden" } else { "dangerous" };
                        let msg = format!(
                            "the CLI agent ran a {level} command ({}: {reason}); the run was stopped for review",
                            one_line(&orkestr_security::redact::redact(&summary), 120)
                        );
                        call.status = ToolCallStatus::Denied;
                        call.output_summary = Some(one_line(&msg, 200));
                        call.finished_at = Some(now());
                        let _ = run.store.save_tool_call(&call, Some(&msg), Some(0)).await;
                        run.deps
                            .publish(
                                NewEvent::new(EventKind::ToolCallFinished(ToolCallFinishedPayload {
                                    tool_call_id: call.id,
                                    status: call.status,
                                    output_summary: call.output_summary.clone(),
                                    duration_ms: 0,
                                }))
                                .agent(run.agent.id),
                            )
                            .await;
                        run.deps
                            .security
                            .audit
                            .record(
                                orkestr_security::NewAudit::new(
                                    orkestr_domain::Actor::Agent(run.agent.id),
                                    "agent.cliDangerousCommand",
                                    orkestr_security::redact::redact(&one_line(&summary, 300)),
                                    orkestr_domain::AuditOutcome::Denied,
                                )
                                .capability(Capability::Terminal)
                                .project(Some(run.agent.project_id))
                                .details(serde_json::json!({
                                    "taskId": run.task.id,
                                    "tool": tool,
                                    "level": level,
                                    "category": class.category().map(|c| format!("{c:?}")),
                                    "reason": reason,
                                })),
                            )
                            .await;
                        outcome = Some(Outcome::Failed(msg));
                        break;
                    }
                }
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
                // Backstop for a bridged run: a call that never reached
                // Orkestr's bridge (allowed by the CLI's own rules) is
                // classified now; a violation stops the run.
                if let Some((tool, input_summary)) = unchecked.remove(&call_id).filter(|_| ok) {
                    if let Some(msg) = posthoc_violation(run, &root, &tool, &input_summary).await {
                        run.control.cancel.cancel();
                        if let Some((mut call, _)) = calls.remove(&call_id) {
                            call.status = ToolCallStatus::Denied;
                            call.output_summary = Some(one_line(&msg, 200));
                            call.finished_at = Some(now());
                            let _ = run.store.save_tool_call(&call, Some(&msg), Some(0)).await;
                        }
                        outcome = Some(Outcome::Failed(msg));
                        break;
                    }
                }
                if let Some((mut call, t0)) = calls.remove(&call_id) {
                    call.status = if denied.remove(&call_id) {
                        ToolCallStatus::Denied
                    } else if ok {
                        ToolCallStatus::Succeeded
                    } else {
                        ToolCallStatus::Failed
                    };
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
            CodingAgentEvent::PermissionRequest {
                request_id,
                tool,
                tool_use_id,
                input,
                description: _,
                blocked_path,
            } => {
                let key = tool_use_id.clone().unwrap_or_else(|| format!("perm-{request_id}"));
                if let Some(id) = tool_use_id.as_deref() {
                    unchecked.remove(id);
                }
                let verdict = bridge_decide(run, &root, &mut calls, &key, &tool, &input, blocked_path.as_deref()).await;
                if verdict.is_err() {
                    denied.insert(key.clone());
                }
                let reply = CliPermissionReply {
                    request_id,
                    allow: verdict.is_ok(),
                    message: verdict.err().map(|m| orkestr_security::redact::redact(&m)),
                };
                if let Some(tx) = &replies {
                    let _ = tx.send(reply);
                }
                if run.control.cancel.is_cancelled() {
                    break;
                }
                run.set_state(AgentState::Thinking, None).await;
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
    let rest = live.flush();
    if !rest.is_empty() {
        run.deps
            .publish(
                NewEvent::new(EventKind::AgentMessage(MessageDeltaPayload {
                    agent_id: Some(run.agent.id),
                    message_id: message_id.clone(),
                    role: MessageRole::Assistant,
                    delta: rest,
                    done: false,
                }))
                .agent(run.agent.id),
            )
            .await;
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

/// The Claude Code sub-agent a template mirrors (`source` = `ClaudeCode`):
/// built-ins run with `--agent <name>` only, file agents also carry their
/// definition (`--agents`) so they work in worktrees where the `.md` file is
/// not checked in.
#[must_use]
pub fn claude_subagent(t: &AgentTemplate) -> Option<CliSubagent> {
    if t.source != TemplateSource::ClaudeCode || !orkestr_providers::adapters::claude_code::is_valid_agent_name(&t.name)
    {
        return None;
    }
    let builtin = t.system_prompt.starts_with(CLAUDE_BUILTIN_MARKER);
    let definition = (!builtin).then(|| serde_json::json!({"description": t.description, "prompt": t.system_prompt}));
    Some(CliSubagent { name: t.name.clone(), definition })
}

/// First line of the system prompt of a template mirroring a built-in
/// Claude Code agent (its real prompt lives inside the CLI).
pub const CLAUDE_BUILTIN_MARKER: &str = "[Claude Code built-in agent]";

fn input_path(input: &Value) -> Option<&str> {
    ["file_path", "notebook_path", "path"].iter().find_map(|k| input.get(*k).and_then(Value::as_str))
}

/// A violation of a call that did not go through the bridge (path outside
/// the worktree / secret / `.git`, or a dangerous command): audited; the
/// message stops the run.
async fn posthoc_violation(run: &mut Run, root: &Path, tool: &str, summary: &str) -> Option<String> {
    let cap = cli_capability(tool);
    let msg = if let Some(why) = cli_path_violation(root, tool, summary) {
        format!("the CLI agent's {why}; the run was stopped for review")
    } else if cap == Capability::Terminal {
        let class = run.deps.security.guard.classify_in(summary, ShellKind::Bash, Some(root));
        if class.is_safe() {
            return None;
        }
        format!(
            "the CLI agent ran a command without asking Orkestr ({}: {}); the run was stopped for review",
            one_line(&orkestr_security::redact::redact(summary), 120),
            class.reason().unwrap_or("dangerous command")
        )
    } else {
        return None;
    };
    let msg = orkestr_security::redact::redact(&msg);
    run.deps
        .security
        .audit
        .record(
            orkestr_security::NewAudit::new(
                Actor::Agent(run.agent.id),
                "agent.cliPostHocViolation",
                one_line(&msg, 300),
                orkestr_domain::AuditOutcome::Denied,
            )
            .capability(cap)
            .project(Some(run.agent.project_id))
            .details(serde_json::json!({ "taskId": run.task.id, "tool": tool })),
        )
        .await;
    Some(msg)
}

/// Decides one `can_use_tool` request of a bridged Claude Code run exactly
/// like a native tool call: template tools → path confinement → permission
/// engine (with the dangerous-action guard) → approval when asked →
/// checkpoint before the first change. `Err` = denied, with the reason the
/// model sees.
#[allow(clippy::too_many_lines)]
async fn bridge_decide(
    run: &mut Run,
    root: &Path,
    calls: &mut CliCalls,
    key: &str,
    tool: &str,
    input: &Value,
    blocked_path: Option<&str>,
) -> Result<(), String> {
    let lower = tool.to_ascii_lowercase();
    let cap = if lower.starts_with("mcp__") { Capability::Network } else { cli_capability(tool) };
    if !calls.contains_key(key) {
        let mut call = cli_call(run, tool, cap);
        call.input = serde_json::json!({
            "summary": orkestr_security::redact::redact(&orkestr_providers::adapters::cli::summarize(input))
        });
        let _ = run.store.save_tool_call(&call, None, None).await;
        run.deps
            .publish(
                NewEvent::new(EventKind::ToolCallStarted(ToolCallStartedPayload { tool_call: call.clone() }))
                    .agent(run.agent.id),
            )
            .await;
        calls.insert(key.to_owned(), (call, Instant::now()));
    }
    let writes_ok = run.tools.iter().any(|t| t == "write_file" || t == "edit_file");
    let shell_ok = run.tools.iter().any(|t| t == "run_command");
    if cap == Capability::Write && !writes_ok {
        return Err("This Orkestr agent may not change files (its template has no write tools).".into());
    }
    if cap == Capability::Terminal && !shell_ok {
        return Err("This Orkestr agent may not run shell commands (its template has no run_command tool).".into());
    }
    let path = input_path(input).or(blocked_path);
    if let Some(p) = path.filter(|_| matches!(cap, Capability::Read | Capability::Write)) {
        if let Some(why) = cli_path_violation(root, tool, p) {
            run.deps
                .security
                .audit
                .record(
                    orkestr_security::NewAudit::new(
                        Actor::Agent(run.agent.id),
                        "agent.cliPathViolation",
                        one_line(&orkestr_security::redact::redact(&why), 300),
                        orkestr_domain::AuditOutcome::Denied,
                    )
                    .capability(cap)
                    .project(Some(run.agent.project_id))
                    .details(serde_json::json!({ "taskId": run.task.id, "tool": tool })),
                )
                .await;
            return Err(format!("Orkestr denied this: {why}. Stay inside the agent's worktree."));
        }
    }
    let str_in = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let resource = match cap {
        Capability::Read | Capability::Write => match path {
            Some(p) => Resource::Path {
                path: orkestr_security::confine_relative(root, p).unwrap_or_else(|_| p.to_owned()),
                recursive: false,
            },
            None => Resource::None,
        },
        Capability::Terminal => Resource::Command {
            command: str_in("command"),
            shell: if lower == "powershell" { ShellKind::PowerShell } else { ShellKind::Bash },
            cwd: None,
        },
        Capability::Network if lower.starts_with("mcp__") => Resource::Other { value: tool.to_owned() },
        Capability::Network => {
            let url = str_in("url");
            Resource::Url { url: if url.is_empty() { str_in("query") } else { url } }
        }
        _ => Resource::None,
    };
    // Sub-agent orchestration / to-do / plan tools have no effect of their
    // own (the sub-agent's tool calls are asked separately).
    if cap == Capability::Read && matches!(resource, Resource::None) {
        return Ok(());
    }
    let decision = run.deps.security.engine.check(&Actor::Agent(run.agent.id), cap, &resource, &run.check_ctx());
    match decision {
        Decision::Allow => {}
        Decision::Deny(r) => return Err(format!("Orkestr's security policy denied this: {}", r.message)),
        Decision::Ask(r) => {
            let what = one_line(&resource.summary(), 120);
            let mut req = ApprovalRequest::new(
                cap,
                format!("{} (Claude Code) wants to {tool} {what}", run.agent.name),
                r.message.clone(),
                r.risk,
            );
            req.project_id = Some(run.agent.project_id);
            req.task_id = Some(run.task.id);
            req.agent_id = Some(run.agent.id);
            req.danger = r.danger;
            req.command_preview = Some(truncate_middle(&orkestr_security::redact::redact(&input.to_string()), 4_000));
            let Some((mut call, t0)) = calls.remove(key) else {
                return Err("internal error: the tool call is unknown".into());
            };
            req.tool_call_id = Some(call.id);
            let approval = match run.deps.security.approvals.request(req).await {
                Ok(a) => a,
                Err(e) => {
                    calls.insert(key.to_owned(), (call, t0));
                    return Err(format!("the approval could not be requested: {e}"));
                }
            };
            let outcome =
                run.await_approval(approval.id, &mut call, format!("Waiting for approval: {tool} {what}")).await;
            call.status = ToolCallStatus::Running;
            let _ = run.store.save_tool_call(&call, None, None).await;
            calls.insert(key.to_owned(), (call, t0));
            match outcome {
                ApprovalOutcome::Approved(_) => {}
                ApprovalOutcome::Rejected(note) => {
                    return Err(format!(
                        "The user rejected this action{}",
                        note.map(|n| format!(": {n}")).unwrap_or_default()
                    ));
                }
                ApprovalOutcome::Expired => return Err("The approval expired without a decision.".into()),
                ApprovalOutcome::Cancelled => return Err("The run was stopped.".into()),
            }
        }
    }
    if matches!(cap, Capability::Write | Capability::Terminal) {
        run.checkpoint().await.map_err(|e| crate::run::no_checkpoint_message(&e))?;
    }
    Ok(())
}

/// Tool policy of a bridged Claude Code run: reads inside the worktree are
/// pre-approved (the CLI never asks for them); secret-bearing files and
/// writes into `.git` are refused by the CLI itself; everything else —
/// edits, shell, web, MCP — is asked over the bridge and decided by Orkestr.
fn cli_tool_policy_bridged() -> CliToolPolicy {
    let p = cli_tool_policy(false);
    let disallowed =
        p.disallowed_tools.into_iter().filter(|t| !matches!(t.as_str(), "WebFetch" | "WebSearch" | "Bash")).collect();
    CliToolPolicy { allowed_tools: p.allowed_tools, disallowed_tools: disallowed, allow_writes: false }
}

/// Static tool policy handed to the CLI. `Bash` is never pre-approved: the
/// CLI's shell would bypass the permission engine and the dangerous-action
/// guard entirely, and there is no `--permission-prompt-tool` bridge to
/// Orkestr's approvals yet (ADR-0012 §3). Web tools are always disallowed.
/// File tools are pre-approved only inside the CLI's working directory (the
/// agent's worktree, `./**`): a bare tool name would approve every path,
/// `~/.ssh` and Orkestr's own data directory included. Secret-bearing files
/// and `.git` are denied even there (deny rules win).
fn cli_tool_policy(writes: bool) -> CliToolPolicy {
    let mut allowed = vec!["Read(./**)".to_owned(), "Glob".into(), "Grep".into(), "LS".into()];
    if writes {
        allowed.extend(["Edit(./**)".to_owned(), "Write(./**)".into(), "MultiEdit(./**)".into()]);
    }
    let mut disallowed = vec!["WebFetch".to_owned(), "WebSearch".into(), "Bash".into()];
    for secret in SECRET_GLOBS {
        disallowed.push(format!("Read({secret})"));
    }
    for tool in ["Edit", "Write", "MultiEdit"] {
        disallowed.push(format!("{tool}(./.git/**)"));
        for secret in SECRET_GLOBS {
            disallowed.push(format!("{tool}({secret})"));
        }
    }
    CliToolPolicy { allowed_tools: allowed, disallowed_tools: disallowed, allow_writes: writes }
}

/// Secret-bearing files (the permission engine's `Secrets` names), as CLI
/// permission globs.
const SECRET_GLOBS: [&str; 8] = [
    "**/.env",
    "**/.env.*",
    "**/*.pem",
    "**/*.key",
    "**/id_rsa*",
    "**/id_ed25519*",
    "**/.git-credentials",
    "**/.netrc",
];

/// A file tool call of a CLI agent that left its worktree or touched a
/// secret-bearing file / `.git` (checked when the tool starts: the CLI's own
/// rules are the first line, this one stops a run that got past them).
fn cli_path_violation(root: &std::path::Path, tool: &str, target: &str) -> Option<String> {
    let t = tool.to_ascii_lowercase();
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let path_tool = matches!(t.as_str(), "read" | "edit" | "write" | "multiedit" | "notebookedit" | "ls");
    // Glob / Grep summaries are a path only when one was given (else the
    // pattern): only an explicit absolute / parent path is checked.
    let search_tool = matches!(t.as_str(), "glob" | "grep");
    let absolute = std::path::Path::new(target).is_absolute()
        || target.starts_with('~')
        || target.starts_with("\\\\")
        || target.starts_with('/')
        || target.contains("..");
    if !(path_tool || (search_tool && absolute)) {
        return None;
    }
    if target.starts_with('~') {
        // The CLI expands `~` to the home directory.
        return Some(format!("{tool} outside the agent's worktree ({target})"));
    }
    let rel = match orkestr_security::confine_relative(root, target) {
        Ok(rel) => rel,
        Err(_) => return Some(format!("{tool} outside the agent's worktree ({target})")),
    };
    if orkestr_security::guard::DangerousActionGuard::is_secret_file(&rel) {
        return Some(format!("{tool} of a secret-bearing file ({rel})"));
    }
    if t != "read" && t != "ls" && crate::tools::touches_git_dir(&rel) {
        return Some(format!("{tool} inside .git ({rel})"));
    }
    None
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
    fn bash_is_never_pre_approved_for_cli_agents() {
        for writes in [false, true] {
            let p = cli_tool_policy(writes);
            assert!(!p.allowed_tools.iter().any(|t| t == "Bash"), "{p:?}");
            assert!(p.disallowed_tools.iter().any(|t| t == "Bash"));
            assert!(p.disallowed_tools.iter().any(|t| t == "WebFetch"));
            assert_eq!(p.allow_writes, writes);
            assert_eq!(p.allowed_tools.iter().any(|t| t == "Edit(./**)"), writes);
            // No bare file-tool rule: it would approve every path on disk.
            assert!(!p.allowed_tools.iter().any(|t| t == "Read" || t == "Edit" || t == "Write"), "{p:?}");
            assert!(p.allowed_tools.iter().any(|t| t == "Read(./**)"));
            assert!(p.disallowed_tools.iter().any(|t| t == "Read(**/.env)"), "{p:?}");
            assert!(p.disallowed_tools.iter().any(|t| t == "Write(./.git/**)"), "{p:?}");
        }
    }

    #[test]
    fn cli_file_tools_outside_the_worktree_or_on_secrets_stop_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wt");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let inside = root.join("src/main.rs").to_string_lossy().into_owned();
        assert_eq!(cli_path_violation(&root, "Read", &inside), None);
        assert_eq!(cli_path_violation(&root, "Edit", "src/main.rs"), None);
        assert_eq!(cli_path_violation(&root, "Grep", "fn main"), None, "a pattern is not a path");
        assert_eq!(cli_path_violation(&root, "Glob", "**/*.rs"), None);
        assert_eq!(cli_path_violation(&root, "Read", ".env.example"), None);
        let outside = dir.path().join("other/secret.txt").to_string_lossy().into_owned();
        assert!(cli_path_violation(&root, "Read", &outside).is_some());
        assert!(cli_path_violation(&root, "Grep", &outside).is_some(), "an explicit search path is checked");
        assert!(cli_path_violation(&root, "Read", "~/.ssh/id_rsa").is_some());
        assert!(cli_path_violation(&root, "Read", "~/notes.txt").is_some());
        assert!(cli_path_violation(&root, "Read", "../other/x").is_some());
        assert!(cli_path_violation(&root, "Read", ".env").is_some());
        assert!(cli_path_violation(&root, "Read", "config/server.pem").is_some());
        assert!(cli_path_violation(&root, "Write", ".git/hooks/pre-commit").is_some());
        assert!(cli_path_violation(&root, "Bash", "cat ~/.ssh/id_rsa").is_none(), "shell commands go to the guard");
    }

    #[test]
    fn cli_tools_map_to_capabilities() {
        assert_eq!(cli_capability("Edit"), Capability::Write);
        assert_eq!(cli_capability("Bash"), Capability::Terminal);
        assert_eq!(cli_capability("Grep"), Capability::Read);
        assert_eq!(cli_capability("WebFetch"), Capability::Network);
    }
}
