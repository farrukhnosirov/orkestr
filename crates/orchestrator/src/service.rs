//! [`Manager`]: the AI Manager service — conversations, plans, runs.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{
    now, ApprovalMode, ConversationId, ManagerConfig, ManagerPlanId, ManagerRunId, Project, ProjectId, TaskActor,
};
use orkestr_protocol::event::{EventKind, ManagerMessageAddedPayload, ManagerPlanPayload, ManagerRunPayload};
use orkestr_protocol::methods::manager::{
    ApprovePlanParams, ApprovePlanResult, ManagerChatMessage, ManagerClearResult, ManagerHistoryParams,
    ManagerHistoryResult, ManagerMessageRole, ManagerPlan, ManagerRun, ManagerRunParams, ManagerRunStatus,
    ManagerStatusParams, ManagerStatusResult, ManagerToolEvent, PlanStatus, RejectPlanParams,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_protocol::NewEvent;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::deps::OrchestratorDeps;
use crate::error::{OrchResult, OrchestratorError};
use crate::plan::{apply_edits, levels, max_parallelism, validate_tasks};
use crate::run::RunControl;
use crate::store::ManagerStore;
use crate::util::{cap, one_line, role_name};

/// Upper bound on finished plans `manager.history` loads for the messages.
pub const MAX_REFERENCED_PLANS: usize = 20;

/// Optional fields of a stored message.
#[derive(Debug, Default, Clone)]
pub struct MessageExtras {
    /// Tools used.
    pub tool_events: Vec<ManagerToolEvent>,
    /// Plan referenced.
    pub plan_id: Option<ManagerPlanId>,
    /// Run referenced.
    pub run_id: Option<ManagerRunId>,
    /// Model.
    pub model: Option<orkestr_domain::ModelRef>,
    /// Usage.
    pub usage: Option<orkestr_domain::UsageSummary>,
    /// Use this id (streamed replies).
    pub id: Option<String>,
}

pub(crate) struct Inner {
    pub deps: OrchestratorDeps,
    pub store: ManagerStore,
    /// Projects with a chat turn in progress (→ its cancel token).
    pub busy: Mutex<HashMap<ProjectId, CancellationToken>>,
    /// Live run supervisors.
    pub runs: Mutex<HashMap<ManagerRunId, Arc<RunControl>>>,
    /// Serialises plan decisions and run starts.
    pub decide: tokio::sync::Mutex<()>,
}

/// The AI Manager service (cheap to clone).
#[derive(Clone)]
pub struct Manager {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager").field("live_runs", &self.inner.runs.lock().len()).finish_non_exhaustive()
    }
}

impl Manager {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: OrchestratorDeps) -> Self {
        let store = ManagerStore::new(deps.db.clone());
        Self {
            inner: Arc::new(Inner {
                deps,
                store,
                busy: Mutex::new(HashMap::new()),
                runs: Mutex::new(HashMap::new()),
                decide: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Persistence.
    #[must_use]
    pub fn store(&self) -> &ManagerStore {
        &self.inner.store
    }

    /// Marks runs that were active when the core stopped as `Interrupted`
    /// (resumable with `manager.resume` / `manager.run {runId}`).
    pub async fn init(&self) -> OrchResult<()> {
        let stale =
            self.inner.store.runs(None, Some(vec![ManagerRunStatus::Running, ManagerRunStatus::Paused]), 1000).await?;
        for mut run in stale {
            if self.inner.runs.lock().contains_key(&run.id) {
                continue;
            }
            run.status = ManagerRunStatus::Interrupted;
            run.status_reason = Some("Orkestr stopped while the run was active — resume it to continue".into());
            run.updated_at = now();
            self.inner.save_run(&run).await;
            tracing::warn!(run_id = %run.id, "manager run from a previous session marked interrupted");
        }
        crate::teams::recover_leads(&self.inner).await;
        Ok(())
    }

    /// Stops every live supervisor (core shutdown): runs become `Interrupted`.
    pub async fn shutdown(&self) {
        let controls: Vec<Arc<RunControl>> = self.inner.runs.lock().values().cloned().collect();
        for c in &controls {
            c.request_stop(crate::run::StopKind::Shutdown);
        }
        for c in controls {
            let mut rx = c.finished.subscribe();
            let _ = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|d| *d)).await;
        }
        let turns: Vec<CancellationToken> = self.inner.busy.lock().values().cloned().collect();
        for t in turns {
            t.cancel();
        }
    }

    /// Whether a chat reply is in progress for the project.
    #[must_use]
    pub fn is_busy(&self, project: ProjectId) -> bool {
        self.inner.busy.lock().contains_key(&project)
    }

    /// Ids of runs supervised by this process.
    #[must_use]
    pub fn live_runs(&self) -> Vec<ManagerRunId> {
        self.inner.runs.lock().keys().copied().collect()
    }

    // ------------------------------------------------------------------
    // history
    // ------------------------------------------------------------------

    /// Conversation, summaries, open plans (plus up to
    /// [`MAX_REFERENCED_PLANS`] plans the loaded messages refer to) and the
    /// active run.
    pub async fn history(&self, p: ManagerHistoryParams) -> OrchResult<ManagerHistoryResult> {
        let project = self.inner.project(p.project_id).await?;
        let (conversation_id, _) = self.inner.store.active_conversation(project.id).await?;
        let limit = p.limit.unwrap_or(200).clamp(1, 1000);
        let messages = self.inner.store.messages(conversation_id, limit).await?;
        let summaries = self.inner.store.summaries(project.id, 10).await?;
        let mut plans = self
            .inner
            .store
            .plans(project.id, Some(vec![PlanStatus::Proposed, PlanStatus::Approved, PlanStatus::Running]), 5)
            .await?;
        // Plans the loaded messages refer to (finished, rejected, superseded)
        // so every plan card in the conversation can be rendered.
        let mut referenced: Vec<ManagerPlanId> = Vec::new();
        for m in messages.iter().rev() {
            if let Some(id) = m.plan_id {
                if !referenced.contains(&id) && !plans.iter().any(|p| p.id == id) {
                    referenced.push(id);
                }
            }
        }
        for id in referenced.into_iter().take(MAX_REFERENCED_PLANS) {
            match self.inner.store.plan(id).await {
                Ok(plan) => plans.push(plan),
                Err(OrchestratorError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let active_run = self.inner.active_run(project.id).await?;
        let config = self.inner.deps.hub.manager_config().await.unwrap_or_default();
        Ok(ManagerHistoryResult {
            conversation_id,
            messages,
            summaries,
            busy: self.is_busy(project.id),
            plans,
            active_run,
            config,
        })
    }

    /// Archives the conversation with an extractive summary and starts a new one.
    pub async fn clear(&self, project_id: ProjectId) -> OrchResult<ManagerClearResult> {
        let project = self.inner.project(project_id).await?;
        if self.is_busy(project.id) {
            return Err(OrchestratorError::Conflict("the Manager is still answering — wait or try again".into()));
        }
        let (conv, started) = self.inner.store.active_conversation(project.id).await?;
        let messages = self.inner.store.messages(conv, 5000).await?;
        if messages.is_empty() {
            return Ok(ManagerClearResult { conversation_id: conv, archived: None });
        }
        let summary = summarize_messages(&messages);
        self.inner.store.archive_conversation(conv, summary.clone()).await?;
        let (fresh, _) = self.inner.store.active_conversation(project.id).await?;
        let archived = orkestr_protocol::methods::manager::ConversationSummary {
            conversation_id: conv,
            project_id: project.id,
            summary,
            message_count: u32::try_from(messages.len()).unwrap_or(u32::MAX),
            started_at: started,
            ended_at: now(),
        };
        Ok(ManagerClearResult { conversation_id: fresh, archived: Some(archived) })
    }

    // ------------------------------------------------------------------
    // plans
    // ------------------------------------------------------------------

    /// Approves a proposed plan: applies edits, creates the tasks with their
    /// dependencies in the task engine and (by default) starts a run.
    pub async fn approve_plan(&self, p: ApprovePlanParams) -> OrchResult<ApprovePlanResult> {
        let plan = {
            let _g = self.inner.decide.lock().await;
            self.inner.approve_locked(p.plan_id, p.edits.unwrap_or_default()).await?
        };
        let run = if p.start_run.unwrap_or(true) {
            Some(self.start_run(ManagerRunParams { plan_id: Some(plan.id), run_id: None, limits: None }).await?)
        } else {
            None
        };
        let plan = self.inner.store.plan(plan.id).await?;
        Ok(ApprovePlanResult { plan, run })
    }

    /// Rejects a proposed plan.
    pub async fn reject_plan(&self, p: RejectPlanParams) -> OrchResult<ManagerPlan> {
        let _g = self.inner.decide.lock().await;
        let mut plan = self.inner.store.plan(p.plan_id).await?;
        if plan.status != PlanStatus::Proposed {
            return Err(OrchestratorError::Conflict(format!(
                "the plan is {:?}; only proposed plans can be rejected",
                plan.status
            )));
        }
        let reason = p.reason.map(|r| cap(r.trim(), 2000)).filter(|r| !r.is_empty());
        plan.status = PlanStatus::Rejected;
        plan.rejection_reason = reason.clone();
        plan.updated_at = now();
        self.inner.save_plan(&plan).await?;
        let ru = crate::util::is_ru(&self.inner.deps.config.ui_language.get());
        let text = match (&reason, ru) {
            (Some(r), false) => format!("Plan rejected by the user: {r}"),
            (None, false) => "Plan rejected by the user.".to_owned(),
            (Some(r), true) => format!("Вы отклонили план: {r}"),
            (None, true) => "Вы отклонили план.".to_owned(),
        };
        self.inner
            .system_event(plan.project_id, &text, MessageExtras { plan_id: Some(plan.id), ..MessageExtras::default() })
            .await;
        Ok(plan)
    }

    /// Runs, newest first.
    pub async fn status(&self, p: ManagerStatusParams) -> OrchResult<ManagerStatusResult> {
        let runs = match p.run_id {
            Some(id) => vec![self.inner.store.run(id).await?],
            None => self.inner.store.runs(p.project_id, None, 20).await?,
        };
        Ok(ManagerStatusResult { runs, live_run_ids: self.live_runs() })
    }

    /// One run.
    pub async fn get_run(&self, id: ManagerRunId) -> OrchResult<ManagerRun> {
        self.inner.store.run(id).await
    }

    /// One plan.
    pub async fn get_plan(&self, id: ManagerPlanId) -> OrchResult<ManagerPlan> {
        self.inner.store.plan(id).await
    }

    /// Waits until a run reaches a final status or `pred` holds (tests, CLI).
    pub async fn wait_run(
        &self,
        id: ManagerRunId,
        timeout: Duration,
        pred: impl Fn(&ManagerRun) -> bool,
    ) -> OrchResult<ManagerRun> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let run = self.inner.store.run(id).await?;
            let live = self.inner.runs.lock().contains_key(&id);
            if pred(&run) || (run.status.is_final() && !live) {
                return Ok(run);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(OrchestratorError::Conflict(format!(
                    "timed out waiting for run {id} (status {:?})",
                    run.status
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Inner {
    pub async fn project(&self, id: ProjectId) -> OrchResult<Project> {
        self.deps.projects.get(id).await?.ok_or_else(|| OrchestratorError::NotFound(format!("project {id}")))
    }

    pub async fn config(&self) -> ManagerConfig {
        match self.deps.hub.manager_config().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "AI Manager settings unavailable; using defaults");
                ManagerConfig::default()
            }
        }
    }

    /// The Manager approval policy after emergency caps.
    pub fn policy(&self, cfg: &ManagerConfig) -> ApprovalMode {
        self.deps.security.engine.effective_mode(cfg.approval_policy)
    }

    pub async fn active_run(&self, project: ProjectId) -> OrchResult<Option<ManagerRun>> {
        Ok(self
            .store
            .runs(
                Some(project),
                Some(vec![ManagerRunStatus::Running, ManagerRunStatus::Paused, ManagerRunStatus::Interrupted]),
                1,
            )
            .await?
            .into_iter()
            .next())
    }

    pub fn is_busy(&self, project: ProjectId) -> bool {
        self.busy.lock().contains_key(&project)
    }

    /// Stores a message in the project's active conversation and publishes it.
    pub async fn add_message(
        &self,
        project: ProjectId,
        conversation: Option<ConversationId>,
        role: ManagerMessageRole,
        text: &str,
        extras: MessageExtras,
    ) -> OrchResult<ManagerChatMessage> {
        let conversation_id = match conversation {
            Some(c) => c,
            None => self.store.active_conversation(project).await?.0,
        };
        let msg = ManagerChatMessage {
            id: extras.id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            project_id: project,
            conversation_id,
            role,
            text: crate::util::redact(text),
            tool_events: extras.tool_events,
            plan_id: extras.plan_id,
            run_id: extras.run_id,
            model: extras.model,
            usage: extras.usage,
            created_at: now(),
        };
        self.store.add_message(&msg).await?;
        self.deps
            .publish(
                NewEvent::new(EventKind::ManagerMessageAdded(ManagerMessageAddedPayload {
                    message: msg.clone(),
                    busy: self.is_busy(project),
                }))
                .project(project),
            )
            .await;
        Ok(msg)
    }

    /// A system-event line in the active conversation (failures are logged).
    pub async fn system_event(&self, project: ProjectId, text: &str, extras: MessageExtras) {
        if let Err(e) = self.add_message(project, None, ManagerMessageRole::SystemEvent, text, extras).await {
            tracing::warn!(error = %e, "failed to store a manager system event");
        }
    }

    pub async fn save_plan(&self, plan: &ManagerPlan) -> OrchResult<()> {
        self.store.save_plan(plan).await?;
        self.deps
            .publish(
                NewEvent::new(EventKind::ManagerPlanUpdated(ManagerPlanPayload { plan: plan.clone() }))
                    .project(plan.project_id),
            )
            .await;
        Ok(())
    }

    /// Saves + publishes a run (failures are logged: supervisors keep going).
    pub async fn save_run(&self, run: &ManagerRun) {
        if let Err(e) = self.store.save_run(run).await {
            tracing::error!(run_id = %run.id, error = %e, "failed to persist manager run");
            return;
        }
        self.deps
            .publish(
                NewEvent::new(EventKind::ManagerRunUpdated(ManagerRunPayload { run: run.clone() }))
                    .project(run.project_id),
            )
            .await;
    }

    /// Plan approval (caller holds `decide`).
    pub async fn approve_locked(
        &self,
        plan_id: ManagerPlanId,
        edits: orkestr_protocol::methods::manager::PlanEdits,
    ) -> OrchResult<ManagerPlan> {
        let mut plan = self.store.plan(plan_id).await?;
        if plan.status != PlanStatus::Proposed {
            return Err(OrchestratorError::Conflict(format!(
                "the plan is {:?}; only proposed plans can be approved",
                plan.status
            )));
        }
        if self.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden(
                "Stop All AI is active — reset the emergency controls first".into(),
            ));
        }
        self.project(plan.project_id).await?;
        let notes = apply_edits(&mut plan.tasks, &edits)?;
        validate_tasks(&plan.tasks).map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        plan.warnings.extend(notes);
        // Create tasks level by level so dependencies exist first.
        let order = levels(&plan.tasks).ok_or_else(|| OrchestratorError::Invalid("dependency cycle".into()))?;
        let mut keyed: Vec<(usize, usize)> =
            plan.tasks.iter().enumerate().map(|(i, t)| (order.get(&t.key).copied().unwrap_or(0), i)).collect();
        keyed.sort_unstable();
        let mut created: BTreeMap<String, orkestr_domain::TaskId> = BTreeMap::new();
        let mut created_ids = Vec::new();
        for (_, i) in keyed {
            let t = &plan.tasks[i];
            let mut tags = vec!["manager".to_owned(), cap(&role_name(&t.role), 40)];
            if let Some(e) = &t.epic {
                tags.push(cap(&format!("epic:{e}"), 40));
            }
            let mut description = t.description.clone();
            if let Some(f) = &t.feature {
                // Stored text: written in the UI language of the moment (like the Manager's notices).
                let label =
                    if crate::util::is_ru(&self.deps.config.ui_language.get()) { "Функция" } else { "Feature" };
                description = format!("{description}\n\n_{label}: {f}_").trim().to_owned();
            }
            let params = CreateTaskParams {
                project_id: plan.project_id,
                title: t.title.clone(),
                description: Some(cap(&description, orkestr_protocol::methods::tasks::MAX_DESCRIPTION_LEN)),
                acceptance_criteria: Some(t.acceptance_criteria.clone()),
                priority: Some(t.priority),
                depends_on: Some(t.depends_on.iter().filter_map(|d| created.get(d).copied()).collect()),
                tags: Some(tags),
                files: (!t.files.is_empty()).then(|| t.files.clone()),
                ..CreateTaskParams::default()
            };
            match self.deps.tasks.create(params, TaskActor::Manager).await {
                Ok(task) => {
                    created.insert(t.key.clone(), task.id);
                    created_ids.push(task.id);
                }
                Err(e) => {
                    // Roll back: never leave half a plan behind.
                    for id in created_ids.iter().rev() {
                        if let Err(d) = self.deps.tasks.delete(*id).await {
                            tracing::warn!(task_id = %id, error = %d, "rollback of plan task failed");
                        }
                    }
                    return Err(OrchestratorError::Invalid(format!("could not create task `{}`: {e}", t.title)));
                }
            }
        }
        for t in &mut plan.tasks {
            t.task_id = created.get(&t.key).copied();
        }
        plan.max_parallelism = max_parallelism(&plan.tasks);
        plan.parallelism = plan.parallelism.clamp(1, plan.max_parallelism.max(1));
        plan.status = PlanStatus::Approved;
        plan.updated_at = now();
        self.save_plan(&plan).await?;
        self.system_event(
            plan.project_id,
            &{
                let titles = one_line(&plan.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join("; "), 400);
                if crate::util::is_ru(&self.deps.config.ui_language.get()) {
                    let n = plan.tasks.len();
                    format!(
                        "План подтверждён: {} {n} {} с зависимостями — {titles}",
                        crate::util::ru_plural(n, "создана", "созданы", "создано"),
                        crate::util::ru_plural(n, "задача", "задачи", "задач"),
                    )
                } else {
                    format!("Plan approved — {} task(s) created with their dependencies: {titles}", plan.tasks.len())
                }
            },
            MessageExtras { plan_id: Some(plan.id), ..MessageExtras::default() },
        )
        .await;
        tracing::info!(plan_id = %plan.id, tasks = plan.tasks.len(), "manager plan approved");
        Ok(plan)
    }

    /// Supersedes older proposals of a project.
    pub async fn supersede_proposals(&self, project: ProjectId, keep: ManagerPlanId) -> OrchResult<()> {
        for mut p in self.store.plans(project, Some(vec![PlanStatus::Proposed]), 50).await? {
            if p.id == keep {
                continue;
            }
            p.status = PlanStatus::Superseded;
            p.updated_at = now();
            self.save_plan(&p).await?;
        }
        Ok(())
    }
}

/// Extractive summary of a conversation (user requests, plans, runs, last
/// Manager answer). Never model reasoning.
#[must_use]
pub fn summarize_messages(messages: &[ManagerChatMessage]) -> String {
    let mut s = String::new();
    let users: Vec<&ManagerChatMessage> = messages.iter().filter(|m| m.role == ManagerMessageRole::User).collect();
    s.push_str(&format!("{} message(s), {} request(s) from the user.\n", messages.len(), users.len()));
    if !users.is_empty() {
        s.push_str("\nRequests:\n");
        for m in users.iter().rev().take(8).rev() {
            s.push_str(&format!("- {}\n", one_line(&m.text, 160)));
        }
    }
    let events: Vec<&ManagerChatMessage> =
        messages.iter().filter(|m| m.role == ManagerMessageRole::SystemEvent).collect();
    if !events.is_empty() {
        s.push_str("\nEvents:\n");
        for m in events.iter().rev().take(8).rev() {
            s.push_str(&format!("- {}\n", one_line(&m.text, 160)));
        }
    }
    if let Some(last) = messages.iter().rev().find(|m| m.role == ManagerMessageRole::Manager) {
        s.push_str(&format!("\nLast Manager answer: {}\n", one_line(&last.text, 300)));
    }
    s
}
