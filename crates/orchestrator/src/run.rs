//! Supervised execution of an approved plan (§1 steps 5–13, §6 dynamic
//! spawn, §11 parallelism within Phase 1 limits).
//!
//! One supervisor task per run: it promotes root tasks to `Ready`, asks the
//! task-engine scheduler which ready tasks may start (parallel caps, provider
//! slots, CPU/RAM, file overlap), spawns an isolated agent per task (own
//! worktree based on its prerequisite's branch, approval-mode and budget
//! caps), watches completions (events + a tick), accepts results according
//! to the approval policy, retries failures within bounds and finishes with a
//! report and a merge-proposal list. Hard bounds: max parallel agents, max
//! agents in total, max cost, time limit, max consecutive failures; pause /
//! resume / cancel; emergency stop is honoured through the project's
//! cancellation token. Run state is persisted on every change, so a core
//! restart leaves an `Interrupted` run that resumes from the task states.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_agent_runtime::AgentError;
use orkestr_domain::{
    now, AgentRole, Budget, BudgetPeriod, ManagerRunId, Project, TaskActor, TaskStatus, WorktreePolicy,
};
use orkestr_protocol::methods::agents::{AgentStopMode, SpawnAgentParams};
use orkestr_protocol::methods::manager::{
    ManagerPlan, ManagerRun, ManagerRunParams, ManagerRunStatus, MergeProposal, PlanStatus, RunLimits, RunLimitsPatch,
    RunReport, RunTask,
};
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_protocol::methods::vcs::DiffTarget;
use orkestr_protocol::methods::scheduler::WaitReason;
use orkestr_task_engine::SchedTask;
use parking_lot::Mutex;
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::parallel::{model_key, Candidate, RiskItem};
use crate::service::{Inner, Manager, MessageExtras};
use crate::util::{cap, one_line, role_name};

/// Why a supervisor was asked to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopKind {
    /// User cancel: stop agents, cancel unfinished tasks.
    Cancel,
    /// Core shutdown: leave everything for resume.
    Shutdown,
}

/// Control block of a live run.
pub struct RunControl {
    /// Emergency token (child of the project's token).
    pub cancel: CancellationToken,
    /// Wakes the supervisor for a stop request.
    pub stop_token: CancellationToken,
    stop: Mutex<Option<StopKind>>,
    /// Pause flag.
    pub paused: watch::Sender<bool>,
    /// Wake-up nudge (pause/resume, external changes).
    pub wake: Notify,
    /// `true` once the supervisor has exited.
    pub finished: watch::Sender<bool>,
}

impl RunControl {
    fn new(cancel: CancellationToken, paused: bool) -> Self {
        Self {
            cancel,
            stop_token: CancellationToken::new(),
            stop: Mutex::new(None),
            paused: watch::channel(paused).0,
            wake: Notify::new(),
            finished: watch::channel(false).0,
        }
    }

    /// Requests a stop (first request wins).
    pub fn request_stop(&self, kind: StopKind) {
        let mut s = self.stop.lock();
        if s.is_none() {
            *s = Some(kind);
        }
        drop(s);
        self.stop_token.cancel();
    }

    fn stop_kind(&self) -> Option<StopKind> {
        self.stop.lock().clone()
    }
}

/// Validated limits from Manager settings + request options + overrides.
pub fn resolve_limits(
    cfg: &orkestr_domain::ManagerConfig,
    plan: &ManagerPlan,
    patch: Option<&RunLimitsPatch>,
    defaults: &crate::deps::ManagerTunables,
    base: Option<&RunLimits>,
) -> OrchResult<RunLimits> {
    let mut l = base.cloned().unwrap_or(RunLimits {
        max_parallel_agents: plan.options.max_parallel_agents.unwrap_or(cfg.max_parallel_agents).max(1),
        max_agents: defaults.default_max_agents.max(u32::try_from(plan.tasks.len()).unwrap_or(u32::MAX)),
        max_cost_usd: plan.options.budget_usd.or(cfg.budget_usd),
        time_limit_mins: plan.options.time_limit_mins.or(cfg.time_limit_mins),
        max_consecutive_failures: defaults.default_max_consecutive_failures,
        max_attempts_per_task: defaults.default_max_attempts,
    });
    if let Some(p) = patch {
        if let Some(v) = p.max_parallel_agents {
            l.max_parallel_agents = v;
        }
        if let Some(v) = p.max_agents {
            l.max_agents = v;
        }
        if let Some(v) = p.max_cost_usd {
            l.max_cost_usd = Some(v);
        }
        if let Some(v) = p.time_limit_mins {
            l.time_limit_mins = Some(v);
        }
        if let Some(v) = p.max_consecutive_failures {
            l.max_consecutive_failures = v;
        }
        if let Some(v) = p.max_attempts_per_task {
            l.max_attempts_per_task = v;
        }
    }
    if !(1..=64).contains(&l.max_parallel_agents) {
        return Err(OrchestratorError::Invalid("maxParallelAgents must be 1..=64".into()));
    }
    if !(1..=500).contains(&l.max_agents) {
        return Err(OrchestratorError::Invalid("maxAgents must be 1..=500".into()));
    }
    if l.max_cost_usd.is_some_and(|c| !c.is_finite() || c < 0.0) {
        return Err(OrchestratorError::Invalid("maxCostUsd must be a non-negative number".into()));
    }
    if l.time_limit_mins.is_some_and(|t| t == 0 || t > 1440) {
        return Err(OrchestratorError::Invalid("timeLimitMins must be 1..=1440".into()));
    }
    if !(1..=50).contains(&l.max_consecutive_failures) {
        return Err(OrchestratorError::Invalid("maxConsecutiveFailures must be 1..=50".into()));
    }
    if !(1..=5).contains(&l.max_attempts_per_task) {
        return Err(OrchestratorError::Invalid("maxAttemptsPerTask must be 1..=5".into()));
    }
    Ok(l)
}

const ACTIVE: [TaskStatus; 2] = [TaskStatus::Running, TaskStatus::Waiting];

impl Manager {
    /// Starts a run for an approved plan (`planId`) or continues a paused,
    /// interrupted or failed run (`runId`).
    pub async fn start_run(&self, p: ManagerRunParams) -> OrchResult<ManagerRun> {
        let _g = self.inner.decide.lock().await;
        match (p.plan_id, p.run_id) {
            (Some(plan_id), None) => self.inner.clone_arc().new_run(plan_id, p.limits).await,
            (None, Some(run_id)) => self.inner.clone_arc().continue_run(run_id, p.limits).await,
            _ => Err(OrchestratorError::Invalid("give exactly one of planId or runId".into())),
        }
    }

    /// Pauses a live run: no new agents; running chat agents pause at their
    /// next turn boundary.
    pub async fn pause(&self, id: ManagerRunId) -> OrchResult<ManagerRun> {
        let control = self.inner.runs.lock().get(&id).cloned();
        let run = self.inner.store.run(id).await?;
        let Some(control) = control else {
            return Err(OrchestratorError::Conflict(format!("the run is {:?}, not running", run.status)));
        };
        let _ = control.paused.send(true);
        control.wake.notify_one();
        self.wait_run(id, Duration::from_secs(5), |r| r.status != ManagerRunStatus::Running).await
    }

    /// Resumes a paused run, or continues an interrupted / failed one.
    pub async fn resume(&self, id: ManagerRunId) -> OrchResult<ManagerRun> {
        let control = self.inner.runs.lock().get(&id).cloned();
        match control {
            Some(c) => {
                let _ = c.paused.send(false);
                c.wake.notify_one();
                self.wait_run(id, Duration::from_secs(5), |r| r.status != ManagerRunStatus::Paused).await
            }
            None => self.start_run(ManagerRunParams { plan_id: None, run_id: Some(id), limits: None }).await,
        }
    }

    /// Cancels a run: stops its agents (their tasks are cancelled) and
    /// cancels the tasks that did not finish.
    pub async fn cancel(&self, id: ManagerRunId) -> OrchResult<ManagerRun> {
        let control = self.inner.runs.lock().get(&id).cloned();
        match control {
            Some(c) => {
                c.request_stop(StopKind::Cancel);
                let mut rx = c.finished.subscribe();
                let _ = tokio::time::timeout(Duration::from_secs(20), rx.wait_for(|d| *d)).await;
                self.inner.store.run(id).await
            }
            None => {
                let mut run = self.inner.store.run(id).await?;
                if run.status.is_final() {
                    return Err(OrchestratorError::Conflict(format!("the run is already {:?}", run.status)));
                }
                let plan = self.inner.store.plan(run.plan_id).await?;
                let project = self.inner.project(run.project_id).await?;
                let control = Arc::new(RunControl::new(CancellationToken::new(), false));
                let mut sup =
                    Supervisor::new(Arc::clone(&self.inner), plan, project, run.clone(), control, Instant::now());
                sup.cancel_tasks("cancelled by the user").await;
                sup.refresh().await;
                sup.finish(ManagerRunStatus::Cancelled, Some("cancelled by the user".into())).await;
                run = self.inner.store.run(id).await?;
                Ok(run)
            }
        }
    }
}

impl Inner {
    fn clone_arc(self: &Arc<Self>) -> Arc<Self> {
        Arc::clone(self)
    }

    async fn new_run(
        self: Arc<Self>,
        plan_id: orkestr_domain::ManagerPlanId,
        patch: Option<RunLimitsPatch>,
    ) -> OrchResult<ManagerRun> {
        let mut plan = self.store.plan(plan_id).await?;
        if plan.status != PlanStatus::Approved {
            return Err(OrchestratorError::Conflict(format!(
                "the plan is {:?}; approve it first (only approved plans can be run)",
                plan.status
            )));
        }
        if let Some(active) = self.active_run(plan.project_id).await? {
            if matches!(active.status, ManagerRunStatus::Running | ManagerRunStatus::Paused) {
                return Err(OrchestratorError::Conflict(
                    "another Manager run is active in this project — pause, finish or cancel it first".into(),
                ));
            }
        }
        let project = self.project(plan.project_id).await?;
        let cfg = self.config().await;
        let limits = resolve_limits(&cfg, &plan, patch.as_ref(), &self.deps.config, None)?;
        let cancel = self
            .deps
            .security
            .emergency
            .register_project(project.id)
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        let mut tasks = Vec::new();
        for t in &plan.tasks {
            let Some(task_id) = t.task_id else { continue };
            let task = self.deps.tasks.get(task_id).await?;
            tasks.push(RunTask {
                task_id,
                key: t.key.clone(),
                title: task.title.clone(),
                role: t.role.clone(),
                status: task.status,
                agent_id: None,
                attempts: 0,
                branch: None,
                worktree_path: None,
                cost_usd: task.usage.cost_usd,
                files_changed: Vec::new(),
                last_error: None,
                awaiting_review: false,
                summary: None,
            });
        }
        if tasks.is_empty() {
            return Err(OrchestratorError::Invalid("the plan has no tasks to run".into()));
        }
        let at = now();
        let run = ManagerRun {
            id: ManagerRunId::new(),
            project_id: project.id,
            plan_id: plan.id,
            goal: plan.goal.clone(),
            status: ManagerRunStatus::Running,
            status_reason: None,
            limits,
            approval_policy: self.policy(&cfg),
            tasks,
            agents_spawned: 0,
            peak_parallel: 0,
            consecutive_failures: 0,
            cost_usd: 0.0,
            active_secs: 0,
            started_at: at,
            updated_at: at,
            finished_at: None,
            report: None,
        };
        plan.status = PlanStatus::Running;
        plan.run_id = Some(run.id);
        plan.updated_at = at;
        self.save_plan(&plan).await?;
        self.save_run(&run).await;
        let text = if crate::util::is_ru(&self.deps.config.ui_language.get()) {
            let (n, p) = (run.tasks.len(), run.limits.max_parallel_agents as usize);
            format!(
                "Запуск начат: {n} {}, параллельно — до {p} {}{}{}.",
                crate::util::ru_plural(n, "задача", "задачи", "задач"),
                crate::util::ru_plural(p, "агента", "агентов", "агентов"),
                run.limits.max_cost_usd.map(|c| format!(", бюджет {c:.2} $")).unwrap_or_default(),
                run.limits.time_limit_mins.map(|m| format!(", ограничение по времени {m} мин")).unwrap_or_default()
            )
        } else {
            format!(
                "Run started — {} task(s), up to {} agent(s) in parallel{}{}.",
                run.tasks.len(),
                run.limits.max_parallel_agents,
                run.limits.max_cost_usd.map(|c| format!(", budget ${c:.2}")).unwrap_or_default(),
                run.limits.time_limit_mins.map(|m| format!(", time limit {m} min")).unwrap_or_default()
            )
        };
        self.system_event(
            project.id,
            &text,
            MessageExtras { plan_id: Some(plan.id), run_id: Some(run.id), ..MessageExtras::default() },
        )
        .await;
        tracing::info!(run_id = %run.id, plan_id = %plan.id, "manager run started");
        let control = Arc::new(RunControl::new(cancel, false));
        Ok(self.spawn_supervisor(plan, project, run, control))
    }

    async fn continue_run(
        self: Arc<Self>,
        run_id: ManagerRunId,
        patch: Option<RunLimitsPatch>,
    ) -> OrchResult<ManagerRun> {
        if self.runs.lock().contains_key(&run_id) {
            return Err(OrchestratorError::Conflict(
                "the run is already live — use manager.resume to unpause it".into(),
            ));
        }
        let mut run = self.store.run(run_id).await?;
        if matches!(run.status, ManagerRunStatus::Completed | ManagerRunStatus::Cancelled) {
            return Err(OrchestratorError::Conflict(format!("the run is {:?} and cannot continue", run.status)));
        }
        if let Some(active) = self.active_run(run.project_id).await? {
            if active.id != run.id && matches!(active.status, ManagerRunStatus::Running | ManagerRunStatus::Paused) {
                return Err(OrchestratorError::Conflict("another Manager run is active in this project".into()));
            }
        }
        let mut plan = self.store.plan(run.plan_id).await?;
        let project = self.project(run.project_id).await?;
        let cfg = self.config().await;
        run.limits = resolve_limits(&cfg, &plan, patch.as_ref(), &self.deps.config, Some(&run.limits))?;
        let cancel = self
            .deps
            .security
            .emergency
            .register_project(project.id)
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        // Unfinished work gets a fresh chance (attempt counters are kept).
        for t in &mut run.tasks {
            let Ok(task) = self.deps.tasks.get(t.task_id).await else { continue };
            if task.status == TaskStatus::Interrupted {
                // Resume from the last checkpoint: the new agent's worktree is
                // based on the interrupted agent's branch (its handoff is
                // injected by the runtime).
                if let Some(b) = t.branch.clone() {
                    self.parallel.mark_resume(t.task_id, b);
                }
            }
            if matches!(task.status, TaskStatus::Failed | TaskStatus::Interrupted) {
                match self.deps.tasks.retry(t.task_id, TaskActor::Manager).await {
                    Ok(c) => t.status = c.task.status,
                    Err(e) => t.last_error = Some(one_line(&e.to_string(), 300)),
                }
            }
            t.awaiting_review = false;
        }
        let previous = run.status;
        run.status = ManagerRunStatus::Running;
        run.status_reason = None;
        run.consecutive_failures = 0;
        run.finished_at = None;
        run.report = None;
        run.updated_at = now();
        plan.status = PlanStatus::Running;
        plan.run_id = Some(run.id);
        plan.updated_at = now();
        self.save_plan(&plan).await?;
        self.save_run(&run).await;
        self.system_event(
            project.id,
            &if crate::util::is_ru(&self.deps.config.ui_language.get()) {
                format!("Запуск возобновлён (был {previous:?}) — продолжаю с текущих состояний задач.")
            } else {
                format!("Run resumed (was {previous:?}) — continuing from the current task states.")
            },
            MessageExtras { plan_id: Some(plan.id), run_id: Some(run.id), ..MessageExtras::default() },
        )
        .await;
        let control = Arc::new(RunControl::new(cancel, false));
        Ok(self.spawn_supervisor(plan, project, run, control))
    }

    fn spawn_supervisor(
        self: Arc<Self>,
        plan: ManagerPlan,
        project: Project,
        run: ManagerRun,
        control: Arc<RunControl>,
    ) -> ManagerRun {
        let id = run.id;
        self.runs.lock().insert(id, Arc::clone(&control));
        let snapshot = run.clone();
        let inner = Arc::clone(&self);
        tokio::spawn(async move {
            let sup = Supervisor::new(Arc::clone(&inner), plan, project, run, Arc::clone(&control), Instant::now());
            sup.drive().await;
            inner.runs.lock().remove(&id);
            inner.parallel.unregister(id);
            inner.parallel.publish_if_changed().await;
            let _ = control.finished.send(true);
        });
        snapshot
    }
}

/// The supervisor of one live run.
struct Supervisor {
    inner: Arc<Inner>,
    plan: ManagerPlan,
    project: Project,
    run: ManagerRun,
    control: Arc<RunControl>,
    segment_start: Instant,
    base_active_secs: u64,
    git_ready: Option<bool>,
    agents_paused: bool,
    last_published: Option<ManagerRun>,
    notes: Vec<String>,
    budget_hit: bool,
    /// Agents whose end was already accounted for (failure streak, retry).
    handled: BTreeSet<orkestr_domain::AgentId>,
}

impl Supervisor {
    fn new(
        inner: Arc<Inner>,
        plan: ManagerPlan,
        project: Project,
        run: ManagerRun,
        control: Arc<RunControl>,
        segment_start: Instant,
    ) -> Self {
        let base_active_secs = run.active_secs;
        Self {
            inner,
            plan,
            project,
            run,
            control,
            segment_start,
            base_active_secs,
            git_ready: None,
            agents_paused: false,
            last_published: None,
            notes: Vec::new(),
            budget_hit: false,
            handled: BTreeSet::new(),
        }
    }

    fn root(&self) -> Option<PathBuf> {
        self.project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir())
    }

    async fn publish_if_changed(&mut self) {
        self.run.active_secs = self.base_active_secs + self.segment_start.elapsed().as_secs();
        let mut cmp = self.run.clone();
        cmp.updated_at = self.last_published.as_ref().map_or(cmp.updated_at, |p| p.updated_at);
        cmp.active_secs = self.last_published.as_ref().map_or(cmp.active_secs, |p| p.active_secs);
        if self.last_published.as_ref() == Some(&cmp) {
            return;
        }
        self.run.updated_at = now();
        self.inner.save_run(&self.run).await;
        self.last_published = Some(self.run.clone());
    }

    async fn drive(mut self) {
        let mut sub = match self.inner.deps.events.subscribe([orkestr_protocol::topic::project(self.project.id)]) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "manager run: event subscription failed; polling only");
                None
            }
        };
        self.promote_roots().await;
        loop {
            if let Some(kind) = self.control.stop_kind() {
                match kind {
                    StopKind::Cancel => {
                        self.stop_agents(AgentStopMode::Cancel).await;
                        self.cancel_tasks("cancelled by the user").await;
                        self.refresh().await;
                        self.finish(ManagerRunStatus::Cancelled, Some("cancelled by the user".into())).await;
                    }
                    StopKind::Shutdown => {
                        self.refresh().await;
                        let paused = *self.control.paused.borrow();
                        self.run.status = ManagerRunStatus::Interrupted;
                        self.run.status_reason = Some(if paused {
                            crate::parallel::PAUSED_INTERRUPTED_REASON.into()
                        } else {
                            "Orkestr stopped while the run was active — resume it to continue".into()
                        });
                        self.publish_if_changed().await;
                    }
                }
                return;
            }
            if self.control.cancel.is_cancelled() {
                // Emergency stop: agents were cancelled by the same controller.
                // Let them record their own outcome (their tasks become
                // Cancelled), then cancel what never started so the final
                // run card shows no task as still running.
                self.wait_agents_stopped(Duration::from_secs(10)).await;
                self.cancel_tasks("emergency stop").await;
                self.refresh().await;
                self.finish(ManagerRunStatus::Cancelled, Some("emergency stop".into())).await;
                return;
            }
            self.refresh().await;
            self.accept_results().await;
            if let Some((status, reason)) = self.check_bounds() {
                self.stop_agents(AgentStopMode::Interrupt).await;
                self.refresh().await;
                self.finish(status, Some(reason)).await;
                return;
            }
            if let Some((status, reason)) = self.check_done() {
                self.finish(status, reason).await;
                return;
            }
            let paused = *self.control.paused.borrow();
            self.apply_pause(paused).await;
            if paused {
                self.run.status = ManagerRunStatus::Paused;
                self.run.status_reason = Some("paused by the user — no new agents start".into());
                let git = self.ensure_git().await;
                let candidates = self.candidates(git).await;
                let cap = self.run.limits.max_parallel_agents as usize;
                self.inner.parallel.hold(self.run.id, self.project.id, cap, candidates, WaitReason::Paused);
                self.inner.parallel.publish_if_changed().await;
            } else {
                self.run.status = ManagerRunStatus::Running;
                if let Some(fatal) = self.schedule().await {
                    self.stop_agents(AgentStopMode::Interrupt).await;
                    self.refresh().await;
                    self.finish(ManagerRunStatus::Failed, Some(fatal)).await;
                    return;
                }
                self.run.status_reason = self.waiting_reason();
            }
            self.publish_if_changed().await;
            let tick = self.inner.deps.config.tick;
            tokio::select! {
                () = async {
                    match sub.as_mut() {
                        Some(s) => loop {
                            match s.recv().await {
                                Some(orkestr_events::Received::Event(ev)) => {
                                    if matches!(ev.kind_name(), "TaskStatusChanged" | "AgentStopped" | "TaskUpdated" | "ApprovalResolved") {
                                        break;
                                    }
                                }
                                Some(orkestr_events::Received::Lagged(_)) => break,
                                None => std::future::pending::<()>().await,
                            }
                        },
                        None => std::future::pending::<()>().await,
                    }
                } => {}
                () = tokio::time::sleep(tick) => {}
                () = self.control.wake.notified() => {}
                () = self.control.cancel.cancelled() => {}
                () = self.control.stop_token.cancelled() => {}
            }
        }
    }

    /// `Planned` tasks without prerequisites become `Ready`.
    async fn promote_roots(&mut self) {
        let keys: BTreeMap<&str, &Vec<String>> =
            self.plan.tasks.iter().map(|t| (t.key.as_str(), &t.depends_on)).collect();
        for t in &self.run.tasks {
            let Ok(task) = self.inner.deps.tasks.get(t.task_id).await else { continue };
            let root = keys.get(t.key.as_str()).is_none_or(|d| d.is_empty()) && task.dependencies.is_empty();
            if task.status == TaskStatus::Planned && root {
                if let Err(e) = self
                    .inner
                    .deps
                    .tasks
                    .transition(
                        t.task_id,
                        TaskStatus::Ready,
                        TaskActor::Manager,
                        Some("manager run started".into()),
                        None,
                    )
                    .await
                {
                    tracing::warn!(task_id = %t.task_id, error = %e, "could not promote a root task");
                }
            }
        }
    }

    /// Reloads task states, agent facts and costs; handles finished agents
    /// (failure streaks, retries).
    async fn refresh(&mut self) {
        let max_attempts = self.run.limits.max_attempts_per_task;
        let mut total = 0.0;
        for i in 0..self.run.tasks.len() {
            let tid = self.run.tasks[i].task_id;
            let Ok(task) = self.inner.deps.tasks.get(tid).await else {
                let rt = &mut self.run.tasks[i];
                if rt.status != TaskStatus::Cancelled {
                    rt.status = TaskStatus::Cancelled;
                    rt.last_error = Some("the task was deleted".into());
                }
                continue;
            };
            let agent = self.run.tasks[i].agent_id;
            let agent_live = agent.is_some_and(|a| self.inner.deps.agents.is_live(a));
            {
                let rt = &mut self.run.tasks[i];
                rt.status = task.status;
                rt.title = task.title.clone();
                rt.cost_usd = task.usage.cost_usd;
                rt.files_changed = task.files_touched.clone();
                if let Some(r) = &task.result {
                    rt.summary = Some(cap(&r.summary, 2000));
                }
                if let Some(w) = &task.worktree {
                    rt.worktree_path = Some(w.path.clone());
                    if w.branch.starts_with("agent/") {
                        rt.branch = Some(w.branch.clone());
                    }
                }
            }
            total += task.usage.cost_usd;
            let unhandled = agent.is_some_and(|a| !agent_live && !self.handled.contains(&a));
            let ended = matches!(task.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Interrupted);
            if unhandled && ended {
                if let Some(a) = agent {
                    self.handled.insert(a);
                }
                match task.status {
                    TaskStatus::Completed => {
                        self.run.consecutive_failures = 0;
                        self.run.tasks[i].last_error = None;
                    }
                    TaskStatus::Failed | TaskStatus::Interrupted if self.run.tasks[i].agent_id.is_some() => {
                        self.run.consecutive_failures += 1;
                        let why = task.result.as_ref().map(|r| one_line(&r.summary, 300));
                        self.run.tasks[i].last_error = why.or_else(|| Some(format!("ended {:?}", task.status)));
                        if self.run.limits.max_cost_usd.is_some()
                            && task.result.as_ref().is_some_and(|r| r.summary.to_ascii_lowercase().contains("budget"))
                        {
                            self.budget_hit = true;
                        }
                        let attempts = self.run.tasks[i].attempts;
                        let retry = task.status == TaskStatus::Failed
                            && attempts < max_attempts
                            && self.run.consecutive_failures < self.run.limits.max_consecutive_failures
                            && !self.limit_failure(&task);
                        if retry {
                            match self.inner.deps.tasks.retry(tid, TaskActor::Manager).await {
                                Ok(c) => {
                                    self.run.tasks[i].status = c.task.status;
                                    self.inner
                                        .system_event(
                                            self.project.id,
                                            &if crate::util::is_ru(&self.inner.deps.config.ui_language.get()) {
                                                format!(
                                                    "Задача «{}» не выполнена (попытка {attempts} из {max_attempts}) — повторяю с новым агентом.",
                                                    task.title
                                                )
                                            } else {
                                                format!(
                                                    "Task \"{}\" failed (attempt {attempts} of {max_attempts}) — retrying with a new agent.",
                                                    task.title
                                                )
                                            },
                                            MessageExtras { run_id: Some(self.run.id), ..MessageExtras::default() },
                                        )
                                        .await;
                                }
                                Err(e) => tracing::warn!(task_id = %tid, error = %e, "retry failed"),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.run.cost_usd = (total * 1_000_000.0).round() / 1_000_000.0;
    }

    /// A failure caused by a budget/turn/time limit is not retried blindly.
    fn limit_failure(&self, task: &orkestr_domain::Task) -> bool {
        task.result.as_ref().is_some_and(|r| {
            let s = r.summary.to_ascii_lowercase();
            s.contains("budget") || s.contains("turn limit") || s.contains("wall-clock")
        })
    }

    /// `Review` / `Testing` results: accepted automatically when the agent
    /// reported success and the approval policy is not `Ask`; otherwise they
    /// wait for the user (Tasks panel / run card → Completed or Failed).
    async fn accept_results(&mut self) {
        for i in 0..self.run.tasks.len() {
            let rt = &self.run.tasks[i];
            if !matches!(rt.status, TaskStatus::Review | TaskStatus::Testing) {
                self.run.tasks[i].awaiting_review = false;
                continue;
            }
            if rt.agent_id.is_some_and(|a| self.inner.deps.agents.is_live(a)) {
                continue;
            }
            let Ok(task) = self.inner.deps.tasks.get(rt.task_id).await else { continue };
            let success = task.result.as_ref().is_some_and(|r| r.success);
            if success && self.run.approval_policy != orkestr_domain::ApprovalMode::Ask {
                let reason = "accepted by the AI Manager: the agent reported done (automated review/test loops arrive in Phase 2)";
                match self
                    .inner
                    .deps
                    .tasks
                    .transition(task.id, TaskStatus::Completed, TaskActor::Manager, Some(reason.into()), None)
                    .await
                {
                    Ok(c) => {
                        self.run.tasks[i].status = c.task.status;
                        self.run.tasks[i].awaiting_review = false;
                        self.run.consecutive_failures = 0;
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, error = %e, "could not accept a task result");
                        self.run.tasks[i].awaiting_review = true;
                    }
                }
            } else if !self.run.tasks[i].awaiting_review {
                self.run.tasks[i].awaiting_review = true;
                let text = if crate::util::is_ru(&self.inner.deps.config.ui_language.get()) {
                    let why = if success {
                        "режим подтверждений — «Спрашивать»"
                    } else {
                        "агент сообщил о частичном результате"
                    };
                    format!(
                        "Задача «{}» ждёт вашей проверки ({why}). Примите её (→ «Выполнена») или отметьте как «Сбой», чтобы повторить.",
                        task.title
                    )
                } else {
                    let why =
                        if success { "the approval policy is Ask" } else { "the agent reported a partial result" };
                    format!(
                        "Task \"{}\" is waiting for your review ({why}). Accept it (→ Completed) or mark it Failed to retry.",
                        task.title
                    )
                };
                self.inner
                    .system_event(
                        self.project.id,
                        &text,
                        MessageExtras { run_id: Some(self.run.id), ..MessageExtras::default() },
                    )
                    .await;
            }
        }
    }

    fn live_agents(&self) -> usize {
        self.run.tasks.iter().filter(|t| t.agent_id.is_some_and(|a| self.inner.deps.agents.is_live(a))).count()
    }

    fn check_bounds(&self) -> Option<(ManagerRunStatus, String)> {
        let l = &self.run.limits;
        if let Some(max) = l.max_cost_usd {
            if self.budget_hit {
                return Some((
                    ManagerRunStatus::Failed,
                    format!(
                        "budget exhausted: an agent could not continue within the remaining run budget (${:.4} spent of ${max:.2})",
                        self.run.cost_usd
                    ),
                ));
            }
            if self.run.cost_usd >= max && (max > 0.0 || self.run.cost_usd > 0.0) {
                return Some((
                    ManagerRunStatus::Failed,
                    format!("budget exhausted: ${:.4} spent of ${max:.2}", self.run.cost_usd),
                ));
            }
        }
        if let Some(mins) = l.time_limit_mins {
            let secs = self.base_active_secs + self.segment_start.elapsed().as_secs();
            if secs >= u64::from(mins) * 60 {
                return Some((ManagerRunStatus::Failed, format!("time limit of {mins} min reached")));
            }
        }
        if self.run.consecutive_failures >= l.max_consecutive_failures {
            return Some((
                ManagerRunStatus::Failed,
                format!(
                    "{} consecutive task failures (limit {})",
                    self.run.consecutive_failures, l.max_consecutive_failures
                ),
            ));
        }
        None
    }

    fn check_done(&self) -> Option<(ManagerRunStatus, Option<String>)> {
        let terminal = |s: TaskStatus| matches!(s, TaskStatus::Completed | TaskStatus::Cancelled);
        if self.run.tasks.iter().all(|t| terminal(t.status)) {
            let cancelled = self.run.tasks.iter().filter(|t| t.status == TaskStatus::Cancelled).count();
            return Some(if cancelled == 0 {
                (ManagerRunStatus::Completed, None)
            } else {
                (ManagerRunStatus::Failed, Some(format!("{cancelled} task(s) were cancelled")))
            });
        }
        if self.live_agents() > 0 {
            return None;
        }
        let progressing = self.run.tasks.iter().any(|t| {
            matches!(t.status, TaskStatus::Ready | TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Recovering)
                || t.awaiting_review
                || (matches!(t.status, TaskStatus::Planned | TaskStatus::Blocked) && self.deps_satisfiable(&t.key))
        });
        if progressing {
            return None;
        }
        let stuck: Vec<String> = self
            .run
            .tasks
            .iter()
            .filter(|t| !terminal(t.status))
            .map(|t| format!("\"{}\" ({:?})", t.title, t.status))
            .collect();
        Some((ManagerRunStatus::Failed, Some(format!("no task can proceed: {}", cap(&stuck.join(", "), 600)))))
    }

    /// Whether a planned task's prerequisites can still complete.
    fn deps_satisfiable(&self, key: &str) -> bool {
        let Some(pt) = self.plan.tasks.iter().find(|t| t.key == key) else { return false };
        pt.depends_on.iter().all(|d| {
            self.run.tasks.iter().find(|t| &t.key == d).is_some_and(|t| {
                !matches!(t.status, TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted)
            })
        })
    }

    fn waiting_reason(&self) -> Option<String> {
        let review = self.run.tasks.iter().filter(|t| t.awaiting_review).count();
        let waiting = self.run.tasks.iter().filter(|t| t.status == TaskStatus::Waiting).count();
        let mut parts = Vec::new();
        if review > 0 {
            parts.push(format!("{review} task(s) wait for your review"));
        }
        if waiting > 0 {
            parts.push(format!("{waiting} agent(s) wait for an approval"));
        }
        parts.extend(self.notes.iter().take(2).cloned());
        if let Some(w) = self.inner.parallel.run_wait_summary(self.run.id) {
            parts.push(w);
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    async fn apply_pause(&mut self, paused: bool) {
        if paused == self.agents_paused {
            return;
        }
        self.agents_paused = paused;
        let ids: Vec<_> =
            self.run.tasks.iter().filter_map(|t| t.agent_id).filter(|a| self.inner.deps.agents.is_live(*a)).collect();
        for a in ids {
            let r = if paused { self.inner.deps.agents.pause(a).await } else { self.inner.deps.agents.resume(a).await };
            if let Err(e) = r {
                tracing::info!(agent_id = %a, error = %e, "agent could not be paused/resumed (keeps running)");
            }
        }
    }

    async fn stop_agents(&mut self, mode: AgentStopMode) {
        let ids: Vec<_> =
            self.run.tasks.iter().filter_map(|t| t.agent_id).filter(|a| self.inner.deps.agents.is_live(*a)).collect();
        for a in ids {
            if let Err(e) = self.inner.deps.agents.stop(a, mode).await {
                tracing::warn!(agent_id = %a, error = %e, "could not stop a run agent");
            }
        }
    }

    /// Waits (bounded) until no agent of the run is live any more.
    async fn wait_agents_stopped(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        while self.live_agents() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Cancels every unfinished task of the run.
    async fn cancel_tasks(&mut self, why: &str) {
        for t in &self.run.tasks {
            let Ok(task) = self.inner.deps.tasks.get(t.task_id).await else { continue };
            let target = match task.status {
                TaskStatus::Completed | TaskStatus::Cancelled => continue,
                TaskStatus::Running | TaskStatus::Waiting => continue, // the agent's stop handles these
                _ => TaskStatus::Cancelled,
            };
            if let Err(e) =
                self.inner.deps.tasks.transition(task.id, target, TaskActor::Manager, Some(why.into()), None).await
            {
                tracing::warn!(task_id = %task.id, error = %e, "could not cancel a run task");
            }
        }
    }

    async fn ensure_git(&mut self) -> bool {
        if let Some(g) = self.git_ready {
            return g;
        }
        let ok = match self.root() {
            Some(root) => {
                let git = self.inner.deps.vcs.git();
                git.is_available() && matches!(git.rev_parse(&root, "HEAD").await, Ok(Some(_)))
            }
            None => false,
        };
        if !ok {
            self.notes.push(
                "the project is not a git repository with a commit — agents share the checkout instead of own worktrees"
                    .into(),
            );
        }
        self.git_ready = Some(ok);
        ok
    }

    /// Ready tasks of the run as scheduler candidates: predicted (plan) ∪
    /// touched files, where each will run (own worktree with git, else the
    /// shared checkout), predicted provider/model and team tag.
    async fn candidates(&self, git: bool) -> Vec<Candidate> {
        let mut out = Vec::new();
        for rt in &self.run.tasks {
            if rt.status != TaskStatus::Ready || rt.agent_id.is_some_and(|a| self.inner.deps.agents.is_live(a)) {
                continue;
            }
            let Ok(task) = self.inner.deps.tasks.get(rt.task_id).await else { continue };
            let template = self.template_for(&rt.role).await;
            let read_only = template.as_ref().is_some_and(|t| t.worktree_policy == WorktreePolicy::ReadOnlyMain)
                && (!git || self.base_branch(&rt.key).0.is_none());
            let mut s = SchedTask::from_task(&task);
            if let Some(pt) = self.plan.tasks.iter().find(|p| p.key == rt.key) {
                for f in &pt.files {
                    if !s.files.contains(f) {
                        s.files.push(f.clone());
                    }
                }
            }
            if read_only {
                s.files.clear();
            }
            // With git every writing agent gets its own worktree: no same-file
            // conflict, but overlaps are flagged as merge risks.
            s.worktree = (git && !read_only).then(|| format!("pending-worktree:{}", task.id));
            let model = self
                .inner
                .parallel
                .predict_model(
                    &self.inner.deps.models,
                    &self.project,
                    &rt.role,
                    self.plan.options.model.clone(),
                    template.as_ref().and_then(|t| t.preferred_model.clone()),
                )
                .await;
            if let Some(m) = &model {
                s.provider = Some(m.provider_id.to_string());
                s.model = Some(model_key(m));
            }
            out.push(Candidate { sched: s, title: task.title.clone() });
        }
        out
    }

    /// Scheduler pass: asks the global scheduler which ready tasks may start
    /// (caps, provider/model slots, rate limits, resources, file conflicts,
    /// fairness across projects) and starts agents for them. Returns a fatal
    /// reason when the run cannot continue (no model, forbidden, …).
    async fn schedule(&mut self) -> Option<String> {
        let live = self.live_agents();
        let max_parallel = self.run.limits.max_parallel_agents as usize;
        let git = self.ensure_git().await;
        let candidates = self.candidates(git).await;
        // Provider rate-limit events feed the back-off while runs are live.
        self.inner.parallel.spawn_rate_listener();
        if !candidates.is_empty() && self.run.agents_spawned >= self.run.limits.max_agents {
            if live == 0 {
                return Some(format!("agent limit reached ({} agents spawned)", self.run.agents_spawned));
            }
            let why = WaitReason::AgentBudget { spawned: self.run.agents_spawned, limit: self.run.limits.max_agents };
            self.inner.parallel.hold(self.run.id, self.project.id, max_parallel, candidates, why);
            self.inner.parallel.publish_if_changed().await;
            return None;
        }
        let active: Vec<SchedTask> = self
            .inner
            .deps
            .tasks
            .list(ListTasksParams { status: Some(ACTIVE.to_vec()), ..ListTasksParams::default() })
            .await
            .unwrap_or_default()
            .iter()
            .map(|t| Inner::active_sched(t, git))
            .collect();
        let outcome = self.inner.parallel.admit(self.run.id, self.project.id, max_parallel, candidates, &active);
        let mut started = 0usize;
        for task_id in outcome.start {
            if live + started >= max_parallel || self.run.agents_spawned >= self.run.limits.max_agents {
                break;
            }
            let Some(i) = self.run.tasks.iter().position(|t| t.task_id == task_id) else { continue };
            match self.spawn_for(i, git).await {
                Ok(()) => started += 1,
                Err(Fatal(reason)) => return Some(reason),
            }
        }
        if let Some((_, first)) = outcome.deferred.first() {
            tracing::debug!(run_id = %self.run.id, deferred = outcome.deferred.len(), first = %first, "tasks held back by the scheduler");
        }
        let parallel = u32::try_from(self.live_agents()).unwrap_or(u32::MAX);
        self.run.peak_parallel = self.run.peak_parallel.max(parallel);
        self.inner.parallel.publish_if_changed().await;
        self.check_merge_risks(git).await;
        None
    }

    /// Flags agents in separate worktrees that touch the same files (early
    /// merge-conflict warning, confirmed with `git merge-tree`).
    async fn check_merge_risks(&mut self, git: bool) {
        let in_flight = |s: TaskStatus| {
            matches!(s, TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Review | TaskStatus::Testing)
        };
        let items: Vec<RiskItem> = self
            .run
            .tasks
            .iter()
            .filter(|t| in_flight(t.status) && t.agent_id.is_some())
            .map(|t| {
                let mut files = t.files_changed.clone();
                if let Some(pt) = self.plan.tasks.iter().find(|p| p.key == t.key) {
                    for f in &pt.files {
                        if !files.contains(f) {
                            files.push(f.clone());
                        }
                    }
                }
                RiskItem { task_id: t.task_id, branch: t.branch.clone(), worktree: t.worktree_path.clone(), files }
            })
            .collect();
        if items.len() < 2 {
            return;
        }
        let fresh = self.inner.parallel.check_merge_risks(&self.project, &items, git).await;
        let ru = crate::util::is_ru(&self.inner.deps.config.ui_language.get());
        for r in fresh {
            let title = |id| self.run.tasks.iter().find(|t| t.task_id == id).map_or_else(String::new, |t| t.title.clone());
            let list = |v: &[String]| v.iter().take(5).map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
            let (a, b) = (title(r.task_a), title(r.task_b));
            let text = match (ru, r.conflicts.is_empty()) {
                (true, true) => format!(
                    "Возможный конфликт слияния: «{a}» и «{b}» меняют одни и те же файлы в разных рабочих копиях (worktree): {}.",
                    list(&r.files)
                ),
                (true, false) => {
                    format!("Конфликт слияния подтверждён git merge-tree: «{a}» и «{b}» ({}).", list(&r.conflicts))
                }
                (false, true) => format!(
                    "Possible merge conflict: \"{a}\" and \"{b}\" change the same files in separate worktrees ({}).",
                    list(&r.files)
                ),
                (false, false) => {
                    format!("Merge conflict confirmed by git merge-tree: \"{a}\" and \"{b}\" ({}).", list(&r.conflicts))
                }
            };
            self.inner
                .system_event(self.project.id, &text, MessageExtras { run_id: Some(self.run.id), ..MessageExtras::default() })
                .await;
        }
    }

    async fn template_for(&self, role: &AgentRole) -> Option<orkestr_domain::AgentTemplate> {
        let templates = self.inner.deps.agents.templates();
        let pick = |list: Vec<orkestr_domain::AgentTemplate>| {
            let official = list.iter().find(|t| t.source == orkestr_domain::TemplateSource::Builtin).cloned();
            official.or_else(|| list.into_iter().next())
        };
        if let Some(t) = templates.list(Some(role.clone()), None).await.ok().and_then(pick) {
            return Some(t);
        }
        templates.list(Some(AgentRole::Backend), None).await.ok().and_then(pick)
    }

    /// Base branch for a task: the branch of its most recently finished
    /// prerequisite (dedicated worktrees only).
    fn base_branch(&self, key: &str) -> (Option<String>, Option<String>) {
        let Some(pt) = self.plan.tasks.iter().find(|t| t.key == key) else { return (None, None) };
        let branches: Vec<&str> = pt
            .depends_on
            .iter()
            .filter_map(|d| self.run.tasks.iter().find(|t| &t.key == d))
            .filter_map(|t| t.branch.as_deref())
            .collect();
        let Some(base) = branches.last() else { return (None, None) };
        let note = (branches.len() > 1).then(|| {
            format!(
                "based on `{base}`; the changes of {} are not merged into it yet (the merge manager arrives in Phase 2)",
                branches[..branches.len() - 1].iter().map(|b| format!("`{b}`")).collect::<Vec<_>>().join(", ")
            )
        });
        (Some((*base).to_owned()), note)
    }

    async fn spawn_for(&mut self, i: usize, git: bool) -> Result<(), Fatal> {
        let (key, role, task_id) = {
            let t = &self.run.tasks[i];
            (t.key.clone(), t.role.clone(), t.task_id)
        };
        let Some(template) = self.template_for(&role).await else {
            return Err(Fatal("no agent template is installed".into()));
        };
        let (base, note) = match (git, self.inner.parallel.take_resume(task_id)) {
            (true, Some(branch)) => {
                let note = format!("resumed from `{branch}` (last checkpoint + handoff)");
                (Some(branch), Some(note))
            }
            (true, None) => self.base_branch(&key),
            (false, _) => (None, None),
        };
        let policy = if git {
            if template.worktree_policy == WorktreePolicy::ReadOnlyMain && base.is_none() {
                WorktreePolicy::ReadOnlyMain
            } else {
                WorktreePolicy::DedicatedWorktree
            }
        } else if template.worktree_policy == WorktreePolicy::ReadOnlyMain {
            WorktreePolicy::ReadOnlyMain
        } else {
            WorktreePolicy::SharedWorktree
        };
        let budget = self.run.limits.max_cost_usd.map(|max| Budget {
            max_cost_usd: Some((max - self.run.cost_usd).max(0.0)),
            ..Budget::unrestricted(BudgetPeriod::PerTask)
        });
        let params = SpawnAgentParams {
            project_id: self.project.id,
            template_id: template.id,
            task_id: Some(task_id),
            new_task: None,
            // Expert-mode model of the request applies to its agents too.
            model: self.plan.options.model.clone(),
            worktree_policy: Some(policy),
            name: None,
            max_turns: None,
            base_branch: base.clone(),
            approval_mode: Some(self.run.approval_policy),
            budget,
        };
        match self.inner.deps.agents.spawn(params).await {
            Ok((agent, _task)) => {
                self.run.agents_spawned += 1;
                let rt = &mut self.run.tasks[i];
                rt.agent_id = Some(agent.id);
                rt.attempts += 1;
                rt.last_error = None;
                if let Some(w) = &agent.worktree {
                    rt.worktree_path = Some(w.path.clone());
                    if policy == WorktreePolicy::DedicatedWorktree && !w.branch.is_empty() {
                        rt.branch = Some(w.branch.clone());
                    }
                }
                if let Some(n) = note {
                    rt.last_error = Some(n);
                }
                tracing::info!(run_id = %self.run.id, agent_id = %agent.id, %task_id, ?policy, ?base, "run agent spawned");
                Ok(())
            }
            Err(e) => {
                let msg = one_line(&e.to_string(), 300);
                match e {
                    AgentError::NoModel(_) | AgentError::Forbidden(_) => {
                        Err(Fatal(format!("could not start an agent for \"{}\": {msg}", self.run.tasks[i].title)))
                    }
                    AgentError::Conflict(_) => {
                        // Runtime capacity or a race: try again on the next pass.
                        self.run.tasks[i].last_error = Some(msg);
                        Ok(())
                    }
                    _ => {
                        self.run.consecutive_failures += 1;
                        self.run.tasks[i].attempts += 1;
                        self.run.tasks[i].last_error = Some(format!("could not start an agent: {msg}"));
                        if self.run.tasks[i].attempts >= self.run.limits.max_attempts_per_task {
                            let _ = self
                                .inner
                                .deps
                                .tasks
                                .transition(
                                    task_id,
                                    TaskStatus::Cancelled,
                                    TaskActor::Manager,
                                    Some(cap(&format!("could not start an agent: {msg}"), 1900)),
                                    None,
                                )
                                .await;
                        }
                        Ok(())
                    }
                }
            }
        }
    }

    /// Final state: report, plan status, conversation event.
    async fn finish(&mut self, status: ManagerRunStatus, reason: Option<String>) {
        self.run.status = status;
        self.run.status_reason = reason.clone();
        self.run.finished_at = Some(now());
        self.run.report = Some(self.report().await);
        for t in &mut self.run.tasks {
            t.awaiting_review = false;
        }
        // The conversation notice is stored before the final status becomes
        // visible, so a client reacting to "run finished" sees it in order.
        let summary = self.run.report.as_ref().map(|r| r.summary.clone()).unwrap_or_default();
        self.inner
            .system_event(
                self.project.id,
                &summary,
                MessageExtras { plan_id: Some(self.plan.id), run_id: Some(self.run.id), ..MessageExtras::default() },
            )
            .await;
        self.publish_if_changed().await;
        if let Ok(mut plan) = self.inner.store.plan(self.plan.id).await {
            plan.status = match status {
                ManagerRunStatus::Completed => PlanStatus::Completed,
                ManagerRunStatus::Cancelled => PlanStatus::Cancelled,
                _ => PlanStatus::Failed,
            };
            plan.updated_at = now();
            if let Err(e) = self.inner.save_plan(&plan).await {
                tracing::warn!(error = %e, "could not update the plan status");
            }
        }
        tracing::info!(run_id = %self.run.id, ?status, ?reason, "manager run finished");
    }

    async fn report(&self) -> RunReport {
        let mut files: BTreeSet<String> = BTreeSet::new();
        let mut tests: Vec<String> = Vec::new();
        let mut issues: Vec<String> = Vec::new();
        let mut merge = Vec::new();
        let root = self.root();
        for t in &self.run.tasks {
            files.extend(t.files_changed.iter().cloned());
            if let Some(a) = t.agent_id {
                if let Ok(hs) = self.inner.deps.agents.handoffs(a).await {
                    if let Some(h) = hs.last() {
                        if let Some(ts) = &h.test_state {
                            tests.push(format!(
                                "{} — `{}`: {}",
                                t.title,
                                ts.command,
                                if ts.passed { "passed" } else { "failed" }
                            ));
                        }
                        issues.extend(h.unresolved_issues.iter().map(|i| format!("{}: {}", t.title, one_line(i, 200))));
                    }
                }
            }
            if matches!(t.status, TaskStatus::Failed | TaskStatus::Interrupted) {
                issues.push(format!(
                    "{} ended {:?}{}",
                    t.title,
                    t.status,
                    t.last_error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()
                ));
            }
            if let (Some(branch), Some(root)) = (&t.branch, &root) {
                if t.status == TaskStatus::Completed {
                    let req = orkestr_vcs::DiffRequest {
                        target: DiffTarget::Range { from: "HEAD".into(), to: branch.clone() },
                        paths: Vec::new(),
                        context_lines: 0,
                        include_untracked: false,
                        max_patch_bytes: 1024,
                    };
                    let (changed, add, del) = match self.inner.deps.vcs.git().diff(root, &req).await {
                        Ok(d) => (
                            d.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
                            Some(d.additions),
                            Some(d.deletions),
                        ),
                        Err(_) => (t.files_changed.clone(), None, None),
                    };
                    merge.push(MergeProposal {
                        task_id: t.task_id,
                        title: t.title.clone(),
                        branch: branch.clone(),
                        files_changed: if changed.is_empty() { t.files_changed.clone() } else { changed },
                        additions: add,
                        deletions: del,
                        note: "review the diff and merge it (automatic merging is Phase 2)".into(),
                    });
                }
            }
        }
        let completed = self.run.tasks.iter().filter(|t| t.status == TaskStatus::Completed).count();
        let failed =
            self.run.tasks.iter().filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Interrupted)).count();
        let not_done = self.run.tasks.len() - completed - failed;
        let secs = self.base_active_secs + self.segment_start.elapsed().as_secs();
        let mut md = format!(
            "**Run {}** — {completed}/{} task(s) completed in {}, ${:.4} spent{}.\n\n",
            match self.run.status {
                ManagerRunStatus::Completed => "completed",
                ManagerRunStatus::Cancelled => "cancelled",
                ManagerRunStatus::Interrupted => "interrupted",
                ManagerRunStatus::Paused => "paused",
                ManagerRunStatus::Running => "running",
                ManagerRunStatus::Failed => "stopped",
            },
            self.run.tasks.len(),
            human_duration(secs),
            self.run.cost_usd,
            self.run.status_reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
        );
        md.push_str("### What was done\n");
        for t in &self.run.tasks {
            md.push_str(&format!(
                "- **{}** ({}) — {:?}{}\n",
                t.title,
                role_name(&t.role),
                t.status,
                t.summary.as_ref().map(|s| format!(": {}", one_line(s, 200))).unwrap_or_default()
            ));
        }
        if !files.is_empty() {
            md.push_str(&format!(
                "\n### Files changed\n{}\n",
                files.iter().take(40).map(|f| format!("- `{f}`")).collect::<Vec<_>>().join("\n")
            ));
        }
        if !tests.is_empty() {
            md.push_str(&format!(
                "\n### Tests\n{}\n",
                tests.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n")
            ));
        }
        if !issues.is_empty() {
            md.push_str(&format!(
                "\n### Open issues\n{}\n",
                issues.iter().take(20).map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n")
            ));
        }
        if !merge.is_empty() {
            md.push_str("\n### Merge proposals (merging is Phase 2)\n");
            for m in &merge {
                md.push_str(&format!(
                    "- `{}` — {} file(s){}\n",
                    m.branch,
                    m.files_changed.len(),
                    match (m.additions, m.deletions) {
                        (Some(a), Some(d)) => format!(", +{a} −{d}"),
                        _ => String::new(),
                    }
                ));
            }
        }
        RunReport {
            summary: md,
            completed: u32::try_from(completed).unwrap_or(u32::MAX),
            failed: u32::try_from(failed).unwrap_or(u32::MAX),
            not_done: u32::try_from(not_done).unwrap_or(u32::MAX),
            files_changed: files.into_iter().collect(),
            tests,
            open_issues: issues,
            cost_usd: self.run.cost_usd,
            duration_secs: secs,
            merge_proposals: merge,
        }
    }
}

struct Fatal(String);

fn human_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(125), "2m 5s");
        assert_eq!(human_duration(3700), "1h 1m");
    }
}
