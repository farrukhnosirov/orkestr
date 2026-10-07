//! Manager conversations (§5): one reply = a bounded tool loop over the
//! Manager model. Text deltas stream as `ManagerMessage` events; the stored
//! reply keeps only visible text plus tool events (name + one-line result) —
//! never hidden reasoning (§1.14, §21).

use std::sync::Arc;

use orkestr_domain::{ConversationId, ManagerConfig, Project, UsageSummary};
use orkestr_protocol::event::{EventKind, MessageDeltaPayload, MessageRole};
use orkestr_protocol::methods::manager::{
    ChatAttachment, ManagerChatMessage, ManagerChatParams, ManagerChatResult, ManagerMessageRole,
    ManagerRequestOptions, ManagerToolEvent, MAX_CHAT_TEXT,
};
use orkestr_protocol::NewEvent;
use orkestr_providers::{ChatMessage, ChatRequest, ToolChoice};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::service::{summarize_messages, Inner, Manager, MessageExtras};
use crate::tools::{tool_specs, TurnCtx};
use crate::util::{cap, manager_chain, one_line, record_usage, redact, stream_chat, DeltaSink};

/// The Manager's system prompt (`language`: the user's UI language, e.g. `ru`).
#[must_use]
pub fn system_prompt(
    project: &Project,
    cfg: &ManagerConfig,
    policy: orkestr_domain::ApprovalMode,
    language: &str,
) -> String {
    let mut s = format!(
        "You are the AI Manager of Orkestr, an AI Development OS. You coordinate specialised AI agents for the project \"{}\".\n\n\
         How you work:\n\
         - You see the real state only through your tools (tasks, agents, providers, approvals, costs, git, memory, snapshots). Never invent state — call a tool.\n\
         - For any request to build, change or fix something, call propose_plan with the goal. The user reviews the plan card and approves it; then Orkestr creates the tasks and runs agents, each in its own git worktree.\n\
         - For questions, read the state with the tools and answer concisely in markdown.\n\
         - Ask the user only when a decision is needed (request_approval).\n\
         - Record important architecture or product decisions with record_decision.\n\
         - Keep answers short and concrete. Do not include hidden reasoning.\n\n\
         Approval policy: {policy:?}{}.\n",
        project.name,
        match policy {
            orkestr_domain::ApprovalMode::Ask => " — creating tasks and starting agents needs the user's approval",
            orkestr_domain::ApprovalMode::SemiAuto => " — plans need the user's approval; approved work runs automatically",
            orkestr_domain::ApprovalMode::FullAuto => " — plans are approved and run automatically",
        }
    );
    s.push_str(crate::ui_tools::PROMPT_LINE);
    s.push_str(&format!("Max parallel agents: {}.\n", cfg.max_parallel_agents));
    s.push('\n');
    s.push_str(&orkestr_agent_runtime::reply_language_instruction(language));
    if !cfg.system_instructions.trim().is_empty() {
        s.push_str(&format!("\nInstructions from the user:\n{}\n", cfg.system_instructions.trim()));
    }
    s
}

fn attachment_block(a: &ChatAttachment) -> String {
    let range = match (a.start_line, a.end_line) {
        (Some(s), Some(e)) => format!(" (lines {s}-{e})"),
        (Some(s), None) => format!(" (from line {s})"),
        _ => String::new(),
    };
    format!(
        "\n\n[Attached from {}{range}]\n```{}\n{}\n```",
        a.path,
        a.language.as_deref().unwrap_or(""),
        cap(&a.text, MAX_CHAT_TEXT)
    )
}

/// Conversation → model messages (older part summarised).
fn history_messages(history: &[ManagerChatMessage], keep: usize) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    let (older, recent) =
        if history.len() > keep { history.split_at(history.len() - keep) } else { (&[][..], history) };
    if !older.is_empty() {
        out.push(ChatMessage::user(format!("Summary of the earlier conversation:\n{}", summarize_messages(older))));
        out.push(ChatMessage::assistant("Understood."));
    }
    // An Orkestr event stored while the user's latest message was being
    // sent (e.g. "run completed" racing a new request) must not end up after
    // that message: the turn answers the user's request, so such events are
    // moved just before it.
    let mut ordered: Vec<&ManagerChatMessage> = recent.iter().collect();
    if let Some(last_user) = ordered.iter().rposition(|m| m.role == ManagerMessageRole::User) {
        let tail = ordered.split_off(last_user + 1);
        let (events, rest): (Vec<_>, Vec<_>) =
            tail.into_iter().partition(|m| m.role == ManagerMessageRole::SystemEvent);
        let user = ordered.pop();
        ordered.extend(events);
        ordered.extend(user);
        ordered.extend(rest);
    }
    for m in ordered {
        match m.role {
            ManagerMessageRole::User => out.push(ChatMessage::user(m.text.clone())),
            ManagerMessageRole::Manager => {
                if !m.text.trim().is_empty() {
                    out.push(ChatMessage::assistant(m.text.clone()));
                }
            }
            ManagerMessageRole::SystemEvent => out.push(ChatMessage::user(format!("[Orkestr event] {}", m.text))),
        }
    }
    out
}

impl Manager {
    /// Sends a user message; the reply is produced in the background.
    pub async fn chat(&self, p: ManagerChatParams) -> OrchResult<ManagerChatResult> {
        let (result, _handle) = self.start_chat(p).await?;
        Ok(result)
    }

    /// Like [`Manager::chat`] but waits for the stored reply (tests, CLI).
    pub async fn chat_and_wait(&self, p: ManagerChatParams) -> OrchResult<(ManagerChatResult, ManagerChatMessage)> {
        let (result, handle) = self.start_chat(p).await?;
        let reply = handle.await.map_err(|e| OrchestratorError::Internal(format!("manager turn panicked: {e}")))?;
        Ok((result, reply))
    }

    async fn start_chat(
        &self,
        p: ManagerChatParams,
    ) -> OrchResult<(ManagerChatResult, JoinHandle<ManagerChatMessage>)> {
        let text = p.text.trim().to_owned();
        if text.is_empty() || text.chars().count() > MAX_CHAT_TEXT {
            return Err(OrchestratorError::Invalid(format!("the message must be 1..={MAX_CHAT_TEXT} characters")));
        }
        if let Some(a) = &p.attachment {
            if a.path.trim().is_empty() || a.text.chars().count() > MAX_CHAT_TEXT {
                return Err(OrchestratorError::Invalid(
                    "the attachment needs a path and at most 20000 characters".into(),
                ));
            }
        }
        let project = self.inner.project(p.project_id).await?;
        // Child of the emergency registry: Stop All AI / Stop project cancel
        // the turn (streaming, further provider rounds, tool calls). While
        // Stop All is active the turn itself refuses with a notice.
        let cancel =
            crate::util::emergency_token(&self.inner.deps, project.id).unwrap_or_else(|_| CancellationToken::new());
        {
            let mut busy = self.inner.busy.lock();
            if busy.contains_key(&project.id) {
                return Err(OrchestratorError::Conflict("the Manager is still answering the previous message".into()));
            }
            busy.insert(project.id, cancel.clone());
        }
        let (conv, _) = match self.inner.store.active_conversation(project.id).await {
            Ok(c) => c,
            Err(e) => {
                self.inner.busy.lock().remove(&project.id);
                return Err(e);
            }
        };
        let shown = match &p.attachment {
            Some(a) => format!("{text}{}", attachment_block(a)),
            None => text.clone(),
        };
        let user_message = match self
            .inner
            .add_message(project.id, Some(conv), ManagerMessageRole::User, &shown, MessageExtras::default())
            .await
        {
            Ok(m) => m,
            Err(e) => {
                self.inner.busy.lock().remove(&project.id);
                return Err(e);
            }
        };
        let reply_id = uuid::Uuid::now_v7().to_string();
        let inner = Arc::clone(&self.inner);
        let options = p.options.unwrap_or_default();
        let rid = reply_id.clone();
        let handle = tokio::spawn(async move {
            let pid = project.id;
            let reply = inner.turn(project, conv, rid, options, cancel).await;
            inner.busy.lock().remove(&pid);
            // Final "not busy" notice for clients that track the flag.
            inner
                .deps
                .publish(
                    NewEvent::new(EventKind::ManagerMessageAdded(
                        orkestr_protocol::event::ManagerMessageAddedPayload { message: reply.clone(), busy: false },
                    ))
                    .project(pid),
                )
                .await;
            reply
        });
        Ok((ManagerChatResult { user_message, reply_message_id: reply_id }, handle))
    }
}

impl Inner {
    /// One Manager reply. Always returns a stored message (errors become a
    /// system event).
    async fn turn(
        self: &Arc<Self>,
        project: Project,
        conv: ConversationId,
        reply_id: String,
        options: ManagerRequestOptions,
        cancel: CancellationToken,
    ) -> ManagerChatMessage {
        match self.turn_inner(&project, conv, &reply_id, options, &cancel).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(project_id = %project.id, error = %e, "manager turn failed");
                self.finish_stream(project.id, &reply_id).await;
                let text = match &e {
                    OrchestratorError::NoModel(m) => format!("The AI Manager has no model: {m}"),
                    OrchestratorError::Conflict(m) if m == "cancelled" => "The Manager's reply was cancelled.".into(),
                    other => format!("The Manager could not answer: {other}"),
                };
                let extras = MessageExtras { id: Some(reply_id.clone()), ..MessageExtras::default() };
                match self.add_message(project.id, Some(conv), ManagerMessageRole::SystemEvent, &text, extras).await {
                    Ok(m) => m,
                    Err(e2) => {
                        tracing::error!(error = %e2, "could not store the manager error message");
                        ManagerChatMessage {
                            id: reply_id,
                            project_id: project.id,
                            conversation_id: conv,
                            role: ManagerMessageRole::SystemEvent,
                            text,
                            tool_events: Vec::new(),
                            plan_id: None,
                            run_id: None,
                            model: None,
                            usage: None,
                            created_at: orkestr_domain::now(),
                        }
                    }
                }
            }
        }
    }

    async fn finish_stream(&self, project: orkestr_domain::ProjectId, id: &str) {
        self.deps
            .publish(
                NewEvent::new(EventKind::ManagerMessage(MessageDeltaPayload {
                    agent_id: None,
                    message_id: id.to_owned(),
                    role: MessageRole::Assistant,
                    delta: String::new(),
                    done: true,
                }))
                .project(project),
            )
            .await;
    }

    #[allow(clippy::too_many_lines)]
    async fn turn_inner(
        self: &Arc<Self>,
        project: &Project,
        conv: ConversationId,
        reply_id: &str,
        options: ManagerRequestOptions,
        cancel: &CancellationToken,
    ) -> OrchResult<ManagerChatMessage> {
        if self.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden(
                "Stop All AI is active — reset the emergency controls first".into(),
            ));
        }
        let cfg = self.config().await;
        let policy = self.policy(&cfg);
        let chain = manager_chain(&self.deps, project, &cfg, options.model.as_ref(), false).await?;
        let history = self.store.messages(conv, 400).await?;
        let mut messages = history_messages(&history, self.deps.config.history_messages);
        if let Some(lang) = options.language.as_deref() {
            self.deps.config.ui_language.set(lang);
        }
        let system = system_prompt(project, &cfg, policy, &self.deps.config.ui_language.get());
        let specs = tool_specs();
        let ctx = TurnCtx { project: project.clone(), conversation: conv, options, policy, cancel: cancel.clone() };
        let sink = DeltaSink { project_id: project.id, message_id: reply_id.to_owned() };
        let mut text = String::new();
        let mut events: Vec<ManagerToolEvent> = Vec::new();
        let mut usage = UsageSummary::default();
        let mut plan_id = None;
        let mut model = chain.first().cloned();
        let mut final_answer: Option<String> = None;
        let max_turns = self.deps.config.chat_max_turns.max(1);
        let mut exhausted = true;
        for _ in 0..max_turns {
            if self.deps.security.emergency.flags().all_stopped {
                return Err(OrchestratorError::Forbidden("Stop All AI is active".into()));
            }
            if cancel.is_cancelled() {
                return Err(OrchestratorError::Conflict("cancelled".into()));
            }
            let mut req = ChatRequest::new(chain[0].model.clone(), messages.clone());
            req.system = Some(system.clone());
            req.tools = specs.clone();
            req.tool_choice = ToolChoice::Auto;
            req.max_output_tokens = Some(self.deps.config.max_output_tokens);
            req.reasoning = Some(cfg.reasoning);
            let out = stream_chat(&self.deps, chain.clone(), req, Some(&sink), cancel).await?;
            let used = out.model.clone().unwrap_or_else(|| chain[0].clone());
            model = Some(used.clone());
            for s in &out.switches {
                events.push(ManagerToolEvent { name: "provider_fallback".into(), summary: s.clone(), is_error: false });
            }
            let u = out.usage.unwrap_or_else(|| {
                orkestr_providers::Usage::estimate(messages.iter().map(|m| m.content.len()).sum(), out.text.len())
            });
            usage.add(&record_usage(&self.deps, project.id, None, &used, u, out.first_ms, true).await);
            if !out.text.trim().is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(out.text.trim());
            }
            messages.push(ChatMessage::assistant_tool_calls(out.text.clone(), out.calls.clone()));
            if out.calls.is_empty() {
                exhausted = false;
                break;
            }
            for call in &out.calls {
                let r = self.execute_tool(&ctx, &call.name, &call.arguments).await;
                if let Some(u) = r.usage {
                    usage.add(&u);
                }
                if r.plan_id.is_some() {
                    plan_id = r.plan_id;
                }
                events.push(ManagerToolEvent {
                    name: call.name.clone(),
                    summary: redact(&one_line(&r.summary, 200)),
                    is_error: r.is_error,
                });
                if let Some(answer) = r.final_answer {
                    final_answer = Some(answer);
                }
                let content = cap(&redact(&r.content), self.deps.config.tool_result_chars);
                messages.push(ChatMessage::tool_result(call.id.clone(), content, r.is_error));
            }
            if final_answer.is_some() {
                exhausted = false;
                break;
            }
        }
        if let Some(answer) = final_answer {
            // Stream the final answer like model text.
            let delta = if text.is_empty() { answer.clone() } else { format!("\n\n{answer}") };
            self.deps
                .publish(
                    NewEvent::new(EventKind::ManagerMessage(MessageDeltaPayload {
                        agent_id: None,
                        message_id: reply_id.to_owned(),
                        role: MessageRole::Assistant,
                        delta: redact(&delta),
                        done: false,
                    }))
                    .project(project.id),
                )
                .await;
            text.push_str(&delta);
        }
        if exhausted {
            text.push_str(&format!("\n\n_(Stopped after {max_turns} tool rounds — ask me to continue if needed.)_"));
        }
        if text.trim().is_empty() {
            text = if events.is_empty() {
                "(no answer)".into()
            } else {
                format!(
                    "Done: {}.",
                    events.iter().map(|e| format!("{} → {}", e.name, e.summary)).collect::<Vec<_>>().join("; ")
                )
            };
        }
        self.finish_stream(project.id, reply_id).await;
        let msg = self
            .add_message(
                project.id,
                Some(conv),
                ManagerMessageRole::Manager,
                &text,
                MessageExtras {
                    tool_events: events,
                    plan_id,
                    run_id: None,
                    model,
                    usage: Some(usage),
                    id: Some(reply_id.to_owned()),
                },
            )
            .await?;
        tracing::debug!(project_id = %project.id, "manager turn finished");
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> ManagerChatMessage {
        serde_json::from_value(serde_json::json!({
            "id": text,
            "projectId": "0192a000-0000-7000-8000-000000000001",
            "conversationId": "0192a000-0000-7000-8000-000000000002",
            "role": role,
            "text": text,
            "toolEvents": [],
            "createdAt": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn events_racing_the_latest_request_are_moved_before_it() {
        let history = vec![
            msg("User", "first"),
            msg("Manager", "ok"),
            msg("SystemEvent", "older event"),
            msg("User", "Create a greeter module"),
            msg("SystemEvent", "Run completed"),
        ];
        let out = history_messages(&history, 50);
        let texts: Vec<&str> = out.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "first",
                "ok",
                "[Orkestr event] older event",
                "[Orkestr event] Run completed",
                "Create a greeter module"
            ]
        );
    }

    #[test]
    fn attachment_is_rendered_as_a_fenced_block() {
        let a = ChatAttachment {
            path: "src/a.rs".into(),
            start_line: Some(3),
            end_line: Some(5),
            language: Some("rust".into()),
            text: "fn a() {}".into(),
        };
        let b = attachment_block(&a);
        assert!(b.starts_with("\n\n[Attached from src/a.rs (lines 3-5)]"));
        assert!(b.contains("```rust\nfn a() {}\n```"));
    }
}
