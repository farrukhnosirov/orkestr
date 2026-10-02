//! One agent run: an isolated tokio task with its own cancellation token
//! (registered in the `EmergencyController`), its own worktree, conversation
//! state and budget/turn/time limits (§64 "no fake agents").
//!
//! Chat-style providers go through the tool loop below; official CLI coding
//! agents (Claude Code / Codex) are supervised by [`crate::cli`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_domain::{
    now, Actor, AgentInstance, AgentState, AgentTemplate, ApprovalId, ApprovalMode, ApprovalStatus, Capability,
    Execution, ExecutionStatus, ModelRef, Project, QualityGate, RiskLevel, SnapshotReason, Task, TaskActor, TaskResult,
    TaskSnapshotPhase, TaskStatus, ToolCall, ToolCallId, ToolCallStatus, UsageRecord, UsageRecordId, UsageSummary,
};
use orkestr_protocol::event::{
    AgentStartedPayload, AgentStateChangedPayload, AgentStoppedPayload, AgentUpdatedPayload, BudgetThresholdPayload,
    EventKind, MessageDeltaPayload, MessageRole, StopReason as WireStopReason, ToolCallFinishedPayload,
    ToolCallStartedPayload,
};
use orkestr_protocol::methods::agents::{AgentStopMode, HandoffDecision, HandoffReason, HandoffTestState};
use orkestr_protocol::NewEvent;
use orkestr_providers::{
    ChatEvent, ChatMessage, ChatRequest, HandoffContext, HandoffHook, ToolCallPart, ToolChoice, ToolSpec, Usage,
};
use orkestr_security::{ApprovalRequest, CheckContext, Decision, PermissionEngine};
use orkestr_task_engine::AgentRunUpdate;
use orkestr_terminal::{RunRequest, ShellSpec, TerminalError, TerminalOwner};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::deps::RuntimeDeps;
use crate::handoff::{self, HandoffInput, Progress};
use crate::state::check_transition;
use crate::store::AgentStore;
use crate::tools::{self, descriptor, one_line, truncate_middle, ToolClass, ToolEnv, ToolOutput};

/// Why a run was asked to stop (absent ⇒ emergency stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCause {
    /// `agents.stop`.
    User(AgentStopMode),
    /// Core shutdown.
    Shutdown,
    /// Wall-clock limit.
    Timeout,
}

/// Shared control block of a live run.
#[derive(Debug)]
pub struct RunControl {
    /// The agent token (child of the project token in the emergency registry).
    pub cancel: CancellationToken,
    cause: Mutex<Option<StopCause>>,
    /// Pause flag (`agents.pause` / `agents.resume`).
    pub paused: tokio::sync::watch::Sender<bool>,
    /// Becomes true when the run has fully finished.
    pub finished: tokio::sync::watch::Sender<bool>,
}

impl RunControl {
    /// New control block.
    #[must_use]
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            cause: Mutex::new(None),
            paused: tokio::sync::watch::channel(false).0,
            finished: tokio::sync::watch::channel(false).0,
        }
    }

    /// Records the cause (first wins) and cancels the run.
    pub fn request_stop(&self, cause: StopCause) {
        {
            let mut c = self.cause.lock();
            if c.is_none() {
                *c = Some(cause);
            }
        }
        self.cancel.cancel();
    }

    /// The recorded cause.
    #[must_use]
    pub fn cause(&self) -> Option<StopCause> {
        *self.cause.lock()
    }
}

/// `submit_result` arguments.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SubmitResult {
    /// Status.
    #[serde(default)]
    pub status: SubmitStatus,
    /// Summary.
    pub summary: String,
    /// Files changed.
    #[serde(default)]
    pub files_changed: Vec<String>,
    /// Tests run.
    #[serde(default)]
    pub tests_run: Vec<TestRun>,
    /// Open issues.
    #[serde(default)]
    pub open_issues: Vec<String>,
}

/// Status of a submitted result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmitStatus {
    /// Finished.
    #[default]
    Done,
    /// Partially done (needs review).
    Partial,
    /// Blocked.
    Blocked,
    /// Failed.
    Failed,
}

/// A test run reported in `submit_result`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TestRun {
    /// Command.
    pub command: String,
    /// Passed.
    #[serde(default)]
    pub passed: Option<bool>,
    /// Summary.
    #[serde(default)]
    pub summary: Option<String>,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// `submit_result` (explicit or implicit).
    Submitted(SubmitResult),
    /// Unrecoverable error.
    Failed(String),
    /// Turn / budget / time limit.
    Limit {
        /// `max_turns`, `budget_exceeded`, `time_limit`.
        code: &'static str,
        /// Human message.
        message: String,
    },
    /// The token was cancelled (see [`RunControl::cause`]).
    Cancelled,
}

enum ToolExec {
    Result { content: String, is_error: bool },
    Submitted(SubmitResult),
    Cancelled,
}

enum ApprovalOutcome {
    Approved(Option<String>),
    Rejected(Option<String>),
    Expired,
    Cancelled,
}

/// Classifies a command line: build/test/lint/typecheck commands are `Verify`.
#[must_use]
pub fn classify_command(command: &str) -> ToolClass {
    let c = command.to_ascii_lowercase();
    let verify = [
        "cargo test",
        "cargo build",
        "cargo check",
        "cargo clippy",
        "cargo fmt --check",
        "cargo nextest",
        "npm test",
        "npm run test",
        "npm run build",
        "npm run lint",
        "npm run typecheck",
        "pnpm test",
        "yarn test",
        "npx vitest",
        "npx jest",
        "vitest",
        "jest",
        "tsc",
        "eslint",
        "pytest",
        "python -m pytest",
        "go test",
        "go build",
        "go vet",
        "dotnet test",
        "dotnet build",
        "mvn test",
        "gradle test",
        "./gradlew test",
        "make test",
        "ruff",
        "mypy",
    ];
    if verify.iter().any(|v| c.trim_start().starts_with(v)) {
        ToolClass::Verify
    } else {
        ToolClass::Mutate
    }
}

fn redact(text: &str) -> String {
    orkestr_security::redact::redact(text)
}

/// A running agent.
pub struct Run {
    /// Services.
    pub deps: Arc<RuntimeDeps>,
    /// Persistence.
    pub store: AgentStore,
    /// Control block.
    pub control: Arc<RunControl>,
    /// The instance (owned by this run while it is live).
    pub agent: AgentInstance,
    /// Template snapshot.
    pub template: AgentTemplate,
    /// Project.
    pub project: Project,
    /// Task (refreshed after transitions).
    pub task: Task,
    /// Models to try (head first).
    pub chain: Vec<ModelRef>,
    /// The head is an official CLI coding agent.
    pub cli: bool,
    /// Tool environment.
    pub env: ToolEnv,
    /// Effective approval mode at spawn.
    pub approval_mode: ApprovalMode,
    /// Tools offered to the model.
    pub tools: Vec<String>,
    /// Progress (shared with the fallback hook).
    pub progress: Arc<Mutex<Progress>>,
    /// Execution record.
    pub execution: Execution,
    /// Usage of this run.
    pub run_usage: UsageSummary,
    checkpoint_taken: bool,
    budget_warned: bool,
    budget_note: Option<String>,
    started: Instant,
}

impl Run {
    /// Creates a run (nothing happens until [`Run::drive`]).
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        deps: Arc<RuntimeDeps>,
        store: AgentStore,
        control: Arc<RunControl>,
        agent: AgentInstance,
        template: AgentTemplate,
        project: Project,
        task: Task,
        chain: Vec<ModelRef>,
        cli: bool,
        env: ToolEnv,
        approval_mode: ApprovalMode,
        tools: Vec<String>,
    ) -> Self {
        let execution = Execution {
            id: env.execution_id,
            task_id: task.id,
            agent_id: agent.id,
            attempt: task.attempts.saturating_add(1),
            status: ExecutionStatus::Running,
            model: agent.model.clone(),
            usage: UsageSummary::default(),
            summary: None,
            error: None,
            started_at: now(),
            finished_at: None,
        };
        Self {
            deps,
            store,
            control,
            agent,
            template,
            project,
            task,
            chain,
            cli,
            env,
            approval_mode,
            tools,
            progress: Arc::new(Mutex::new(Progress::default())),
            execution,
            run_usage: UsageSummary::default(),
            checkpoint_taken: false,
            budget_warned: false,
            budget_note: None,
            started: Instant::now(),
        }
    }

    // ------------------------------------------------------------------
    // state / events
    // ------------------------------------------------------------------

    async fn publish(&self, kind: EventKind) {
        let ev = NewEvent::new(kind).project(self.agent.project_id).task(self.task.id).agent(self.agent.id);
        self.deps.publish(ev).await;
    }

    /// Persists the agent and publishes `AgentUpdated`.
    pub async fn persist(&mut self) {
        self.agent.last_activity_at = Some(now());
        if let Err(e) = self.store.save(&self.agent).await {
            tracing::error!(agent_id = %self.agent.id, error = %e, "failed to persist agent");
        }
        self.publish(EventKind::AgentUpdated(AgentUpdatedPayload { agent: self.agent.clone() })).await;
    }

    /// Transitions the agent state (validated by the state machine).
    pub async fn set_state(&mut self, to: AgentState, activity: Option<String>) {
        let from = self.agent.state;
        if let Err(e) = check_transition(from, to) {
            tracing::warn!(agent_id = %self.agent.id, error = %e, "ignored invalid agent transition");
            return;
        }
        self.agent.activity.clone_from(&activity);
        if from == to {
            self.persist().await;
            return;
        }
        self.agent.state = to;
        self.persist().await;
        self.publish(EventKind::AgentStateChanged(AgentStateChangedPayload {
            agent_id: self.agent.id,
            from,
            to,
            activity,
        }))
        .await;
        tracing::debug!(agent_id = %self.agent.id, ?from, ?to, "agent state");
    }

    /// Reaches `to` from any state, passing through `Thinking` when the
    /// direct edge does not exist (e.g. `Waiting → Completed`).
    async fn force_state(&mut self, to: AgentState, activity: Option<String>) {
        if !crate::state::can_transition(self.agent.state, to) {
            if self.agent.state == AgentState::Idle {
                self.set_state(AgentState::Listening, None).await;
            }
            if crate::state::can_transition(self.agent.state, AgentState::Thinking) {
                self.set_state(AgentState::Thinking, None).await;
            }
        }
        self.set_state(to, activity).await;
    }

    async fn task_to(&mut self, to: TaskStatus, reason: &str) -> bool {
        if self.task.status == to {
            return true;
        }
        match self
            .deps
            .tasks
            .transition(self.task.id, to, TaskActor::Agent(self.agent.id), Some(reason.to_owned()), None)
            .await
        {
            Ok(change) => {
                self.task = change.task;
                true
            }
            Err(e) => {
                tracing::warn!(task_id = %self.task.id, ?to, error = %e, "agent could not transition its task");
                if let Ok(t) = self.deps.tasks.get(self.task.id).await {
                    self.task = t;
                }
                false
            }
        }
    }

    async fn message(&mut self, role: MessageRole, text: &str, calls: &[ToolCallId]) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let text = redact(&truncate_middle(text, 64_000));
        if let Err(e) = self.store.add_message(&id, self.agent.id, Some(self.execution.id), role, &text, calls).await {
            tracing::warn!(error = %e, "failed to persist agent message");
        }
        id
    }

    /// A system line in the transcript, also streamed.
    async fn note(&mut self, text: &str) {
        let id = self.message(MessageRole::System, text, &[]).await;
        self.publish(EventKind::AgentMessage(MessageDeltaPayload {
            agent_id: Some(self.agent.id),
            message_id: id,
            role: MessageRole::System,
            delta: redact(text),
            done: true,
        }))
        .await;
    }

    fn check_ctx(&self) -> CheckContext {
        CheckContext {
            project_id: Some(self.agent.project_id),
            agent_id: Some(self.agent.id),
            task_id: Some(self.task.id),
            approval_mode: self.approval_mode,
            worktree_root: self.env.root.clone(),
            protected_environment: false,
        }
    }

    // ------------------------------------------------------------------
    // lifecycle
    // ------------------------------------------------------------------

    /// Runs to completion (start → loop → finish).
    pub async fn drive(mut self) {
        let control = Arc::clone(&self.control);
        let limit = self
            .agent
            .budget
            .as_ref()
            .and_then(|b| b.max_duration_secs)
            .map_or(self.deps.config.default_max_wall_clock, Duration::from_secs);
        let outcome = {
            let fut = self.work();
            tokio::pin!(fut);
            tokio::select! {
                o = &mut fut => o,
                () = tokio::time::sleep(limit) => {
                    control.request_stop(StopCause::Timeout);
                    fut.await
                }
            }
        };
        self.finish(outcome).await;
        let _ = control.finished.send(true);
    }

    async fn work(&mut self) -> Outcome {
        if let Err(o) = self.start().await {
            return o;
        }
        if self.cli {
            crate::cli::run_cli(self).await
        } else {
            self.chat_loop().await
        }
    }

    async fn start(&mut self) -> Result<(), Outcome> {
        self.agent.started_at = Some(now());
        self.agent.execution_id = Some(self.execution.id);
        self.set_state(AgentState::Listening, Some("Reading the task".into())).await;
        self.publish(EventKind::AgentStarted(AgentStartedPayload {
            agent_id: self.agent.id,
            task_id: Some(self.task.id),
        }))
        .await;
        if let Err(e) = self.store.save_execution(&self.execution).await {
            return Err(Outcome::Failed(format!("could not record the execution: {e}")));
        }
        if self.task.status != TaskStatus::Running && !self.task_to(TaskStatus::Running, "agent_started").await {
            return Err(Outcome::Failed(format!(
                "the task could not be started (it is {:?}); it must be Ready",
                self.task.status
            )));
        }
        let model = self.agent.model.as_ref().map_or_else(|| "?".to_owned(), |m| m.model.clone());
        let place = self.agent.worktree.as_ref().map_or_else(
            || "no workspace".to_owned(),
            |w| {
                if w.branch.is_empty() {
                    w.path.clone()
                } else {
                    format!("{} ({})", w.path, w.branch)
                }
            },
        );
        self.note(&format!("Started on “{}” with {model} — {place}.", self.task.title)).await;
        Ok(())
    }

    /// Waits while paused. Returns false when cancelled meanwhile.
    pub async fn wait_if_paused(&mut self) -> bool {
        let mut rx = self.control.paused.subscribe();
        if !*rx.borrow() {
            return !self.control.cancel.is_cancelled();
        }
        self.set_state(AgentState::Waiting, Some("Paused by the user".into())).await;
        self.task_to(TaskStatus::Waiting, "agent_paused").await;
        let cancelled = loop {
            if !*rx.borrow() {
                break false;
            }
            tokio::select! {
                r = rx.changed() => if r.is_err() { break false },
                () = self.control.cancel.cancelled() => break true,
            }
        };
        if cancelled {
            return false;
        }
        self.task_to(TaskStatus::Running, "agent_resumed").await;
        self.set_state(AgentState::Thinking, Some("Resumed".into())).await;
        true
    }

    // ------------------------------------------------------------------
    // budget / usage
    // ------------------------------------------------------------------

    fn head_model(&self) -> Option<ModelRef> {
        self.agent.model.clone().or_else(|| self.chain.first().cloned())
    }

    /// Pre-flight budget check (agent limits + cost engine budgets).
    pub async fn check_budget(&mut self, est_input_tokens: u64) -> Result<(), Outcome> {
        let usage = self.agent.usage;
        let model = self.head_model();
        let estimate = model.as_ref().and_then(|m| orkestr_providers::pricing_for(&m.model)).map_or(0.0, |p| {
            orkestr_providers::pricing::preflight_estimate(
                &p,
                est_input_tokens,
                u64::from(self.deps.config.max_output_tokens),
            )
        });
        if let Some(b) = self.agent.budget.clone() {
            if let Some(max) = b.max_tokens {
                if usage.total_tokens() >= max {
                    return Err(Outcome::Limit {
                        code: "budget_exceeded",
                        message: format!("token budget exhausted ({} of {max} tokens used)", usage.total_tokens()),
                    });
                }
                self.maybe_warn("tokens", usage.total_tokens() as f64, max as f64, b.warn_at).await;
            }
            if let Some(max) = b.max_cost_usd {
                if usage.cost_usd >= max || (usage.cost_usd + estimate > max && estimate > 0.0) {
                    return Err(Outcome::Limit {
                        code: "budget_exceeded",
                        message: format!("cost budget exhausted (${:.4} spent of ${max:.2})", usage.cost_usd),
                    });
                }
                self.maybe_warn("cost", usage.cost_usd, max, b.warn_at).await;
            }
        }
        let ctx = orkestr_cost::BudgetContext {
            project_id: Some(self.agent.project_id),
            task_id: Some(self.task.id),
            provider_id: model.as_ref().map(|m| m.provider_id),
            agent_id: Some(self.agent.id),
        };
        match self.deps.cost.check(ctx, estimate).await {
            Ok(r) if r.outcome == orkestr_protocol::methods::cost::BudgetOutcome::Exceeded => {
                return Err(Outcome::Limit {
                    code: "budget_exceeded",
                    message: "a project/global spending budget would be exceeded".into(),
                });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "budget check failed; continuing"),
        }
        Ok(())
    }

    async fn maybe_warn(&mut self, what: &str, spent: f64, limit: f64, warn_at: f64) {
        if self.budget_warned || limit <= 0.0 {
            return;
        }
        let fraction = spent / limit;
        if fraction >= warn_at {
            self.budget_warned = true;
            let left = self.agent.max_turns.saturating_sub(self.agent.turns_used);
            self.budget_note = Some(format!(
                "Budget notice: {:.0}% of your {what} budget is used and {left} turns remain — prioritise finishing and call submit_result.",
                fraction * 100.0
            ));
            if what == "cost" {
                self.publish(EventKind::BudgetThreshold(BudgetThresholdPayload {
                    scope: format!("agent:{}", self.agent.id),
                    spent_usd: spent,
                    limit_usd: limit,
                    fraction,
                }))
                .await;
            }
        }
    }

    /// Records one call's usage: cost engine + agent + run totals.
    pub async fn record_usage(&mut self, model: &ModelRef, usage: Usage, latency_ms: Option<u64>, success: bool) {
        let cost = orkestr_providers::estimate_cost(&model.model, &usage).unwrap_or(0.0);
        let rec = UsageRecord {
            id: UsageRecordId::new(),
            ts: now(),
            provider_id: model.provider_id,
            model: model.model.clone(),
            project_id: Some(self.agent.project_id),
            task_id: Some(self.task.id),
            agent_id: Some(self.agent.id),
            execution_id: Some(self.execution.id),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cost_usd: cost,
            estimated: usage.estimated || usage.reported_cost_usd.is_none(),
            latency_ms,
            success,
        };
        if let Err(e) = self.deps.cost.ingest(&rec).await {
            tracing::warn!(error = %e, "usage ingest failed");
        }
        let s = UsageSummary { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens, cost_usd: cost };
        self.agent.usage.add(&s);
        self.run_usage.add(&s);
        self.execution.usage.add(&s);
    }

    // ------------------------------------------------------------------
    // chat tool loop
    // ------------------------------------------------------------------

    fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .filter_map(|n| descriptor(n))
            .map(|d| ToolSpec { name: d.name.into(), description: d.description.into(), parameters: d.input_schema })
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    async fn chat_loop(&mut self) -> Outcome {
        let system = crate::prompt::system_prompt(
            &self.template,
            &self.project,
            &self.task,
            &self.agent,
            &self.tools,
            &self.deps.config.ui_language.get(),
        );
        let previous = self.store.latest_task_handoff(self.task.id).await.ok().flatten().map(|h| handoff::render(&h));
        let initial = crate::prompt::initial_message(
            &self.deps,
            &self.template,
            &self.project,
            &self.task,
            self.env.root.as_deref(),
            previous,
        )
        .await;
        self.message(MessageRole::User, &initial, &[]).await;
        let mut messages = vec![ChatMessage::user(initial)];
        let specs = self.tool_specs();
        let mut nudged = false;
        loop {
            if self.control.cancel.is_cancelled() || !self.wait_if_paused().await {
                return Outcome::Cancelled;
            }
            if self.agent.turns_used >= self.agent.max_turns {
                return Outcome::Limit {
                    code: "max_turns",
                    message: format!("turn limit of {} reached without submit_result", self.agent.max_turns),
                };
            }
            let mut req = ChatRequest::new(self.head_model().map(|m| m.model).unwrap_or_default(), messages.clone());
            req.system = Some(match &self.budget_note {
                Some(n) => format!("{system}\n\n{n}"),
                None => system.clone(),
            });
            req.tools = specs.clone();
            req.tool_choice = ToolChoice::Auto;
            req.max_output_tokens = Some(self.deps.config.max_output_tokens);
            if let Err(o) = self.check_budget(u64::try_from(req.prompt_chars() / 4).unwrap_or(u64::MAX)).await {
                return o;
            }
            self.set_state(AgentState::Thinking, Some("Waiting for the model".into())).await;

            let hook: Arc<dyn HandoffHook> = Arc::new(FallbackHook {
                store: self.store.clone(),
                agent: self.agent.clone(),
                task: self.task.clone(),
                progress: Arc::clone(&self.progress),
                turns_left: self.agent.max_turns.saturating_sub(self.agent.turns_used),
            });
            let chain = self.chain.clone();
            let mut stream = self.deps.models.chat(chain, req, Some(hook), self.control.cancel.child_token());
            let started = Instant::now();
            let mut first: Option<u64> = None;
            let mut message_id = uuid::Uuid::now_v7().to_string();
            let mut text = String::new();
            let mut calls: Vec<ToolCallPart> = Vec::new();
            let mut usage: Option<Usage> = None;
            let mut model = self.head_model().unwrap_or_else(|| self.chain[0].clone());
            let mut ended = false;
            while let Some(ev) = stream.next().await {
                first.get_or_insert_with(|| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                match ev {
                    ChatEvent::TextDelta(t) => {
                        text.push_str(&t);
                        self.publish(EventKind::AgentMessage(MessageDeltaPayload {
                            agent_id: Some(self.agent.id),
                            message_id: message_id.clone(),
                            role: MessageRole::Assistant,
                            delta: redact(&t),
                            done: false,
                        }))
                        .await;
                    }
                    ChatEvent::ToolCall { id, name, arguments } => calls.push(ToolCallPart { id, name, arguments }),
                    ChatEvent::Usage(u) => usage = Some(u),
                    ChatEvent::Switched { from, to, reason } => {
                        // The failed attempt's output is discarded by the executor.
                        if let Some(u) = usage.take() {
                            self.record_usage(&from, u, first, false).await;
                        }
                        self.publish(EventKind::AgentMessage(MessageDeltaPayload {
                            agent_id: Some(self.agent.id),
                            message_id: message_id.clone(),
                            role: MessageRole::Assistant,
                            delta: String::new(),
                            done: true,
                        }))
                        .await;
                        message_id = uuid::Uuid::now_v7().to_string();
                        text.clear();
                        calls.clear();
                        self.agent.model = Some(to.clone());
                        self.execution.model = Some(to.clone());
                        model = to.clone();
                        self.persist().await;
                        let _ = self
                            .deps
                            .tasks
                            .record_agent_run(
                                self.task.id,
                                AgentRunUpdate { model: Some(to.clone()), ..Default::default() },
                            )
                            .await;
                        self.note(&format!(
                            "Provider fallback: {} → {} ({reason}). A structured handoff was created.",
                            from.model, to.model
                        ))
                        .await;
                    }
                    ChatEvent::Done { .. } => {
                        ended = true;
                        break;
                    }
                    ChatEvent::Error(e) => {
                        if let Some(u) = usage.take() {
                            self.record_usage(&model, u, first, false).await;
                        }
                        self.persist().await;
                        if self.control.cancel.is_cancelled() || e == orkestr_providers::ProviderError::Cancelled {
                            return Outcome::Cancelled;
                        }
                        return Outcome::Failed(format!("model call failed: {}", redact(&e.to_string())));
                    }
                }
            }
            if !ended {
                if self.control.cancel.is_cancelled() {
                    return Outcome::Cancelled;
                }
                return Outcome::Failed("the model stream ended unexpectedly".into());
            }
            self.publish(EventKind::AgentMessage(MessageDeltaPayload {
                agent_id: Some(self.agent.id),
                message_id: message_id.clone(),
                role: MessageRole::Assistant,
                delta: String::new(),
                done: true,
            }))
            .await;
            let u =
                usage.unwrap_or_else(|| Usage::estimate(messages.iter().map(|m| m.content.len()).sum(), text.len()));
            self.record_usage(&model, u, first, true).await;
            self.agent.turns_used += 1;
            if !text.trim().is_empty() {
                self.progress.lock().last_text = one_line(&text, 600);
            }
            let ids: Vec<ToolCallId> = calls.iter().map(|_| ToolCallId::new()).collect();
            // Persist the visible assistant text with its tool-call refs (no reasoning exists in ChatEvent).
            let mid = message_id.clone();
            let stored = redact(&truncate_middle(&text, 64_000));
            if let Err(e) = self
                .store
                .add_message(&mid, self.agent.id, Some(self.execution.id), MessageRole::Assistant, &stored, &ids)
                .await
            {
                tracing::warn!(error = %e, "failed to persist assistant message");
            }
            self.persist().await;
            messages.push(ChatMessage::assistant_tool_calls(text.clone(), calls.clone()));

            if calls.is_empty() {
                if nudged {
                    return Outcome::Submitted(SubmitResult {
                        status: SubmitStatus::Partial,
                        summary: if text.trim().is_empty() {
                            "The agent stopped without a result.".into()
                        } else {
                            text
                        },
                        files_changed: Vec::new(),
                        tests_run: Vec::new(),
                        open_issues: vec!["the agent finished without calling submit_result".into()],
                    });
                }
                nudged = true;
                let nudge = "You replied without calling a tool. Continue the task with the tools, or call submit_result now if you are finished or blocked.";
                self.message(MessageRole::System, nudge, &[]).await;
                messages.push(ChatMessage::user(nudge));
                continue;
            }
            for (call, id) in calls.into_iter().zip(ids) {
                match self.execute_call(&call, id).await {
                    ToolExec::Result { content, is_error } => {
                        messages.push(ChatMessage::tool_result(call.id.clone(), content, is_error));
                    }
                    ToolExec::Submitted(r) => return Outcome::Submitted(r),
                    ToolExec::Cancelled => return Outcome::Cancelled,
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // tool execution
    // ------------------------------------------------------------------

    async fn finish_call(
        &mut self,
        call: &mut ToolCall,
        status: ToolCallStatus,
        summary: &str,
        result: Option<&str>,
        started: Instant,
    ) {
        call.status = status;
        call.output_summary = Some(redact(&one_line(summary, 200)));
        call.finished_at = Some(now());
        let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let preview = result.map(|r| redact(&truncate_middle(r, 8_000)));
        if let Err(e) = self.store.save_tool_call(call, preview.as_deref(), Some(duration)).await {
            tracing::warn!(error = %e, "failed to persist tool call");
        }
        self.publish(EventKind::ToolCallFinished(ToolCallFinishedPayload {
            tool_call_id: call.id,
            status,
            output_summary: call.output_summary.clone(),
            duration_ms: duration,
        }))
        .await;
    }

    fn result_text(&self, content: &str) -> String {
        redact(&truncate_middle(content, self.deps.config.tool_result_chars))
    }

    /// Asks the user and waits; the agent (and its task) are `Waiting` meanwhile.
    async fn await_approval(&mut self, id: ApprovalId, call: &mut ToolCall, activity: String) -> ApprovalOutcome {
        call.status = ToolCallStatus::AwaitingApproval;
        call.approval_id = Some(id);
        if let Err(e) = self.store.save_tool_call(call, None, None).await {
            tracing::warn!(error = %e, "failed to persist tool call");
        }
        self.set_state(AgentState::Waiting, Some(activity)).await;
        self.task_to(TaskStatus::Waiting, "awaiting_approval").await;
        let status =
            self.deps.security.approvals.wait(id, self.deps.config.approval_timeout, &self.control.cancel).await;
        let note = self.deps.security.approvals.get(id).await.ok().flatten().and_then(|a| a.resolution_note);
        if self.control.cancel.is_cancelled() {
            return ApprovalOutcome::Cancelled;
        }
        self.task_to(TaskStatus::Running, "approval_resolved").await;
        self.set_state(AgentState::Thinking, None).await;
        match status {
            Ok(ApprovalStatus::Approved) => ApprovalOutcome::Approved(note),
            Ok(ApprovalStatus::Rejected) => ApprovalOutcome::Rejected(note),
            Ok(ApprovalStatus::Cancelled) => ApprovalOutcome::Cancelled,
            Ok(ApprovalStatus::Expired | ApprovalStatus::Pending) => ApprovalOutcome::Expired,
            Err(e) => {
                tracing::warn!(error = %e, "approval wait failed");
                ApprovalOutcome::Expired
            }
        }
    }

    async fn class_state(&mut self, class: ToolClass, summary: &str) {
        let reviewer = matches!(
            self.agent.role,
            orkestr_domain::AgentRole::Reviewer | orkestr_domain::AgentRole::Security | orkestr_domain::AgentRole::Qa
        );
        let to = match class {
            ToolClass::Mutate => AgentState::Coding,
            ToolClass::Verify => AgentState::Testing,
            ToolClass::Inspect if reviewer => AgentState::Reviewing,
            _ => AgentState::Thinking,
        };
        self.set_state(to, Some(summary.to_owned())).await;
    }

    async fn checkpoint(&mut self) {
        if self.checkpoint_taken || self.env.root.is_none() {
            return;
        }
        self.checkpoint_taken = true;
        match tools::memory::snapshot(
            &self.env,
            format!("Before {} changes", self.agent.name),
            SnapshotReason::BeforeRiskyAction,
            Some(TaskSnapshotPhase::Before),
        )
        .await
        {
            Ok(s) => {
                self.progress.lock().snapshot_id = Some(s.id);
                self.note(&format!("Checkpoint {} taken before the first change.", s.id)).await;
            }
            Err(e) => tracing::warn!(agent_id = %self.agent.id, error = %e.message, "automatic checkpoint failed"),
        }
    }

    /// Executes one model tool call (validation → permission → execution).
    #[allow(clippy::too_many_lines)]
    async fn execute_call(&mut self, part: &ToolCallPart, id: ToolCallId) -> ToolExec {
        let started = Instant::now();
        let name = part.name.as_str();
        let desc = descriptor(name);
        let mut call = ToolCall {
            id,
            execution_id: self.execution.id,
            agent_id: self.agent.id,
            task_id: Some(self.task.id),
            tool: name.chars().take(64).collect(),
            capability: desc.as_ref().and_then(|d| d.capability).unwrap_or(Capability::Read),
            input: orkestr_security::redact::redact_json(&part.arguments),
            output_summary: None,
            status: ToolCallStatus::Running,
            approval_id: None,
            started_at: now(),
            finished_at: None,
        };
        if let Err(e) = self.store.save_tool_call(&call, None, None).await {
            tracing::warn!(error = %e, "failed to persist tool call");
        }
        self.publish(EventKind::ToolCallStarted(ToolCallStartedPayload { tool_call: call.clone() })).await;

        let Some(desc) = desc.filter(|_| self.tools.iter().any(|t| t == name)) else {
            let msg = format!("tool `{name}` is not available to this agent. Available: {}", self.tools.join(", "));
            self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
            return ToolExec::Result { content: msg, is_error: true };
        };
        if let Err(e) = tools::schema::validate(&desc.input_schema, &part.arguments) {
            let msg = format!("invalid arguments: {e}");
            self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
            return ToolExec::Result { content: msg, is_error: true };
        }
        let args = &part.arguments;

        match name {
            "submit_result" => {
                return match serde_json::from_value::<SubmitResult>(args.clone()) {
                    Ok(r) => {
                        self.finish_call(&mut call, ToolCallStatus::Succeeded, &r.summary, Some(&r.summary), started)
                            .await;
                        ToolExec::Submitted(r)
                    }
                    Err(e) => {
                        let msg = format!("invalid submit_result: {e}");
                        self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
                        ToolExec::Result { content: msg, is_error: true }
                    }
                };
            }
            "ask_user" => return self.ask_user(args, call, started).await,
            "run_command" => return self.run_command(args, call, started).await,
            _ => {}
        }

        // Permission check (actor = agent, confined to the agent root).
        match tools::permission_target(name, args) {
            Err(e) => {
                self.finish_call(&mut call, ToolCallStatus::Denied, &e.message, Some(&e.message), started).await;
                return ToolExec::Result { content: e.message, is_error: true };
            }
            Ok(None) => {}
            Ok(Some((cap, resource))) => {
                let decision =
                    self.deps.security.engine.check(&Actor::Agent(self.agent.id), cap, &resource, &self.check_ctx());
                match decision {
                    Decision::Allow => {}
                    Decision::Deny(r) => {
                        let msg = format!("denied: {}", r.message);
                        self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                        self.progress.lock().step(format!("{name} denied"));
                        return ToolExec::Result { content: msg, is_error: true };
                    }
                    Decision::Ask(r) => {
                        let mut req = ApprovalRequest::new(
                            cap,
                            format!("{} wants to {name} {}", self.agent.name, one_line(&resource.summary(), 120)),
                            r.message.clone(),
                            r.risk,
                        );
                        req.project_id = Some(self.agent.project_id);
                        req.task_id = Some(self.task.id);
                        req.agent_id = Some(self.agent.id);
                        req.tool_call_id = Some(call.id);
                        req.command_preview = Some(truncate_middle(&call.input.to_string(), 4_000));
                        req.danger = r.danger;
                        let approval = match self.deps.security.approvals.request(req).await {
                            Ok(a) => a,
                            Err(e) => {
                                let msg = format!("approval could not be requested: {e}");
                                self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                                return ToolExec::Result { content: msg, is_error: true };
                            }
                        };
                        match self.await_approval(approval.id, &mut call, format!("Waiting for approval: {name}")).await
                        {
                            ApprovalOutcome::Approved(_) => {}
                            ApprovalOutcome::Cancelled => {
                                self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started)
                                    .await;
                                return ToolExec::Cancelled;
                            }
                            ApprovalOutcome::Rejected(note) => {
                                let msg = format!(
                                    "the user rejected this action{}",
                                    note.map(|n| format!(": {n}")).unwrap_or_default()
                                );
                                self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                                return ToolExec::Result { content: msg, is_error: true };
                            }
                            ApprovalOutcome::Expired => {
                                let msg = "the approval expired without a decision".to_owned();
                                self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                                return ToolExec::Result { content: msg, is_error: true };
                            }
                        }
                    }
                }
            }
        }

        if desc.class == ToolClass::Mutate {
            self.checkpoint().await;
        }
        let activity = format!("{name} {}", one_line(&resource_hint(args), 80)).trim().to_owned();
        self.class_state(desc.class, &activity).await;
        call.status = ToolCallStatus::Running;
        let env = self.env.clone();
        let cancel = self.control.cancel.clone();
        let result = tokio::select! {
            r = tools::execute(name, args, &env) => Some(r),
            () = cancel.cancelled() => None,
            () = tokio::time::sleep(desc.timeout) => Some(Err(tools::ToolError::new(format!("tool timed out after {}s", desc.timeout.as_secs())))),
        };
        let exec = match result {
            None => {
                self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started).await;
                return ToolExec::Cancelled;
            }
            Some(Ok(out)) => {
                self.absorb(name, args, &out);
                self.finish_call(&mut call, ToolCallStatus::Succeeded, &out.summary, Some(&out.content), started).await;
                ToolExec::Result { content: self.result_text(&out.content), is_error: out.is_error }
            }
            Some(Err(e)) => {
                self.finish_call(&mut call, ToolCallStatus::Failed, &e.message, Some(&e.message), started).await;
                ToolExec::Result { content: format!("error: {}", e.message), is_error: true }
            }
        };
        self.set_state(AgentState::Thinking, None).await;
        exec
    }

    fn absorb(&self, name: &str, args: &Value, out: &ToolOutput) {
        let mut p = self.progress.lock();
        for f in &out.files_changed {
            p.file(f);
        }
        if let Some(s) = out.snapshot_id {
            p.snapshot_id = Some(s);
        }
        if let Some((command, passed, summary)) = &out.test {
            p.test_state =
                Some(HandoffTestState { command: command.clone(), passed: *passed, summary: summary.clone() });
        }
        if name == "record_decision" {
            p.decisions.push(HandoffDecision {
                title: tools::opt_str(args, "title").unwrap_or_default().to_owned(),
                choice: tools::opt_str(args, "decision").unwrap_or_default().to_owned(),
                rationale_summary: tools::opt_str(args, "rationale_summary").map(str::to_owned),
            });
        }
        if matches!(
            name,
            "write_file" | "edit_file" | "git_commit" | "run_command" | "record_decision" | "record_memory"
        ) {
            p.step(out.summary.clone());
        }
    }

    async fn ask_user(&mut self, args: &Value, mut call: ToolCall, started: Instant) -> ToolExec {
        let question = tools::opt_str(args, "question").unwrap_or_default().trim().to_owned();
        if question.is_empty() {
            let msg = "`question` must not be empty".to_owned();
            self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
            return ToolExec::Result { content: msg, is_error: true };
        }
        let options = tools::opt_list(args, "options");
        let mut preview = question.clone();
        if !options.is_empty() {
            preview.push_str("\n\nOptions: ");
            preview.push_str(&options.join(" / "));
        }
        let mut req = ApprovalRequest::new(
            Capability::Read,
            format!("Question from {}", self.agent.name),
            format!("The agent asks: {}", one_line(&question, 300)),
            RiskLevel::Low,
        );
        req.project_id = Some(self.agent.project_id);
        req.task_id = Some(self.task.id);
        req.agent_id = Some(self.agent.id);
        req.tool_call_id = Some(call.id);
        req.command_preview = Some(preview);
        let approval = match self.deps.security.approvals.request(req).await {
            Ok(a) => a,
            Err(e) => {
                let msg = format!("could not ask the user: {e}");
                self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
                return ToolExec::Result { content: msg, is_error: true };
            }
        };
        let answer = match self.await_approval(approval.id, &mut call, "Waiting for the user's answer".into()).await {
            ApprovalOutcome::Cancelled => {
                self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started).await;
                return ToolExec::Cancelled;
            }
            ApprovalOutcome::Approved(note) => {
                format!("The user answered YES / approved.{}", note.map(|n| format!(" Note: {n}")).unwrap_or_default())
            }
            ApprovalOutcome::Rejected(note) => {
                format!("The user answered NO / rejected.{}", note.map(|n| format!(" Note: {n}")).unwrap_or_default())
            }
            ApprovalOutcome::Expired => "The question expired without an answer. Decide conservatively.".into(),
        };
        self.finish_call(&mut call, ToolCallStatus::Succeeded, &answer, Some(&answer), started).await;
        ToolExec::Result { content: answer, is_error: false }
    }

    /// `run_command`: the terminal's security gate is the permission check
    /// (actor = agent); `Ask` → approval → the gate lets the same command
    /// through once.
    async fn run_command(&mut self, args: &Value, mut call: ToolCall, started: Instant) -> ToolExec {
        let command = tools::opt_str(args, "command").unwrap_or_default().trim().to_owned();
        let Some(root) = self.env.root.clone() else {
            let msg = "this agent has no workspace to run commands in".to_owned();
            self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
            return ToolExec::Result { content: msg, is_error: true };
        };
        if command.is_empty() {
            let msg = "`command` must not be empty".to_owned();
            self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
            return ToolExec::Result { content: msg, is_error: true };
        }
        let class = classify_command(&command);
        let cwd = tools::opt_str(args, "cwd").map(std::path::PathBuf::from);
        let timeout = tools::opt_u64(args, "timeout_secs")
            .map_or(self.deps.config.command_timeout, |s| Duration::from_secs(s.clamp(1, 1800)));
        let mut approved_once = false;
        loop {
            if class == ToolClass::Mutate {
                self.checkpoint().await;
            }
            self.class_state(class, &format!("$ {}", one_line(&command, 80))).await;
            let req = RunRequest {
                project_id: Some(self.agent.project_id),
                task_id: Some(self.task.id),
                root: Some(root.clone()),
                cwd: cwd.clone(),
                owner: TerminalOwner::Agent { agent_id: self.agent.id },
                shell: ShellSpec::Default,
                command: command.clone(),
                env: Vec::new(),
                timeout: Some(timeout),
                max_output: Some(256 * 1024),
                stream_to_view: Some(true),
            };
            match self.deps.terminals.run(req, self.control.cancel.child_token()).await {
                Ok(out) => {
                    if self.control.cancel.is_cancelled() {
                        self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started).await;
                        return ToolExec::Cancelled;
                    }
                    let code = out.exit_code;
                    let ok = code == Some(0) && !out.timed_out;
                    let mut content = format!(
                        "exit code: {}{}\n",
                        code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
                        if out.timed_out { " (timed out)" } else { "" }
                    );
                    if !out.stdout.trim().is_empty() {
                        content.push_str(&format!("--- stdout ---\n{}\n", out.stdout.trim_end()));
                    }
                    if !out.stderr.trim().is_empty() {
                        content.push_str(&format!("--- stderr ---\n{}\n", out.stderr.trim_end()));
                    }
                    if out.truncated {
                        content.push_str("(output truncated)\n");
                    }
                    let summary =
                        format!("`{}` → exit {}", one_line(&command, 60), code.map_or("?".into(), |c| c.to_string()));
                    {
                        let mut p = self.progress.lock();
                        if class == ToolClass::Verify {
                            p.test_state = Some(HandoffTestState {
                                command: command.clone(),
                                passed: ok,
                                summary: one_line(&summary, 200),
                            });
                        }
                        p.step(summary.clone());
                    }
                    let status = if ok { ToolCallStatus::Succeeded } else { ToolCallStatus::Failed };
                    self.finish_call(&mut call, status, &summary, Some(&content), started).await;
                    self.set_state(AgentState::Thinking, None).await;
                    return ToolExec::Result { content: self.result_text(&content), is_error: !ok };
                }
                Err(TerminalError::ApprovalRequired { reason, approval_id }) => {
                    let parsed = approval_id.as_deref().and_then(|s| s.parse::<ApprovalId>().ok());
                    let Some(id) = parsed.filter(|_| !approved_once) else {
                        let msg = format!("command needs approval but none could be obtained: {reason}");
                        self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                        return ToolExec::Result { content: msg, is_error: true };
                    };
                    match self
                        .await_approval(id, &mut call, format!("Waiting for approval: {}", one_line(&command, 60)))
                        .await
                    {
                        ApprovalOutcome::Approved(_) => {
                            approved_once = true;
                            self.note(&format!("Command approved: {}", one_line(&command, 120))).await;
                        }
                        ApprovalOutcome::Cancelled => {
                            self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started).await;
                            return ToolExec::Cancelled;
                        }
                        ApprovalOutcome::Rejected(note) => {
                            let msg = format!(
                                "the user rejected the command{}",
                                note.map(|n| format!(": {n}")).unwrap_or_default()
                            );
                            self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                            self.set_state(AgentState::Thinking, None).await;
                            return ToolExec::Result { content: msg, is_error: true };
                        }
                        ApprovalOutcome::Expired => {
                            let msg = "the command approval expired".to_owned();
                            self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                            return ToolExec::Result { content: msg, is_error: true };
                        }
                    }
                }
                Err(TerminalError::Denied(reason)) => {
                    let msg = format!("denied: {reason}");
                    self.finish_call(&mut call, ToolCallStatus::Denied, &msg, Some(&msg), started).await;
                    self.set_state(AgentState::Thinking, None).await;
                    return ToolExec::Result { content: msg, is_error: true };
                }
                Err(e) => {
                    if self.control.cancel.is_cancelled() {
                        self.finish_call(&mut call, ToolCallStatus::Cancelled, "cancelled", None, started).await;
                        return ToolExec::Cancelled;
                    }
                    let msg = format!("command failed to run: {e}");
                    self.finish_call(&mut call, ToolCallStatus::Failed, &msg, Some(&msg), started).await;
                    self.set_state(AgentState::Thinking, None).await;
                    return ToolExec::Result { content: msg, is_error: true };
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // finish
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    async fn finish(&mut self, outcome: Outcome) {
        let outcome = match (&outcome, self.control.cause()) {
            (Outcome::Cancelled, Some(StopCause::Timeout)) => Outcome::Limit {
                code: "time_limit",
                message: format!("wall-clock limit reached after {}s", self.started.elapsed().as_secs()),
            },
            _ => outcome,
        };
        let cause = self.control.cause();
        tracing::info!(agent_id = %self.agent.id, outcome = ?outcome_label(&outcome), ?cause, "agent run finished");

        // Snapshot the worktree when the agent ends with changes (§25): the
        // after-checkpoint that `snapshots.rollbackTask` pairs with the
        // automatic before-checkpoint, so every task with changes can be
        // rolled back — including tasks that finished successfully.
        if !self.progress.lock().files_changed.is_empty() {
            let label = if matches!(outcome, Outcome::Submitted(_)) { "finished" } else { "stopped" };
            match tools::memory::snapshot(
                &self.env,
                format!("{} {label}", self.agent.name),
                SnapshotReason::TaskCheckpoint,
                Some(TaskSnapshotPhase::After),
            )
            .await
            {
                Ok(s) => self.progress.lock().snapshot_id = Some(s.id),
                Err(e) => tracing::warn!(error = %e.message, "stop snapshot failed"),
            }
        }

        let (state, reason, exec_status, handoff_reason, task_target, detail): (
            AgentState,
            WireStopReason,
            ExecutionStatus,
            HandoffReason,
            Option<(TaskStatus, String)>,
            Option<String>,
        ) = match &outcome {
            Outcome::Submitted(r) => {
                let gates = &self.template.quality_gates;
                let target = match r.status {
                    SubmitStatus::Done if gates.is_empty() => TaskStatus::Completed,
                    SubmitStatus::Done if gates.contains(&QualityGate::Review) => TaskStatus::Review,
                    SubmitStatus::Done => TaskStatus::Testing,
                    SubmitStatus::Partial => TaskStatus::Review,
                    SubmitStatus::Blocked | SubmitStatus::Failed => TaskStatus::Failed,
                };
                let ok = matches!(r.status, SubmitStatus::Done | SubmitStatus::Partial);
                (
                    AgentState::Completed,
                    WireStopReason::Completed,
                    if ok { ExecutionStatus::Succeeded } else { ExecutionStatus::Failed },
                    HandoffReason::Completed,
                    Some((target, format!("agent_result_{:?}", r.status).to_lowercase())),
                    None,
                )
            }
            Outcome::Failed(m) => (
                AgentState::Error,
                WireStopReason::Error,
                ExecutionStatus::Failed,
                HandoffReason::Failed,
                Some((TaskStatus::Failed, format!("agent_error: {}", one_line(m, 300)))),
                Some(m.clone()),
            ),
            Outcome::Limit { code, message } => (
                AgentState::Error,
                WireStopReason::LimitReached,
                ExecutionStatus::Failed,
                HandoffReason::Stopped,
                Some((TaskStatus::Failed, format!("{code}: {}", one_line(message, 300)))),
                Some(format!("{code}: {message}")),
            ),
            Outcome::Cancelled => match cause {
                Some(StopCause::User(AgentStopMode::Interrupt)) => (
                    AgentState::Stopped,
                    WireStopReason::UserRequested,
                    ExecutionStatus::Cancelled,
                    HandoffReason::Stopped,
                    Some((TaskStatus::Interrupted, "stopped by the user".into())),
                    Some("stopped by the user".into()),
                ),
                Some(StopCause::User(AgentStopMode::Cancel)) => (
                    AgentState::Stopped,
                    WireStopReason::UserRequested,
                    ExecutionStatus::Cancelled,
                    HandoffReason::Stopped,
                    Some((TaskStatus::Cancelled, "cancelled by the user".into())),
                    Some("stopped by the user (task cancelled)".into()),
                ),
                Some(StopCause::Shutdown) => (
                    AgentState::Stopped,
                    WireStopReason::Shutdown,
                    ExecutionStatus::Interrupted,
                    HandoffReason::Stopped,
                    None,
                    Some("the core is shutting down".into()),
                ),
                Some(StopCause::Timeout) | None => (
                    AgentState::Stopped,
                    WireStopReason::EmergencyStop,
                    ExecutionStatus::Cancelled,
                    HandoffReason::Stopped,
                    Some((TaskStatus::Cancelled, "EmergencyStop".into())),
                    Some("emergency stop".into()),
                ),
            },
        };

        // Task outcome (through the task engine).
        if let Some((target, why)) = task_target {
            match target {
                TaskStatus::Interrupted => {
                    if self.task.status == TaskStatus::Waiting {
                        self.task_to(TaskStatus::Running, "agent_stopping").await;
                    }
                    self.task_to(TaskStatus::Interrupted, &why).await;
                }
                TaskStatus::Review | TaskStatus::Testing | TaskStatus::Completed => {
                    if self.task.status == TaskStatus::Waiting {
                        self.task_to(TaskStatus::Running, "agent_finishing").await;
                    }
                    self.task_to(target, &why).await;
                }
                other => {
                    if !matches!(self.task.status, TaskStatus::Completed | TaskStatus::Cancelled) {
                        self.task_to(other, &why).await;
                    }
                }
            }
        }

        let (files, result) = {
            let p = self.progress.lock();
            let mut files = p.files_changed.clone();
            let result = match &outcome {
                Outcome::Submitted(r) => {
                    for f in &r.files_changed {
                        if !files.contains(f) {
                            files.push(f.clone());
                        }
                    }
                    Some(TaskResult { success: r.status == SubmitStatus::Done, summary: redact(&r.summary) })
                }
                Outcome::Failed(m) | Outcome::Limit { message: m, .. } => {
                    Some(TaskResult { success: false, summary: redact(m) })
                }
                Outcome::Cancelled => None,
            };
            (files, result)
        };
        if let Outcome::Submitted(r) = &outcome {
            let mut p = self.progress.lock();
            p.open_issues.extend(r.open_issues.iter().cloned());
            if let Some(t) = r.tests_run.last() {
                p.test_state = Some(HandoffTestState {
                    command: t.command.clone(),
                    passed: t.passed.unwrap_or(false),
                    summary: t.summary.clone().unwrap_or_default(),
                });
            }
            p.last_text = one_line(&r.summary, 600);
        }
        if let Err(e) = self
            .deps
            .tasks
            .record_agent_run(
                self.task.id,
                AgentRunUpdate {
                    usage_delta: Some(self.run_usage),
                    files_touched: files,
                    result,
                    ..AgentRunUpdate::default()
                },
            )
            .await
        {
            tracing::warn!(error = %e, "could not record the run on the task");
        }

        // Handoff.
        let h = {
            let p = self.progress.lock().clone();
            handoff::build(&HandoffInput {
                agent: &self.agent,
                task: &self.task,
                reason: handoff_reason,
                to_model: None,
                progress: &p,
                note: detail.clone(),
                turns_left: self.agent.max_turns.saturating_sub(self.agent.turns_used),
                cost_left: self
                    .agent
                    .budget
                    .as_ref()
                    .and_then(|b| b.max_cost_usd)
                    .map(|m| (m - self.agent.usage.cost_usd).max(0.0)),
            })
        };
        if let Err(e) = self.store.add_handoff(self.agent.id, &h).await {
            tracing::warn!(error = %e, "could not persist handoff");
        }

        // Execution.
        self.execution.status = exec_status;
        self.execution.finished_at = Some(now());
        self.execution.summary = match &outcome {
            Outcome::Submitted(r) => Some(redact(&truncate_middle(&r.summary, 4_000))),
            _ => None,
        };
        self.execution.error = detail.as_deref().filter(|_| exec_status != ExecutionStatus::Succeeded).map(redact);
        if let Err(e) = self.store.save_execution(&self.execution).await {
            tracing::warn!(error = %e, "could not persist execution");
        }

        // Agent-scoped permission rules from the template are removed.
        remove_agent_rules(&self.deps, self.agent.id).await;
        self.deps.security.emergency.unregister_agent(self.agent.id);

        // Final state + AgentStopped.
        self.agent.last_error = match (&outcome, &detail) {
            (Outcome::Failed(_) | Outcome::Limit { .. }, Some(d)) => Some(redact(d)),
            _ => None,
        };
        self.agent.stopped_at = Some(now());
        if let Outcome::Submitted(r) = &outcome {
            self.note(&format!("Result ({:?}): {}", r.status, one_line(&r.summary, 300))).await;
        } else if let Some(d) = &detail {
            self.note(&format!("Stopped: {d}")).await;
        }
        let activity = match &outcome {
            Outcome::Submitted(_) => Some("Finished".to_owned()),
            _ => detail.clone(),
        };
        self.force_state(state, activity).await;
        self.publish(EventKind::AgentStopped(AgentStoppedPayload {
            agent_id: self.agent.id,
            reason,
            detail: detail.map(|d| redact(&d)),
        }))
        .await;
    }
}

fn outcome_label(o: &Outcome) -> &'static str {
    match o {
        Outcome::Submitted(_) => "submitted",
        Outcome::Failed(_) => "failed",
        Outcome::Limit { code, .. } => code,
        Outcome::Cancelled => "cancelled",
    }
}

fn resource_hint(args: &Value) -> String {
    for k in ["path", "pattern", "message", "title", "label"] {
        if let Some(v) = args.get(k).and_then(Value::as_str) {
            return v.to_owned();
        }
    }
    String::new()
}

/// Deletes the Agent-scope permission rules installed at spawn.
pub async fn remove_agent_rules(deps: &RuntimeDeps, agent: orkestr_domain::AgentId) {
    let filter = orkestr_security::permissions::RuleFilter { project_id: None, agent_id: Some(agent), task_id: None };
    let Ok(rules) = deps.security.permissions.list(&filter).await else { return };
    for r in rules {
        if matches!(r.rule.scope, orkestr_domain::PermissionScope::Agent { agent_id } if agent_id == agent) {
            if let Err(e) = deps.security.permissions.delete(&r.id, deps.security.user()).await {
                tracing::warn!(error = %e, "could not remove an agent permission rule");
            }
        }
    }
}

/// Builds and records a structured handoff before every provider switch
/// (§7, §57) and gives the next model the handoff in its system prompt.
struct FallbackHook {
    store: AgentStore,
    agent: AgentInstance,
    task: Task,
    progress: Arc<Mutex<Progress>>,
    turns_left: u32,
}

#[async_trait]
impl HandoffHook for FallbackHook {
    async fn handoff(&self, ctx: &HandoffContext, mut request: ChatRequest) -> Option<ChatRequest> {
        let mut agent = self.agent.clone();
        agent.model = Some(ctx.from.clone());
        let p = self.progress.lock().clone();
        let h = handoff::build(&HandoffInput {
            agent: &agent,
            task: &self.task,
            reason: HandoffReason::ProviderFallback,
            to_model: Some(ctx.to.clone()),
            progress: &p,
            note: Some(format!("{} failed: {}", ctx.from.model, ctx.error)),
            turns_left: self.turns_left,
            cost_left: None,
        });
        if let Err(e) = self.store.add_handoff(self.agent.id, &h).await {
            tracing::warn!(error = %e, "could not persist fallback handoff");
        }
        let note = format!(
            "\n\n## Handoff\nYou are taking over from {} ({}). The conversation so far is included.\n{}",
            ctx.from.model,
            ctx.error.class(),
            handoff::render(&h)
        );
        request.system = Some(format!("{}{note}", request.system.unwrap_or_default()));
        Some(request)
    }
}

/// Helper: JSON for tests and the CLI result parser.
#[must_use]
pub fn submit_json(status: &str, summary: &str) -> Value {
    json!({ "status": status, "summary": summary })
}

/// Pending CLI tool calls keyed by CLI call id.
pub type CliCalls = HashMap<String, (ToolCall, Instant)>;
