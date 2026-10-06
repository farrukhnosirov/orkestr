//! The end-to-end quality pipeline of a task (§17, §68.15–16).
//!
//! **Author** (the agent implements in its worktree) → **Review** (automatic
//! code review, [`crate::review`]) → **Build** → **Test** (the quality runner
//! in the task's worktree, through the [`PipelineChecks`] seam the core wires)
//! → **Security** (adversarial review, [`crate::adversarial`]; unresolved
//! critical findings block) → quality gates → completion (by the approval
//! policy) → **Merge** (Merge Manager proposal; merged right away when the
//! merge policy allows it). Stages are configured per project and task kind
//! ([`PipelineConfig`]).
//!
//! The pipeline drives the task state machine through the task engine:
//! `Running → Review → Testing → Completed` (review "changes requested" and
//! failing checks go back to `Running` for a fix — the fix loop). A failure
//! enters the Self-Healing loop ([`crate::healing`]): analysis → fix agent
//! (same model, escalated later) with a briefing → Build → Test → Review →
//! Security again, within hard limits (attempts, wall time, budget, repeated
//! failure signature). When a limit is hit the user decides through an
//! approval (more attempts or fail the task).
//!
//! One driver task per pipeline (restart-safe: the record is persisted on
//! every change, and a driver stopped by a shutdown resumes from the record).
//! Fix agents go through the global parallel scheduler like every agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_agent_runtime::{AgentError, SpawnExtras};
use orkestr_domain::{
    now, AgentId, AgentRole, ApprovalId, ApprovalMode, ApprovalStatus, Budget, BudgetPeriod, Capability, ManagerRunId,
    ModelRef, Project, RiskLevel, Task, TaskActor, TaskId, TaskStatus, WorktreePolicy,
};
use orkestr_protocol::event::{EventKind, HealingAttemptPayload, PipelinePayload, PipelineStagePayload};
use orkestr_protocol::methods::agents::{AgentStopMode, SpawnAgentParams};
use orkestr_protocol::methods::merge::{
    MergeBranchInput, MergeExecuteParams, MergeProposalStatus, MergeProposeParams, MergeRecommendation,
};
use orkestr_protocol::methods::pipeline::{
    FailureAnalysis, GetPipelineResult, HealingAttempt, HealingOutcome, ListPipelinesParams, PipelineConfig,
    PipelineMergeMode, PipelineStage, PipelineStageKind, PipelineStatus, SetPipelineConfigParams, StageStatus,
    TaskPipeline, MAX_HEALING_ATTEMPTS, MAX_PIPELINE_MINS,
};
use orkestr_protocol::methods::quality::{CheckKind, CheckStatus, GateReport, GateStatus, QualityRun};
use orkestr_protocol::methods::reviews::{AdversarialReviewParams, RequestReviewParams, ReviewStatus, ReviewVerdict};
use orkestr_protocol::NewEvent;
use orkestr_security::ApprovalRequest;
use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::healing::{self, HealDecision, HealState};
use crate::pipeline_store::PipelineStore;
use crate::service::{Inner, Manager, MessageExtras};
use crate::util::{cap, is_ru, one_line};

/// Stage order of a fresh result.
const FIRST_ORDER: [PipelineStageKind; 4] =
    [PipelineStageKind::Review, PipelineStageKind::Build, PipelineStageKind::Test, PipelineStageKind::Security];
/// Stage order after a fix (Fix → Build → Test → Review → Security).
const AFTER_FIX_ORDER: [PipelineStageKind; 4] =
    [PipelineStageKind::Build, PipelineStageKind::Test, PipelineStageKind::Review, PipelineStageKind::Security];
/// Check kinds of the Build stage.
pub const BUILD_KINDS: [CheckKind; 2] = [CheckKind::Build, CheckKind::Typecheck];
/// Check kinds of the Test stage.
pub const TEST_KINDS: [CheckKind; 1] = [CheckKind::Test];
/// How long the Merge stage follows an automatic merge.
const MERGE_WAIT: Duration = Duration::from_secs(30 * 60);
/// Max characters of a status reason.
const MAX_REASON: usize = 600;

// ---------------------------------------------------------------------------
// seam
// ---------------------------------------------------------------------------

/// Quality checks and gates for the pipeline (the core implements it over
/// the quality runner).
#[async_trait]
pub trait PipelineChecks: Send + Sync {
    /// Runs the project's checks of `kinds` in `workspace` for the task and
    /// waits for the result. `Ok(None)` = no check of these kinds is configured.
    async fn run_checks(
        &self,
        project: &Project,
        task: &Task,
        agent: Option<AgentId>,
        workspace: &Path,
        kinds: &[CheckKind],
        cancel: &CancellationToken,
    ) -> Result<Option<QualityRun>, String>;

    /// Evaluates the project's quality gates for the task.
    async fn gates(&self, project: &Project, task: &Task) -> Option<GateReport>;
}

/// Why a driver was asked to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriverStop {
    /// Run cancelled / bounds hit: the pipeline ends `Cancelled`.
    Cancel,
    /// Core shutdown: the record stays resumable.
    Shutdown,
}

/// Control block of a live driver.
pub(crate) struct DriverHandle {
    cancel: CancellationToken,
    stop: Mutex<Option<DriverStop>>,
    run_id: Option<ManagerRunId>,
    done: watch::Sender<bool>,
}

impl DriverHandle {
    pub(crate) fn request_stop(&self, kind: DriverStop) {
        let mut s = self.stop.lock();
        if s.is_none() {
            *s = Some(kind);
        }
        drop(s);
        self.cancel.cancel();
    }
}

/// Pipeline state of the Manager service.
#[derive(Default)]
pub(crate) struct PipelineSlots {
    checks: RwLock<Option<Arc<dyn PipelineChecks>>>,
    merger: RwLock<Option<crate::merge::MergeManager>>,
    live: Mutex<HashMap<TaskId, Arc<DriverHandle>>>,
}

/// What a driver needs from its caller.
pub(crate) struct PipelineCtx {
    pub task_id: TaskId,
    pub run_id: Option<ManagerRunId>,
    pub role: Option<AgentRole>,
    pub policy: ApprovalMode,
    pub model: Option<ModelRef>,
    /// Remaining budget of the run (USD) when the pipeline starts.
    pub budget_usd: Option<f64>,
    pub paused: Option<watch::Receiver<bool>>,
    /// Emergency token (the driver's own token is a child of it).
    pub parent_cancel: CancellationToken,
}

// ---------------------------------------------------------------------------
// service API
// ---------------------------------------------------------------------------

impl Manager {
    /// Wires the quality runner (core composition root).
    pub fn set_pipeline_checks(&self, checks: Arc<dyn PipelineChecks>) {
        *self.inner.pipeline.checks.write() = Some(checks);
    }

    /// Wires the Merge Manager (core composition root). A merge that
    /// completes later (e.g. the run-wide proposal) is mirrored on the Merge
    /// stage of the pipelines of its branches.
    pub fn set_merge_manager(&self, merger: crate::merge::MergeManager) {
        merger.on_completed(Arc::new(PipelineMergeHook { inner: Arc::downgrade(&self.inner) }));
        *self.inner.pipeline.merger.write() = Some(merger);
    }

    /// `pipeline.get`.
    pub async fn pipeline(&self, task: TaskId) -> OrchResult<GetPipelineResult> {
        self.inner.deps.tasks.get(task).await?;
        let mut all = self.inner.pipeline_store().for_task(task, 20).await?;
        let pipeline = if all.is_empty() { None } else { Some(all.remove(0)) };
        Ok(GetPipelineResult { pipeline, history: all, live: self.inner.pipeline_live(task) })
    }

    /// `pipeline.list`.
    pub async fn list_pipelines(&self, p: ListPipelinesParams) -> OrchResult<Vec<TaskPipeline>> {
        self.inner.project(p.project_id).await?;
        self.inner.pipeline_store().list(p.project_id, p.run_id, p.limit.unwrap_or(50).clamp(1, 500)).await
    }

    /// `pipeline.start`: runs the pipeline for a task whose agent finished
    /// (`Review` / `Testing`), outside a Manager run.
    pub async fn start_pipeline(&self, task_id: TaskId) -> OrchResult<TaskPipeline> {
        let task = self.inner.deps.tasks.get(task_id).await?;
        if !matches!(task.status, TaskStatus::Review | TaskStatus::Testing) {
            return Err(OrchestratorError::Conflict(format!(
                "the task is {:?}; the pipeline starts when its agent finished (Review or Testing)",
                task.status
            )));
        }
        if task.agent_id.is_some_and(|a| self.inner.deps.agents.is_live(a)) {
            return Err(OrchestratorError::Conflict("the task's agent is still working".into()));
        }
        let project = self.inner.project(task.project_id).await?;
        let cancel = self.inner.quality_token(project.id)?;
        let cfg = self.inner.config().await;
        let ctx = PipelineCtx {
            task_id,
            run_id: None,
            role: None,
            policy: self.inner.policy(&cfg),
            model: None,
            budget_usd: None,
            paused: None,
            parent_cancel: cancel,
        };
        self.inner
            .launch_pipeline(ctx)
            .await?
            .ok_or_else(|| OrchestratorError::Conflict("a pipeline is already running for this task".into()))
    }

    /// `pipeline.getConfig`.
    pub async fn pipeline_config(&self, project: orkestr_domain::ProjectId) -> OrchResult<PipelineConfig> {
        self.inner.project(project).await?;
        self.inner.pipeline_config(project).await
    }

    /// `pipeline.setConfig`.
    pub async fn set_pipeline_config(&self, p: SetPipelineConfigParams) -> OrchResult<PipelineConfig> {
        self.inner.project(p.project_id).await?;
        let store = self.inner.pipeline_store();
        if p.reset.unwrap_or(false) {
            store.reset_config(p.project_id).await?;
            return Ok(PipelineConfig::defaults(p.project_id));
        }
        let mut cfg = self.inner.pipeline_config(p.project_id).await?;
        if let Some(e) = p.enabled {
            cfg.enabled = e;
        }
        if let Some(s) = p.stages {
            cfg.stages = normalize_stages(&s);
        }
        if let Some(o) = p.overrides {
            if o.len() > 32 {
                return Err(OrchestratorError::Invalid("at most 32 task-kind overrides".into()));
            }
            let mut out = Vec::new();
            for mut ov in o {
                ov.kind = ov.kind.trim().to_owned();
                if ov.kind.is_empty() || ov.kind.chars().count() > 60 {
                    return Err(OrchestratorError::Invalid(
                        "an override needs a task kind of 1..=60 characters".into(),
                    ));
                }
                ov.stages = normalize_stages(&ov.stages);
                out.push(ov);
            }
            cfg.overrides = out;
        }
        if let Some(h) = p.healing {
            if h.max_attempts > MAX_HEALING_ATTEMPTS {
                return Err(OrchestratorError::Invalid(format!("maxAttempts must be 0..={MAX_HEALING_ATTEMPTS}")));
            }
            if h.max_wall_mins == 0 || h.max_wall_mins > MAX_PIPELINE_MINS {
                return Err(OrchestratorError::Invalid(format!("maxWallMins must be 1..={MAX_PIPELINE_MINS}")));
            }
            if h.max_budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
                return Err(OrchestratorError::Invalid("maxBudgetUsd must be a non-negative amount".into()));
            }
            cfg.healing = h;
        }
        if let Some(m) = p.merge {
            cfg.merge = m;
        }
        cfg.customized = true;
        cfg.updated_at = Some(now());
        store.save_config(&cfg).await?;
        Ok(cfg)
    }

    /// Waits until no driver works on the task (tests, CLI) and returns its
    /// latest pipeline.
    pub async fn wait_pipeline(&self, task: TaskId, timeout: Duration) -> OrchResult<Option<TaskPipeline>> {
        let deadline = tokio::time::Instant::now() + timeout;
        while self.inner.pipeline_live(task) {
            if tokio::time::Instant::now() >= deadline {
                return Err(OrchestratorError::Conflict("timed out waiting for the pipeline".into()));
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        self.inner.pipeline_store().latest(task).await
    }
}

/// Author first, canonical order, no duplicates.
fn normalize_stages(s: &[PipelineStageKind]) -> Vec<PipelineStageKind> {
    PipelineStageKind::ALL.iter().copied().filter(|k| *k == PipelineStageKind::Author || s.contains(k)).collect()
}

impl Inner {
    pub(crate) fn pipeline_store(&self) -> PipelineStore {
        PipelineStore::new(self.deps.db.clone())
    }

    /// Saved configuration or the defaults.
    pub(crate) async fn pipeline_config(&self, project: orkestr_domain::ProjectId) -> OrchResult<PipelineConfig> {
        Ok(self.pipeline_store().config(project).await?.unwrap_or_else(|| PipelineConfig::defaults(project)))
    }

    /// Whether a driver works on the task.
    pub(crate) fn pipeline_live(&self, task: TaskId) -> bool {
        self.pipeline.live.lock().contains_key(&task)
    }

    /// Asks the drivers of a run (or every driver) to stop and waits (bounded).
    pub(crate) async fn stop_pipelines(&self, run: Option<ManagerRunId>, kind: DriverStop) {
        let handles: Vec<Arc<DriverHandle>> =
            self.pipeline.live.lock().values().filter(|h| run.is_none() || h.run_id == run).cloned().collect();
        for h in &handles {
            h.request_stop(kind);
        }
        for h in handles {
            let mut rx = h.done.subscribe();
            let _ = tokio::time::timeout(Duration::from_secs(15), rx.wait_for(|d| *d)).await;
        }
    }

    /// Creates (or resumes) the task's pipeline and starts its driver.
    /// `Ok(None)` when a driver already works on the task.
    pub(crate) async fn launch_pipeline(self: &Arc<Self>, ctx: PipelineCtx) -> OrchResult<Option<TaskPipeline>> {
        let task = self.deps.tasks.get(ctx.task_id).await?;
        let project = self.project(task.project_id).await?;
        let cfg = self.pipeline_config(project.id).await?;
        let handle = Arc::new(DriverHandle {
            cancel: ctx.parent_cancel.child_token(),
            stop: Mutex::new(None),
            run_id: ctx.run_id,
            done: watch::channel(false).0,
        });
        {
            let mut live = self.pipeline.live.lock();
            if live.contains_key(&task.id) {
                return Ok(None);
            }
            live.insert(task.id, Arc::clone(&handle));
        }
        let store = self.pipeline_store();
        let latest = match store.latest(task.id).await {
            Ok(l) => l,
            Err(e) => {
                self.pipeline.live.lock().remove(&task.id);
                return Err(e);
            }
        };
        let kinds = task_kinds(&task, ctx.role.as_ref());
        let enabled = cfg.stages_for(&kinds);
        let resumable = latest.as_ref().filter(|p| {
            !p.status.is_final()
                && task
                    .agent_id
                    .is_some_and(|a| p.author_agent_id == Some(a) || p.healing.iter().any(|h| h.agent_id == Some(a)))
        });
        let pipeline = match resumable {
            Some(p) => {
                let mut p = p.clone();
                p.run_id = ctx.run_id.or(p.run_id);
                for s in &mut p.stages {
                    if s.status == StageStatus::Running {
                        s.status = StageStatus::Pending;
                    }
                }
                if let Some(h) = p.healing.last_mut() {
                    if matches!(h.outcome, HealingOutcome::Queued | HealingOutcome::Running) {
                        h.outcome = HealingOutcome::Stopped;
                        h.finished_at = Some(now());
                    }
                }
                p
            }
            None => {
                if let Some(mut old) = latest.filter(|p| !p.status.is_final()) {
                    old.status = PipelineStatus::Cancelled;
                    old.status_reason = Some("superseded by a new attempt of the task".into());
                    old.finished_at = Some(now());
                    let _ = store.save(&old).await;
                }
                let at = now();
                TaskPipeline {
                    id: uuid::Uuid::now_v7().to_string(),
                    project_id: project.id,
                    task_id: task.id,
                    run_id: ctx.run_id,
                    status: PipelineStatus::Running,
                    status_reason: None,
                    stages: PipelineStageKind::ALL
                        .iter()
                        .map(|k| {
                            let mut s = PipelineStage::pending(*k);
                            if !enabled.contains(k) {
                                s.status = StageStatus::Skipped;
                                s.detail = Some("disabled for this task kind".into());
                            }
                            s
                        })
                        .collect(),
                    healing: Vec::new(),
                    limits: cfg.healing.clone(),
                    granted_attempts: 0,
                    author_agent_id: task.agent_id,
                    author_model: task.model.clone(),
                    branch: task.worktree.as_ref().map(|w| w.branch.clone()).filter(|b| b.starts_with("agent/")),
                    approval_id: None,
                    review_verdict: None,
                    security_verdict: None,
                    gates: None,
                    merge_proposal_id: None,
                    merge_outcome: None,
                    cost_at_start_usd: task.usage.cost_usd,
                    cost_usd: task.usage.cost_usd,
                    started_at: at,
                    updated_at: at,
                    finished_at: None,
                }
            }
        };
        if let Err(e) = store.save(&pipeline).await {
            self.pipeline.live.lock().remove(&task.id);
            return Err(e);
        }
        self.publish_pipeline(&pipeline).await;
        let git = git_ready(self, &project).await;
        let driver = Driver {
            inner: Arc::clone(self),
            cancel: handle.cancel.clone(),
            handle: Arc::clone(&handle),
            ctx,
            project,
            cfg,
            enabled,
            p: pipeline.clone(),
            git,
            sched_id: ManagerRunId::new(),
            forced: false,
        };
        let inner = Arc::clone(self);
        let task_id = task.id;
        tokio::spawn(async move {
            driver.drive().await;
            inner.pipeline.live.lock().remove(&task_id);
            let _ = handle.done.send(true);
        });
        Ok(Some(pipeline))
    }

    async fn publish_pipeline(&self, p: &TaskPipeline) {
        self.deps.publish(NewEvent::new(EventKind::PipelineUpdated(PipelinePayload { pipeline: p.clone() }))).await;
    }
}

/// Marks the Merge stage of the merged branches' pipelines `Passed`.
struct PipelineMergeHook {
    inner: std::sync::Weak<Inner>,
}

#[async_trait::async_trait]
impl crate::merge::MergeCompletedHook for PipelineMergeHook {
    async fn merged(&self, proposal: &orkestr_protocol::methods::merge::MergeManagerProposal) {
        let Some(inner) = self.inner.upgrade() else { return };
        for b in &proposal.branches {
            let Some(task) = b.task_id else { continue };
            let Ok(Some(mut p)) = inner.pipeline_store().latest(task).await else { continue };
            if p.branch.as_deref() != Some(b.branch.as_str()) {
                continue;
            }
            let Some(stage) = p.stages.iter_mut().find(|s| s.kind == PipelineStageKind::Merge) else { continue };
            if stage.status == StageStatus::Passed {
                continue;
            }
            let outcome = format!("merged `{}` into `{}`", b.branch, proposal.target);
            stage.status = StageStatus::Passed;
            stage.finished_at = Some(now());
            stage.detail = Some(cap(&format!("{outcome} (proposal {})", proposal.id), MAX_REASON));
            stage.ref_id = Some(proposal.id.to_string());
            let payload = PipelineStagePayload {
                pipeline_id: p.id.clone(),
                project_id: p.project_id,
                task_id: p.task_id,
                stage: PipelineStageKind::Merge,
                status: StageStatus::Passed,
                runs: stage.runs,
                detail: stage.detail.clone(),
            };
            p.merge_outcome = Some(cap(&outcome, MAX_REASON));
            p.merge_proposal_id = Some(proposal.id);
            p.updated_at = now();
            if let Err(e) = inner.pipeline_store().save(&p).await {
                tracing::warn!(pipeline = %p.id, error = %e, "could not record the merge on the pipeline");
                continue;
            }
            inner.deps.publish(NewEvent::new(EventKind::PipelineStageChanged(payload))).await;
            inner.publish_pipeline(&p).await;
            // A finished run's card and report mirror the merge too (a live
            // run's supervisor copies the pipeline itself).
            let Some(run_id) = p.run_id else { continue };
            let Ok(mut run) = inner.store.run(run_id).await else { continue };
            use orkestr_protocol::methods::manager::ManagerRunStatus as Rs;
            if matches!(run.status, Rs::Running | Rs::Paused) {
                continue;
            }
            if let Some(t) = run.tasks.iter_mut().find(|t| t.task_id == p.task_id) {
                t.pipeline = Some(orkestr_protocol::methods::pipeline::RunTaskPipeline::from(&p));
            }
            if let Some(r) = run.report.as_mut() {
                if let Some(pr) = r.pipelines.iter_mut().find(|x| x.task_id == p.task_id) {
                    let title = pr.title.clone();
                    *pr = report_of(&p, &title);
                }
                if let Some(m) = r.merge_proposals.iter_mut().find(|m| m.task_id == p.task_id) {
                    m.note = cap(&outcome, 300);
                }
            }
            inner.save_run(&run).await;
        }
    }
}

/// Role name + tags of a task (the "task kind" of the configuration).
fn task_kinds(task: &Task, role: Option<&AgentRole>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(r) = role {
        out.push(crate::util::role_name(r));
    }
    out.extend(task.tags.iter().cloned());
    out
}

async fn git_ready(inner: &Inner, project: &Project) -> bool {
    let Some(root) = project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir()) else { return false };
    let git = inner.deps.vcs.git();
    git.is_available() && matches!(git.rev_parse(&root, "HEAD").await, Ok(Some(_)))
}

// ---------------------------------------------------------------------------
// driver
// ---------------------------------------------------------------------------

/// How a pipeline ended.
#[derive(Debug)]
enum End {
    Passed,
    Failed(String),
    Cancelled(String),
    /// Shutdown: the record stays resumable.
    Stopped,
}

/// Result of one stage.
enum Step {
    Done,
    Failed(Box<FailureAnalysis>),
    End(End),
}

/// Outcome of waiting for the user.
enum UserDecision {
    Accepted,
    End(End),
}

/// Outcome of one fix attempt.
enum FixResult {
    Submitted,
    AgentFailed,
    End(End),
}

struct Driver {
    inner: Arc<Inner>,
    cancel: CancellationToken,
    handle: Arc<DriverHandle>,
    ctx: PipelineCtx,
    project: Project,
    cfg: PipelineConfig,
    enabled: Vec<PipelineStageKind>,
    p: TaskPipeline,
    git: bool,
    sched_id: ManagerRunId,
    forced: bool,
}

impl Driver {
    fn ru(&self) -> bool {
        is_ru(&self.inner.deps.config.ui_language.get())
    }

    fn manager(&self) -> Manager {
        Manager { inner: Arc::clone(&self.inner) }
    }

    fn stopped(&self) -> End {
        match *self.handle.stop.lock() {
            Some(DriverStop::Shutdown) => End::Stopped,
            Some(DriverStop::Cancel) => End::Cancelled("stopped with the run".into()),
            None => End::Cancelled("emergency stop".into()),
        }
    }

    async fn task(&self) -> Result<Task, End> {
        self.inner.deps.tasks.get(self.p.task_id).await.map_err(|e| End::Cancelled(format!("the task is gone: {e}")))
    }

    async fn save(&mut self) {
        if let Ok(t) = self.inner.deps.tasks.get(self.p.task_id).await {
            self.p.cost_usd = t.usage.cost_usd;
        }
        self.p.updated_at = now();
        if let Err(e) = self.inner.pipeline_store().save(&self.p).await {
            tracing::error!(pipeline = %self.p.id, error = %e, "failed to persist a pipeline");
            return;
        }
        self.inner.publish_pipeline(&self.p).await;
    }

    async fn set_status(&mut self, status: PipelineStatus, reason: Option<String>) {
        self.p.status = status;
        self.p.status_reason = reason.map(|r| cap(&r, MAX_REASON));
        self.save().await;
    }

    async fn stage(
        &mut self,
        kind: PipelineStageKind,
        status: StageStatus,
        detail: Option<String>,
        ref_id: Option<String>,
    ) {
        let at = now();
        let Some(s) = self.p.stages.iter_mut().find(|s| s.kind == kind) else { return };
        s.status = status;
        if status == StageStatus::Running {
            s.runs += 1;
            s.started_at = Some(at);
            s.finished_at = None;
        } else if status != StageStatus::Pending {
            s.finished_at = Some(at);
        }
        if detail.is_some() || status == StageStatus::Running {
            s.detail = detail.map(|d| cap(&d, MAX_REASON));
        }
        if ref_id.is_some() {
            s.ref_id = ref_id;
        }
        let payload = PipelineStagePayload {
            pipeline_id: self.p.id.clone(),
            project_id: self.p.project_id,
            task_id: self.p.task_id,
            stage: kind,
            status,
            runs: s.runs,
            detail: s.detail.clone(),
        };
        self.inner.deps.publish(NewEvent::new(EventKind::PipelineStageChanged(payload))).await;
        self.save().await;
    }

    fn enabled(&self, kind: PipelineStageKind) -> bool {
        self.enabled.contains(&kind)
    }

    /// Moves the task (no-op when it is already there). `false` on failure.
    async fn move_task(&self, to: TaskStatus, reason: &str) -> bool {
        let Ok(task) = self.inner.deps.tasks.get(self.p.task_id).await else { return false };
        if task.status == to {
            return true;
        }
        match self.inner.deps.tasks.transition(task.id, to, TaskActor::Manager, Some(cap(reason, 400)), None).await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(task_id = %task.id, ?to, error = %e, "pipeline: task transition failed");
                false
            }
        }
    }

    /// Waits while the run is paused. `Err` when stopped.
    async fn wait_unpaused(&mut self) -> Result<(), End> {
        if self.cancel.is_cancelled() {
            return Err(self.stopped());
        }
        let Some(mut rx) = self.ctx.paused.clone() else { return Ok(()) };
        if !*rx.borrow() {
            return Ok(());
        }
        tokio::select! {
            r = rx.wait_for(|p| !*p) => { let _ = r; }
            () = self.cancel.cancelled() => return Err(self.stopped()),
        }
        Ok(())
    }

    async fn notice(&self, en: String, ru: String) {
        let text = if self.ru() { ru } else { en };
        self.inner
            .system_event(self.p.project_id, &text, MessageExtras { run_id: self.p.run_id, ..MessageExtras::default() })
            .await;
    }

    async fn drive(mut self) {
        let end = self.go().await;
        self.finish(end).await;
    }

    async fn finish(&mut self, end: End) {
        for s in &mut self.p.stages {
            if s.status == StageStatus::Running {
                s.status = StageStatus::Pending;
            }
        }
        if let Some(h) = self.p.healing.last_mut() {
            if matches!(h.outcome, HealingOutcome::Queued | HealingOutcome::Running) {
                h.outcome = HealingOutcome::Stopped;
                h.finished_at = Some(now());
            }
        }
        self.inner.parallel.unregister(self.sched_id);
        match end {
            End::Passed => {
                if let Some(h) = self.p.healing.last_mut() {
                    if h.outcome == HealingOutcome::Submitted {
                        h.outcome = HealingOutcome::Fixed;
                        h.finished_at = Some(now());
                    }
                }
                self.p.finished_at = Some(now());
                let healed = self.p.healing.len();
                self.set_status(PipelineStatus::Passed, None).await;
                if healed > 0 {
                    let title = self.task().await.map(|t| t.title).unwrap_or_default();
                    self.notice(
                        format!("Self-healing fixed \"{title}\" after {healed} attempt(s) — every quality stage passed."),
                        format!("Самовосстановление исправило задачу «{title}» (попыток: {healed}) — все этапы качества пройдены."),
                    )
                    .await;
                }
            }
            End::Failed(r) => {
                if let Some(h) = self.p.healing.last_mut() {
                    if h.outcome == HealingOutcome::Submitted {
                        h.outcome = HealingOutcome::StillFailing;
                    }
                }
                self.p.finished_at = Some(now());
                self.set_status(PipelineStatus::Failed, Some(r)).await;
            }
            End::Cancelled(r) => {
                self.p.finished_at = Some(now());
                self.set_status(PipelineStatus::Cancelled, Some(r)).await;
            }
            End::Stopped => {
                let reason = Some("interrupted — resumes with the run".to_owned());
                let status = self.p.status;
                self.set_status(status, reason).await;
            }
        }
        tracing::info!(pipeline = %self.p.id, task = %self.p.task_id, status = ?self.p.status, "quality pipeline finished");
    }

    async fn go(&mut self) -> End {
        let task = match self.task().await {
            Ok(t) => t,
            Err(e) => return e,
        };
        // Resume an open decision.
        if self.p.status == PipelineStatus::AwaitingUser {
            let reason = self.p.status_reason.clone().unwrap_or_else(|| "waiting for your decision".into());
            return match self.await_user(reason).await {
                UserDecision::Accepted => self.merge().await,
                UserDecision::End(e) => e,
            };
        }
        if let Some(id) = self.p.approval_id.take().and_then(|a| ApprovalId::parse(&a).ok()) {
            let _ = self.inner.deps.security.approvals.cancel(id, "superseded: the pipeline resumed").await;
        }
        // Author.
        if self.p.stage(PipelineStageKind::Author).is_some_and(|s| s.status != StageStatus::Passed) {
            self.stage(PipelineStageKind::Author, StageStatus::Running, None, None).await;
            let ok = task.result.as_ref().is_some_and(|r| r.success);
            let summary = task.result.as_ref().map(|r| one_line(&r.summary, 300));
            let agent = task.agent_id.map(|a| a.to_string());
            if ok {
                self.stage(PipelineStageKind::Author, StageStatus::Passed, summary, agent).await;
            } else {
                self.stage(
                    PipelineStageKind::Author,
                    StageStatus::Failed,
                    Some(summary.unwrap_or_else(|| "the agent reported a partial result".into())),
                    agent,
                )
                .await;
                return match self.await_user("the agent reported a partial result — review it".into()).await {
                    UserDecision::Accepted => self.merge().await,
                    UserDecision::End(e) => e,
                };
            }
        }
        if self.p.status != PipelineStatus::Running {
            self.set_status(PipelineStatus::Running, None).await;
        }
        let mut order: &[PipelineStageKind] = if self.p.healing.is_empty() { &FIRST_ORDER } else { &AFTER_FIX_ORDER };
        loop {
            match self.stages(order).await {
                Ok(()) => break,
                Err(Step::Failed(a)) => {
                    if let Err(e) = self.heal(*a).await {
                        return e;
                    }
                    order = &AFTER_FIX_ORDER;
                }
                Err(Step::End(e)) => return e,
                Err(Step::Done) => break,
            }
        }
        // Gates, then completion by the approval policy.
        if let Some(why) = self.gates().await {
            return match self.await_user(why).await {
                UserDecision::Accepted => self.merge().await,
                UserDecision::End(e) => e,
            };
        }
        if self.ctx.policy == ApprovalMode::Ask {
            return match self.await_user("every quality stage passed — waiting for your acceptance".into()).await {
                UserDecision::Accepted => self.merge().await,
                UserDecision::End(e) => e,
            };
        }
        if !self.move_task(TaskStatus::Completed, "quality pipeline passed (review, build, tests, security)").await {
            let status = self.task().await.map(|t| t.status);
            return match status {
                Ok(TaskStatus::Completed) => self.merge().await,
                Ok(TaskStatus::Cancelled) => End::Cancelled("the task was cancelled".into()),
                Ok(s) => End::Failed(format!("the task could not be completed (it is {s:?})")),
                Err(e) => e,
            };
        }
        self.merge().await
    }

    /// Runs the enabled stages of `order`. `Err(Step::Failed)` hands a
    /// failure to the healing loop.
    async fn stages(&mut self, order: &[PipelineStageKind]) -> Result<(), Step> {
        for &kind in order {
            if !self.enabled(kind) {
                continue;
            }
            self.wait_unpaused().await.map_err(Step::End)?;
            let step = match kind {
                PipelineStageKind::Review => self.review_stage().await,
                PipelineStageKind::Build => self.check_stage(kind, &BUILD_KINDS).await,
                PipelineStageKind::Test => self.check_stage(kind, &TEST_KINDS).await,
                PipelineStageKind::Security => self.security_stage().await,
                _ => Step::Done,
            };
            match step {
                Step::Done => {}
                other => return Err(other),
            }
        }
        Ok(())
    }

    async fn review_stage(&mut self) -> Step {
        let kind = PipelineStageKind::Review;
        if !self.git || self.p.branch.is_none() {
            let why = if self.git {
                "the task has no agent branch — there is no change to review"
            } else {
                "no git repository — there is no change to review"
            };
            self.stage(kind, StageStatus::Skipped, Some(why.into()), None).await;
            return Step::Done;
        }
        match self.task().await {
            Ok(t) if t.status == TaskStatus::Testing => {
                self.move_task(TaskStatus::Review, "quality pipeline: code review").await;
            }
            Ok(_) => {}
            Err(e) => return Step::End(e),
        }
        self.stage(kind, StageStatus::Running, None, None).await;
        let mgr = self.manager();
        let review = tokio::select! {
            r = mgr.request_review(RequestReviewParams {
                task_id: self.p.task_id,
                model: None,
                adversarial: Some(false),
                transition: Some(true),
                wait: Some(true),
            }) => r,
            () = self.cancel.cancelled() => return Step::End(self.stopped()),
        };
        let review = match review {
            Ok(r) => r.review,
            Err(e) => {
                let why = format!("the code review could not run: {e}");
                self.stage(kind, StageStatus::Failed, Some(why.clone()), None).await;
                return self.user_step(why).await;
            }
        };
        let rid = Some(review.id.to_string());
        if review.status != ReviewStatus::Finished {
            let why = format!("the code review failed: {}", review.error.clone().unwrap_or_default());
            self.stage(kind, StageStatus::Failed, Some(why.clone()), rid).await;
            return self.user_step(why).await;
        }
        self.p.review_verdict = review.verdict;
        let detail = Some(one_line(&review.summary, 300));
        match review.verdict {
            Some(ReviewVerdict::Approve) => {
                // apply_review moved Review → Testing; make sure (a task the
                // review did not move stays consistent).
                if self.task().await.is_ok_and(|t| t.status == TaskStatus::Review) {
                    self.move_task(TaskStatus::Testing, "code review approved").await;
                }
                self.stage(kind, StageStatus::Passed, detail, rid).await;
                Step::Done
            }
            Some(ReviewVerdict::Reject) => {
                self.stage(kind, StageStatus::Failed, detail, rid).await;
                // apply_review already failed the task.
                self.move_task(TaskStatus::Failed, "code review rejected the change").await;
                Step::End(End::Failed(format!(
                    "the code review rejected the change: {}",
                    one_line(&review.summary, 300)
                )))
            }
            _ => {
                self.stage(kind, StageStatus::Failed, detail, rid).await;
                let root = self.workspace().await;
                let text = self.task_text().await;
                Step::Failed(Box::new(healing::analyze_review(kind, &review, root.as_deref(), &text)))
            }
        }
    }

    async fn security_stage(&mut self) -> Step {
        let kind = PipelineStageKind::Security;
        if !self.git || self.p.branch.is_none() {
            let why = if self.git {
                "the task has no agent branch — there is no change to attack"
            } else {
                "no git repository — there is no change to attack"
            };
            self.stage(kind, StageStatus::Skipped, Some(why.into()), None).await;
            return Step::Done;
        }
        self.stage(kind, StageStatus::Running, None, None).await;
        let mgr = self.manager();
        let review = tokio::select! {
            r = mgr.adversarial_review(AdversarialReviewParams {
                task_id: self.p.task_id,
                model: None,
                wait: Some(true),
            }) => r,
            () = self.cancel.cancelled() => return Step::End(self.stopped()),
        };
        let review = match review {
            Ok(r) => r.review,
            Err(e) => {
                let why = format!("the adversarial review could not run: {e}");
                self.stage(kind, StageStatus::Failed, Some(why.clone()), None).await;
                return self.user_step(why).await;
            }
        };
        let rid = Some(review.id.to_string());
        if review.status != ReviewStatus::Finished {
            let why = format!("the adversarial review failed: {}", review.error.clone().unwrap_or_default());
            self.stage(kind, StageStatus::Failed, Some(why.clone()), rid).await;
            return self.user_step(why).await;
        }
        self.p.security_verdict = review.verdict;
        let blocking = review.blocking().len();
        if blocking == 0 {
            let detail = format!("{} finding(s), none blocking", review.findings.len());
            self.stage(kind, StageStatus::Passed, Some(detail), rid).await;
            return Step::Done;
        }
        self.stage(kind, StageStatus::Failed, Some(format!("{blocking} unresolved critical finding(s)")), rid).await;
        // Fix loop: Testing → Running happens when the fix agent starts.
        let root = self.workspace().await;
        let text = self.task_text().await;
        Step::Failed(Box::new(healing::analyze_review(kind, &review, root.as_deref(), &text)))
    }

    async fn check_stage(&mut self, kind: PipelineStageKind, kinds: &[CheckKind]) -> Step {
        let checks = self.inner.pipeline.checks.read().clone();
        let Some(checks) = checks else {
            self.stage(kind, StageStatus::Skipped, Some("no quality runner is connected".into()), None).await;
            return Step::Done;
        };
        let Some(workspace) = self.workspace().await else {
            self.stage(kind, StageStatus::Skipped, Some("the task has no workspace on this machine".into()), None)
                .await;
            return Step::Done;
        };
        match self.task().await {
            Ok(t) if t.status == TaskStatus::Review => {
                self.move_task(TaskStatus::Testing, "quality pipeline: build and tests").await;
            }
            Ok(_) => {}
            Err(e) => return Step::End(e),
        }
        let task = match self.task().await {
            Ok(t) => t,
            Err(e) => return Step::End(e),
        };
        self.stage(kind, StageStatus::Running, None, None).await;
        let agent = task.agent_id;
        let result = tokio::select! {
            r = checks.run_checks(&self.project, &task, agent, &workspace, kinds, &self.cancel) => r,
            () = self.cancel.cancelled() => return Step::End(self.stopped()),
        };
        match result {
            Ok(None) => {
                let what = if kind == PipelineStageKind::Build { "build / type check" } else { "test" };
                self.stage(kind, StageStatus::Skipped, Some(format!("no {what} check is configured")), None).await;
                Step::Done
            }
            Ok(Some(run)) => {
                let rid = Some(run.id.clone());
                let passed: u32 = run.results.iter().map(|r| r.passed).sum();
                let failed: u32 = run.results.iter().map(|r| r.failed).sum();
                match run.status {
                    CheckStatus::Passed => {
                        let detail = if kind == PipelineStageKind::Test {
                            format!("{passed} passed")
                        } else {
                            format!("{} check(s) passed", run.results.len())
                        };
                        self.stage(kind, StageStatus::Passed, Some(detail), rid).await;
                        Step::Done
                    }
                    CheckStatus::Failed | CheckStatus::TimedOut | CheckStatus::Error => {
                        let a = healing::analyze_checks(kind, &run, Some(&workspace), &self.task_text().await);
                        let detail = if kind == PipelineStageKind::Test {
                            format!("{failed} failed, {passed} passed")
                        } else {
                            one_line(&a.summary, 200)
                        };
                        self.stage(kind, StageStatus::Failed, Some(detail), rid).await;
                        Step::Failed(Box::new(a))
                    }
                    CheckStatus::Skipped => {
                        self.stage(kind, StageStatus::Skipped, Some("the checks were skipped".into()), rid).await;
                        Step::Done
                    }
                    other => {
                        let why = format!("the checks did not complete ({other:?}) — an approval may be needed");
                        self.stage(kind, StageStatus::Failed, Some(why.clone()), rid).await;
                        self.user_step(why).await
                    }
                }
            }
            Err(e) => {
                let why = format!("the checks could not run: {e}");
                self.stage(kind, StageStatus::Failed, Some(why.clone()), None).await;
                self.user_step(why).await
            }
        }
    }

    /// A stage that cannot decide: the user does (accept → completed and
    /// merged; anything else ends the pipeline).
    async fn user_step(&mut self, why: String) -> Step {
        match self.await_user(why).await {
            UserDecision::Accepted => Step::End(self.merge().await),
            UserDecision::End(e) => Step::End(e),
        }
    }

    /// The task's worktree (or the project root).
    async fn workspace(&self) -> Option<PathBuf> {
        let task = self.inner.deps.tasks.get(self.p.task_id).await.ok()?;
        task.worktree
            .as_ref()
            .map(|w| PathBuf::from(&w.path))
            .filter(|p| p.is_dir())
            .or_else(|| self.project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir()))
    }

    async fn task_text(&self) -> String {
        self.inner
            .deps
            .tasks
            .get(self.p.task_id)
            .await
            .map(|t| format!("{}\n{}", t.title, t.description))
            .unwrap_or_default()
    }

    /// Evaluates the quality gates; `Some(reason)` when a required gate fails.
    async fn gates(&mut self) -> Option<String> {
        let checks = self.inner.pipeline.checks.read().clone()?;
        let task = self.task().await.ok()?;
        let report = checks.gates(&self.project, &task).await?;
        let failed: Vec<String> = report
            .gates
            .iter()
            .filter(|g| g.enabled && g.required && g.status == GateStatus::Failed)
            .map(|g| format!("{:?}", g.gate))
            .collect();
        let overall = report.overall;
        self.p.gates = Some(report);
        self.save().await;
        (overall == GateStatus::Failed && !failed.is_empty())
            .then(|| format!("required quality gates failed: {}", failed.join(", ")))
    }

    /// Waits for the user to accept (Completed) or reject the task.
    async fn await_user(&mut self, reason: String) -> UserDecision {
        let fresh = self.p.status != PipelineStatus::AwaitingUser;
        self.set_status(PipelineStatus::AwaitingUser, Some(reason.clone())).await;
        if fresh {
            let title = self.task().await.map(|t| t.title).unwrap_or_default();
            self.notice(
                format!(
                    "Task \"{title}\" is waiting for your review ({reason}). Accept it (→ Completed) or mark it Failed."
                ),
                format!(
                    "Задача «{title}» ждёт вашей проверки ({}). Примите её (→ «Выполнена») или отметьте как «Сбой».",
                    reason_ru(&reason)
                ),
            )
            .await;
        }
        let tick = self.inner.deps.config.tick;
        loop {
            match self.task().await {
                Ok(t) => match t.status {
                    TaskStatus::Completed => {
                        self.set_status(PipelineStatus::Running, Some("accepted by the user".into())).await;
                        return UserDecision::Accepted;
                    }
                    TaskStatus::Failed => {
                        return UserDecision::End(End::Failed("the user marked the task failed".into()))
                    }
                    TaskStatus::Cancelled => return UserDecision::End(End::Cancelled("the task was cancelled".into())),
                    TaskStatus::Interrupted => return UserDecision::End(End::Stopped),
                    _ => {}
                },
                Err(e) => return UserDecision::End(e),
            }
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = self.cancel.cancelled() => return UserDecision::End(self.stopped()),
            }
        }
    }

    // -----------------------------------------------------------------
    // self-healing
    // -----------------------------------------------------------------

    /// Handles a failure: limits → escalation, or a fix attempt. `Ok(())` =
    /// a fix was submitted; run the stages again.
    async fn heal(&mut self, analysis: FailureAnalysis) -> Result<(), End> {
        if let Some(last) = self.p.healing.last_mut() {
            if last.outcome == HealingOutcome::Submitted {
                last.outcome = HealingOutcome::StillFailing;
                last.finished_at = Some(now());
            }
        }
        loop {
            self.wait_unpaused().await?;
            let task = self.task().await?;
            let spent = task.usage.cost_usd;
            let mut budget = self.p.limits.max_budget_usd;
            for b in [
                task.budget.as_ref().and_then(|b| b.max_cost_usd),
                self.ctx.budget_usd.map(|r| self.p.cost_at_start_usd + r),
            ]
            .into_iter()
            .flatten()
            {
                budget = Some(budget.map_or(b, |x: f64| x.min(b)));
            }
            let budget_exceeded = self
                .inner
                .deps
                .cost
                .check(
                    orkestr_cost::BudgetContext {
                        project_id: Some(self.p.project_id),
                        task_id: Some(self.p.task_id),
                        provider_id: None,
                        agent_id: None,
                    },
                    0.0,
                )
                .await
                .is_ok_and(|r| r.outcome == orkestr_protocol::methods::cost::BudgetOutcome::Exceeded);
            let elapsed = (now() - self.p.started_at).to_std().unwrap_or_default();
            let decision = healing::decide(&HealState {
                limits: &self.p.limits,
                granted: self.p.granted_attempts,
                attempts: &self.p.healing,
                signature: &analysis.signature,
                elapsed,
                spent,
                budget,
                budget_exceeded,
                forced: self.forced,
            });
            self.forced = false;
            match decision {
                HealDecision::Escalate(e) => {
                    self.escalate(&e, &analysis).await?;
                    self.forced = true;
                }
                HealDecision::Attempt { attempt, escalate_model } => {
                    match self.fix(attempt, escalate_model, &analysis).await {
                        FixResult::Submitted => return Ok(()),
                        FixResult::AgentFailed => {}
                        FixResult::End(e) => return Err(e),
                    }
                }
            }
        }
    }

    /// Asks the user how to continue. `Ok(())` = more attempts granted.
    async fn escalate(&mut self, e: &healing::Escalation, analysis: &FailureAnalysis) -> Result<(), End> {
        let task = self.task().await?;
        let reason = e.reason();
        self.set_status(PipelineStatus::Escalated, Some(format!("self-healing stopped: {reason}"))).await;
        let attempts = self.p.healing.len();
        self.notice(
            format!(
                "Self-healing stopped on \"{}\" ({reason}; {attempts} attempt(s)). Last failure: {}. Approve the request to allow more attempts, or reject it to fail the task.",
                task.title,
                one_line(&analysis.summary, 240)
            ),
            format!(
                "Самовосстановление остановлено для задачи «{}» ({}; попыток: {attempts}). Последний сбой: {}. Подтвердите запрос, чтобы разрешить ещё попытки, или отклоните — задача будет отмечена как сбойная.",
                task.title,
                escalation_ru(e),
                one_line(&analysis.summary, 240)
            ),
        )
        .await;
        let mut req = ApprovalRequest::new(
            Capability::Write,
            cap(&format!("Self-healing gave up on \"{}\" — allow more fix attempts?", task.title), 300),
            cap(
                &format!(
                    "The quality pipeline could not fix the {:?} stage automatically: {reason}. Last failure: {}. Approve to grant {} more attempt(s) (a different model is tried); reject to mark the task failed.",
                    analysis.stage,
                    one_line(&analysis.summary, 400),
                    self.p.limits.max_attempts.max(1)
                ),
                1_000,
            ),
            RiskLevel::Medium,
        );
        req.project_id = Some(self.p.project_id);
        req.task_id = Some(self.p.task_id);
        let approval = match self.inner.deps.security.approvals.request(req).await {
            Ok(a) => a,
            Err(err) => {
                let why = format!("self-healing stopped ({reason}) and the escalation could not be filed: {err}");
                self.move_task(TaskStatus::Failed, &why).await;
                return Err(End::Failed(why));
            }
        };
        self.p.approval_id = Some(approval.id.to_string());
        self.save().await;
        let status = self
            .inner
            .deps
            .security
            .approvals
            .wait(approval.id, self.inner.deps.config.approval_timeout, &self.cancel)
            .await
            .unwrap_or(ApprovalStatus::Expired);
        self.p.approval_id = None;
        if self.cancel.is_cancelled() {
            return Err(self.stopped());
        }
        if status == ApprovalStatus::Approved {
            self.p.granted_attempts = self.p.granted_attempts.saturating_add(self.p.limits.max_attempts.max(1));
            self.set_status(PipelineStatus::Healing, Some("more attempts granted by the user".into())).await;
            return Ok(());
        }
        let why = format!("self-healing stopped ({reason}); the user declined further attempts");
        // Review / Testing / Running all may go to Failed.
        self.move_task(TaskStatus::Failed, &why).await;
        Err(End::Failed(why))
    }

    /// The model of a fix attempt: the previous one, or another model when
    /// escalating (routing for `Coding`, excluding the previous model).
    async fn pick_model(&self, escalate: bool) -> (Option<ModelRef>, bool, Option<String>) {
        let prev = self
            .p
            .healing
            .iter()
            .rev()
            .find_map(|h| h.model.clone())
            .or_else(|| self.p.author_model.clone())
            .or_else(|| self.ctx.model.clone());
        if !escalate {
            return (prev, false, None);
        }
        let exclude: Vec<ModelRef> = prev.iter().cloned().collect();
        let chain =
            crate::review::role_chain(&self.inner, &self.project, "Coding", None, &exclude).await.unwrap_or_default();
        match chain.into_iter().find(|m| Some(m) != prev.as_ref()) {
            Some(m) => (Some(m), true, None),
            None => (prev, false, Some("no other model is configured — the same model tries again".into())),
        }
    }

    /// Template of the fix agent: the author's (unless read-only), else the
    /// role's built-in template, else Backend.
    async fn template(&self) -> Option<orkestr_domain::AgentTemplate> {
        let templates = self.inner.deps.agents.templates();
        if let Some(a) = self.p.author_agent_id {
            if let Ok(agent) = self.inner.deps.agents.get(a).await {
                if let Some(tid) = agent.template_id {
                    if let Ok(t) = templates.get(tid).await {
                        if t.worktree_policy != WorktreePolicy::ReadOnlyMain {
                            return Some(t);
                        }
                    }
                }
            }
        }
        let pick = |list: Vec<orkestr_domain::AgentTemplate>| {
            let writable: Vec<_> =
                list.into_iter().filter(|t| t.worktree_policy != WorktreePolicy::ReadOnlyMain).collect();
            let official = writable.iter().find(|t| t.source == orkestr_domain::TemplateSource::Builtin).cloned();
            official.or_else(|| writable.into_iter().next())
        };
        if let Some(role) = &self.ctx.role {
            if let Some(t) = templates.list(Some(role.clone()), None).await.ok().and_then(pick) {
                return Some(t);
            }
        }
        templates.list(Some(AgentRole::Backend), None).await.ok().and_then(pick)
    }

    /// Waits for a scheduler slot for the fix agent (global caps, provider /
    /// model concurrency, rate limits, resources).
    async fn admit(&mut self, model: Option<&ModelRef>) -> Result<(), End> {
        let tick = self.inner.deps.config.tick;
        loop {
            let task = self.task().await?;
            let mut sched = orkestr_task_engine::SchedTask::from_task(&task);
            if let Some(m) = model {
                sched.provider = Some(m.provider_id.to_string());
                sched.model = Some(crate::parallel::model_key(m));
            }
            sched.worktree = self.git.then(|| format!("pending-worktree:{}", task.id));
            let active: Vec<orkestr_task_engine::SchedTask> = self
                .inner
                .deps
                .tasks
                .list(orkestr_protocol::methods::tasks::ListTasksParams {
                    status: Some(crate::parallel::ACTIVE_STATUSES.to_vec()),
                    ..Default::default()
                })
                .await
                .unwrap_or_default()
                .iter()
                .map(|t| Inner::active_sched(t, self.git))
                .collect();
            let candidate = crate::parallel::Candidate { sched, title: task.title.clone() };
            let out = self.inner.parallel.admit(self.sched_id, self.p.project_id, 1, vec![candidate], &active);
            self.inner.parallel.publish_if_changed().await;
            if out.start.contains(&task.id) {
                self.inner.parallel.unregister(self.sched_id);
                return Ok(());
            }
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = self.cancel.cancelled() => return Err(self.stopped()),
            }
        }
    }

    async fn fix(&mut self, attempt: u32, escalate: bool, analysis: &FailureAnalysis) -> FixResult {
        let (model, escalated, note) = self.pick_model(escalate).await;
        let allowed = self.p.limits.max_attempts.saturating_add(self.p.granted_attempts);
        self.p.healing.push(HealingAttempt {
            attempt,
            analysis: analysis.clone(),
            model: model.clone(),
            escalated,
            agent_id: None,
            outcome: HealingOutcome::Queued,
            note,
            started_at: now(),
            finished_at: None,
        });
        self.set_status(
            PipelineStatus::Healing,
            Some(format!("fix attempt {attempt} of {allowed}: {}", one_line(&analysis.summary, 200))),
        )
        .await;
        self.inner
            .deps
            .publish(NewEvent::new(EventKind::HealingAttemptStarted(HealingAttemptPayload {
                pipeline_id: self.p.id.clone(),
                project_id: self.p.project_id,
                task_id: self.p.task_id,
                attempt,
                stage: analysis.stage,
                signature: analysis.signature.clone(),
                summary: one_line(&analysis.summary, 300),
                model: model.clone(),
                escalated,
            })))
            .await;
        let title = self.task().await.map(|t| t.title).unwrap_or_default();
        self.notice(
            format!(
                "Self-healing \"{title}\" — attempt {attempt} of {allowed}{}: {}",
                if escalated { " with a stronger model" } else { "" },
                one_line(&analysis.summary, 200)
            ),
            format!(
                "Самовосстановление задачи «{title}» — попытка {attempt} из {allowed}{}: {}",
                if escalated { " с более сильной моделью" } else { "" },
                one_line(&analysis.summary, 200)
            ),
        )
        .await;
        if let Err(e) = self.admit(model.as_ref()).await {
            return FixResult::End(e);
        }
        let Some(template) = self.template().await else {
            return self.attempt_failed("no writable agent template is installed".into()).await;
        };
        let task = match self.task().await {
            Ok(t) => t,
            Err(e) => return FixResult::End(e),
        };
        let (policy, base) = if self.git {
            (WorktreePolicy::DedicatedWorktree, self.p.branch.clone())
        } else {
            (WorktreePolicy::SharedWorktree, None)
        };
        let budget = self.ctx.budget_usd.map(|r| Budget {
            max_cost_usd: Some((self.p.cost_at_start_usd + r - task.usage.cost_usd).max(0.0)),
            ..Budget::unrestricted(BudgetPeriod::PerTask)
        });
        let params = SpawnAgentParams {
            project_id: self.p.project_id,
            template_id: template.id,
            task_id: Some(self.p.task_id),
            new_task: None,
            model: model.clone(),
            worktree_policy: Some(policy),
            name: None,
            max_turns: None,
            base_branch: base,
            approval_mode: Some(self.ctx.policy),
            budget,
        };
        let extras = SpawnExtras {
            briefing: Some(healing::briefing(analysis, attempt, allowed)),
            fix_loop: true,
            ..SpawnExtras::default()
        };
        let tick = self.inner.deps.config.tick;
        let mut tries = 0u32;
        let agent = loop {
            tries += 1;
            match self.inner.deps.agents.spawn_with(params.clone(), extras.clone()).await {
                Ok((agent, _)) => break agent,
                Err(AgentError::Conflict(m)) if !m.contains("only Planned") && tries < 600 => {
                    // Runtime capacity: try again shortly.
                    tokio::select! {
                        () = tokio::time::sleep(tick) => {}
                        () = self.cancel.cancelled() => return FixResult::End(self.stopped()),
                    }
                }
                Err(AgentError::Forbidden(m)) => return FixResult::End(End::Cancelled(m)),
                Err(e) => return self.attempt_failed(format!("could not start the fix agent: {e}")).await,
            }
        };
        if let Some(w) = agent.worktree.as_ref().filter(|w| w.branch.starts_with("agent/")) {
            self.p.branch = Some(w.branch.clone());
        }
        if let Some(h) = self.p.healing.last_mut() {
            h.agent_id = Some(agent.id);
            h.outcome = HealingOutcome::Running;
            if h.model.is_none() {
                h.model = agent.model.clone();
            }
        }
        self.save().await;
        // Wait for the fix agent.
        while self.inner.deps.agents.is_live(agent.id) {
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = self.cancel.cancelled() => {
                    let mode = if *self.handle.stop.lock() == Some(DriverStop::Shutdown) {
                        AgentStopMode::Interrupt
                    } else {
                        AgentStopMode::Cancel
                    };
                    let _ = self.inner.deps.agents.stop(agent.id, mode).await;
                    return FixResult::End(self.stopped());
                }
            }
        }
        let task = match self.task().await {
            Ok(t) => t,
            Err(e) => return FixResult::End(e),
        };
        match task.status {
            TaskStatus::Review | TaskStatus::Testing => {
                if let Some(h) = self.p.healing.last_mut() {
                    h.outcome = HealingOutcome::Submitted;
                    h.note = h.note.take().or_else(|| task.result.as_ref().map(|r| one_line(&r.summary, 300)));
                }
                self.set_status(
                    PipelineStatus::Running,
                    Some(format!("fix attempt {attempt} submitted — checking it")),
                )
                .await;
                FixResult::Submitted
            }
            TaskStatus::Cancelled => FixResult::End(End::Cancelled("the task was cancelled".into())),
            TaskStatus::Interrupted => FixResult::End(End::Stopped),
            _ => {
                let why = task.result.as_ref().map_or_else(
                    || format!("the fix agent ended with the task {:?}", task.status),
                    |r| one_line(&r.summary, 300),
                );
                self.attempt_failed(why).await
            }
        }
    }

    async fn attempt_failed(&mut self, why: String) -> FixResult {
        if let Some(h) = self.p.healing.last_mut() {
            h.outcome = HealingOutcome::AgentFailed;
            h.note = Some(cap(&why, MAX_REASON));
            h.finished_at = Some(now());
        }
        self.save().await;
        FixResult::AgentFailed
    }

    // -----------------------------------------------------------------
    // merge
    // -----------------------------------------------------------------

    async fn merge(&mut self) -> End {
        let kind = PipelineStageKind::Merge;
        if !self.enabled(kind) {
            return End::Passed;
        }
        let merger = self.inner.pipeline.merger.read().clone();
        let Some(merger) = merger else {
            self.stage(kind, StageStatus::Skipped, Some("the Merge Manager is not available".into()), None).await;
            return End::Passed;
        };
        let Some(branch) = self.p.branch.clone().filter(|_| self.git) else {
            self.stage(kind, StageStatus::Skipped, Some("no agent branch to merge".into()), None).await;
            return End::Passed;
        };
        self.stage(kind, StageStatus::Running, None, None).await;
        let proposal = merger
            .propose(MergeProposeParams {
                project_id: self.p.project_id,
                run_id: None,
                branches: Some(vec![MergeBranchInput { branch: branch.clone(), task_id: Some(self.p.task_id) }]),
                target: None,
            })
            .await;
        let proposal = match proposal {
            Ok(p) => p,
            Err(e) => {
                let why = format!("the merge proposal failed: {e}");
                self.p.merge_outcome = Some(cap(&why, MAX_REASON));
                self.stage(kind, StageStatus::Failed, Some(why), None).await;
                return End::Passed;
            }
        };
        let pid = Some(proposal.id.to_string());
        self.p.merge_proposal_id = Some(proposal.id);
        if proposal.recommendation == MergeRecommendation::NothingToMerge {
            self.p.merge_outcome = Some("nothing to merge — the branch is already in the target".into());
            self.stage(kind, StageStatus::Passed, Some("already merged".into()), pid).await;
            return End::Passed;
        }
        if self.cfg.merge == PipelineMergeMode::PerPolicy && proposal.auto_mergeable {
            if let Err(e) = merger.execute(MergeExecuteParams { proposal_id: proposal.id, order: None }).await {
                let why = format!("the automatic merge could not start: {e}");
                self.p.merge_outcome = Some(cap(&why, MAX_REASON));
                self.stage(kind, StageStatus::Failed, Some(why), pid).await;
                return End::Passed;
            }
            let deadline = tokio::time::Instant::now() + MERGE_WAIT;
            loop {
                let current = match merger.get(proposal.id).await {
                    Ok(p) => p,
                    Err(e) => {
                        self.p.merge_outcome = Some(cap(&e.to_string(), MAX_REASON));
                        self.stage(kind, StageStatus::Failed, Some(e.to_string()), pid).await;
                        return End::Passed;
                    }
                };
                if current.status.is_final() {
                    let ok = current.status == MergeProposalStatus::Completed;
                    let outcome = if ok {
                        format!("merged `{branch}` into `{}`", current.target)
                    } else {
                        format!(
                            "merge {:?}{}",
                            current.status,
                            current.status_detail.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
                        )
                    };
                    self.p.merge_outcome = Some(cap(&outcome, MAX_REASON));
                    let st = if ok { StageStatus::Passed } else { StageStatus::Failed };
                    self.stage(kind, st, Some(outcome), pid).await;
                    return End::Passed;
                }
                if tokio::time::Instant::now() >= deadline {
                    self.p.merge_outcome = Some(format!("merge still {:?}", current.status));
                    self.stage(kind, StageStatus::Waiting, Some(format!("merge still {:?}", current.status)), pid)
                        .await;
                    return End::Passed;
                }
                tokio::select! {
                    () = tokio::time::sleep(self.inner.deps.config.tick) => {}
                    () = self.cancel.cancelled() => {
                        self.stage(kind, StageStatus::Waiting, Some("the merge continues in the Merge Manager".into()), pid).await;
                        return End::Passed;
                    }
                }
            }
        }
        let why = if self.cfg.merge == PipelineMergeMode::Propose {
            "merge proposal ready — merge it in the Merge Manager".to_owned()
        } else {
            format!(
                "merge proposal ready ({:?}) — waiting for your approval in the Merge Manager",
                proposal.recommendation
            )
        };
        self.p.merge_outcome = Some("proposed — awaiting approval".into());
        self.stage(kind, StageStatus::Waiting, Some(why), pid).await;
        End::Passed
    }
}

/// Russian wording of the pipeline's waiting reasons (stored reasons stay English).
fn reason_ru(reason: &str) -> String {
    if reason.starts_with("every quality stage passed") {
        "все этапы качества пройдены — нужна ваша приёмка".into()
    } else if reason.starts_with("the agent reported a partial result") {
        "агент сообщил о частичном результате".into()
    } else if reason.starts_with("required quality gates failed") {
        format!("не пройдены обязательные проверки качества: {}", reason.split(": ").nth(1).unwrap_or_default())
    } else {
        format!("конвейер качества не смог продолжить: {reason}")
    }
}

fn escalation_ru(e: &healing::Escalation) -> String {
    match e {
        healing::Escalation::Disabled => "самовосстановление отключено для проекта".into(),
        healing::Escalation::Attempts { used, allowed } => format!("использовано попыток: {used} из {allowed}"),
        healing::Escalation::WallTime { mins } => format!("истёк лимит времени конвейера — {mins} мин"),
        healing::Escalation::Budget { spent, limit } => format!("бюджет задачи исчерпан (${spent:.4} из ${limit:.2})"),
        healing::Escalation::NoProgress { .. } => "нет прогресса: исправление дало тот же сбой".into(),
    }
}

/// Run-report view of a pipeline.
#[must_use]
pub fn report_of(p: &TaskPipeline, title: &str) -> orkestr_protocol::methods::pipeline::PipelineReport {
    let brief = orkestr_protocol::methods::pipeline::RunTaskPipeline::from(p);
    orkestr_protocol::methods::pipeline::PipelineReport {
        task_id: p.task_id,
        title: title.to_owned(),
        status: p.status,
        stages: brief.stages,
        review_verdict: p.review_verdict,
        security_verdict: p.security_verdict,
        gates: p.gates.as_ref().map(|g| g.overall),
        failed_gates: p
            .gates
            .as_ref()
            .map(|g| {
                g.gates
                    .iter()
                    .filter(|r| r.enabled && r.required && r.status == GateStatus::Failed)
                    .map(|r| r.gate)
                    .collect()
            })
            .unwrap_or_default(),
        healing_attempts: brief.healing_attempts,
        merge_outcome: p.merge_outcome.clone(),
        merge_proposal_id: p.merge_proposal_id,
    }
}
