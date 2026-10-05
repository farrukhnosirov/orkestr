//! Parallel execution hardening (§11, §12, §38, §62.5, §68.24–26).
//!
//! [`ParallelScheduler`] is the one admission controller of the core. Every
//! live run supervisor registers its ready tasks and asks [`ParallelScheduler::admit`]
//! which may start; the decision is computed over the ready tasks of **all**
//! live runs (all projects) against everything already running, with the
//! task-engine [`Scheduler`]:
//!
//! * global, per-project (≤ the run's own cap) and per-team caps;
//! * provider / model concurrency (agent slots) and provider rate-limit
//!   back-off (from `ProviderRateLimited` events and rate-limit errors seen by
//!   the [`GatedGateway`]);
//! * CPU / RAM / disk / GPU thresholds from the live node sampler;
//! * Git conflict avoidance — predicted files (plan) ∪ files actually touched:
//!   never two agents on the same file in the same worktree; agents in
//!   separate worktrees may overlap, but the overlap is flagged early and
//!   confirmed with `git merge-tree` ([`MergeRisk`]);
//! * priority queue with aging and fair share across projects; runs keep
//!   going whatever project the UI shows (§12);
//! * explanations for every held-back task ("waiting: provider concurrency 2/2").
//!
//! [`GatedGateway`] wraps the model gateway with per-provider / per-model
//! request semaphores (API concurrency) and records rate limits.
//!
//! After a core restart, [`Manager::auto_resume_interrupted`] resumes
//! interrupted runs (and interrupted stand-alone agent tasks) from their last
//! checkpoint — the previous handoff is injected into the new agent's prompt
//! by the runtime and a dedicated worktree is based on the previous agent
//! branch — when the policy allows; otherwise it leaves them waiting for the
//! user with a note in the Manager conversation. Every automatic loop here is
//! bounded (`max_auto_resume_attempts`, back-off caps, queue TTLs).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_agent_runtime::{ModelGateway, ProviderInfo};
use orkestr_domain::{
    now, AgentRole, ApprovalMode, ManagerRunId, ModelRef, Project, ProjectId, ProviderId, TaskActor, TaskId,
    TaskPriority, TaskStatus, Timestamp, WorktreePolicy,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::agents::SpawnAgentParams;
use orkestr_protocol::methods::manager::{ManagerRunParams, ManagerRunStatus};
use orkestr_protocol::methods::scheduler::{
    AdmissionCheckResult, AutoResumePolicy, MergeRisk, MergeRiskPayload, ProjectLoad, ProviderLoad, SchedEntry,
    SchedEntryState, SchedulerCounts, SchedulerExplanation, SchedulerResources, SchedulerSettings, SchedulerStatus,
    SchedulerUpdatedPayload, WaitReason,
};
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_protocol::NewEvent;
use orkestr_providers::{ChatEvent, ChatRequest, ChatStream, HandoffHook, ProviderAdapter, ProviderError, RouteRequest};
use orkestr_storage::{Db, Migration};
use orkestr_task_engine::{
    effective_priority, first_overlap, AgingPolicy, Deferral, ResourceSnapshot, SchedTask, Scheduler, SchedulerLimits,
    TaskService,
};
use parking_lot::{Mutex, RwLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::deps::{ManagerHub, ResourceProbe};
use crate::error::{OrchResult, OrchestratorError};
use crate::service::{Inner, Manager, MessageExtras};

/// Tables of this module (composed by [`crate::orchestrator_migrations`]).
pub const PARALLEL_MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_parallel_0001_scheduler_settings",
    owner: "orchestrator",
    up: "CREATE TABLE scheduler_settings (
            id         INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
            doc        TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS scheduler_settings;",
}];

/// A run's registration expires when its supervisor stops refreshing it.
pub const QUEUE_TTL: Duration = Duration::from_secs(120);
/// Longest rate-limit back-off.
pub const MAX_BACKOFF_SECS: u64 = 600;
/// Rate-limit hits older than this no longer escalate the back-off.
pub const BACKOFF_RESET: Duration = Duration::from_secs(600);
/// Minimum interval between merge-risk checks of one project.
pub const RISK_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Max worktree pairs `git merge-tree`d per check.
pub const MAX_RISK_PREVIEWS: usize = 6;
/// How long a predicted head model is cached per (project, role).
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);
/// Reason stored on a run that was paused by the user when Orkestr stopped
/// (such runs are never resumed automatically).
pub const PAUSED_INTERRUPTED_REASON: &str =
    "paused by the user when Orkestr stopped — resume it to continue";

/// Task statuses that hold an agent slot.
pub const ACTIVE_STATUSES: [TaskStatus; 2] = [TaskStatus::Running, TaskStatus::Waiting];

/// A ready task offered for admission by a run supervisor.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Scheduling view (files = predicted ∪ touched; worktree = where it will run).
    pub sched: SchedTask,
    /// Title (status / explain).
    pub title: String,
}

/// What [`ParallelScheduler::admit`] decided for one run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdmitOutcome {
    /// Tasks of this run to start now, in order.
    pub start: Vec<TaskId>,
    /// Tasks of this run held back, with the reason.
    pub deferred: Vec<(TaskId, Deferral)>,
}

/// One agent's workspace for the merge-risk check.
#[derive(Debug, Clone)]
pub struct RiskItem {
    /// Task.
    pub task_id: TaskId,
    /// Agent branch (dedicated worktree).
    pub branch: Option<String>,
    /// Worktree path.
    pub worktree: Option<String>,
    /// Predicted ∪ touched files.
    pub files: Vec<String>,
}

/// Outcome of [`Manager::auto_resume_interrupted`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoResumeReport {
    /// Runs resumed automatically.
    pub resumed_runs: Vec<ManagerRunId>,
    /// Runs left for the user.
    pub waiting_runs: Vec<ManagerRunId>,
    /// Stand-alone tasks resumed with a new agent.
    pub resumed_tasks: Vec<TaskId>,
    /// Stand-alone tasks left for the user.
    pub waiting_tasks: Vec<TaskId>,
}

#[derive(Debug, Clone)]
struct RunQueue {
    project_id: ProjectId,
    run_cap: usize,
    paused: bool,
    /// Reason that applies to every entry (paused, agent budget).
    hold: Option<WaitReason>,
    entries: Vec<Candidate>,
    updated: Instant,
}

#[derive(Debug, Clone)]
struct Decision {
    reason: Option<WaitReason>,
    text: Option<String>,
    position: u32,
}

#[derive(Debug, Clone)]
struct RateState {
    until: Timestamp,
    hits: u32,
    last_hit: Instant,
}

/// Predicted head model cached at an instant.
type CachedModel = (Instant, Option<ModelRef>);

#[derive(Debug, Default)]
struct GateState {
    sems: HashMap<String, (usize, Arc<Semaphore>)>,
    in_flight: HashMap<String, u32>,
    queued: HashMap<String, u32>,
}

/// Engines the scheduler reads.
pub struct ParallelDeps {
    /// Database (settings).
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Task engine.
    pub tasks: Arc<TaskService>,
    /// Provider facts (agent slots, names).
    pub hub: Arc<dyn ManagerHub>,
    /// Host resources.
    pub resources: Arc<dyn ResourceProbe>,
    /// Git (merge-risk previews).
    pub vcs: orkestr_vcs::Vcs,
}

/// The global parallel-execution scheduler (see module docs).
pub struct ParallelScheduler {
    deps: ParallelDeps,
    settings: RwLock<SchedulerSettings>,
    queues: Mutex<HashMap<ManagerRunId, RunQueue>>,
    task_runs: Mutex<HashMap<TaskId, ManagerRunId>>,
    decisions: Mutex<HashMap<TaskId, Decision>>,
    rate: Mutex<BTreeMap<String, RateState>>,
    gate: Mutex<GateState>,
    risks: Mutex<BTreeMap<(TaskId, TaskId), MergeRisk>>,
    risk_checked: Mutex<HashMap<ProjectId, Instant>>,
    resume_branches: Mutex<HashMap<TaskId, String>>,
    model_cache: Mutex<HashMap<(ProjectId, String), CachedModel>>,
    last_counts: Mutex<Option<SchedulerCounts>>,
    active_count: Mutex<usize>,
    listening: Mutex<bool>,
}

impl std::fmt::Debug for ParallelScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelScheduler").field("runs", &self.queues.lock().len()).finish_non_exhaustive()
    }
}

fn u32c(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Wire form of a task-engine deferral.
#[must_use]
pub fn wait_reason(d: &Deferral) -> WaitReason {
    match d.clone() {
        Deferral::GlobalLimit { running, limit } => WaitReason::GlobalLimit { running: u32c(running), limit: u32c(limit) },
        Deferral::ProjectLimit { running, limit } => {
            WaitReason::ProjectLimit { running: u32c(running), limit: u32c(limit) }
        }
        Deferral::TeamLimit { team, running, limit } => {
            WaitReason::TeamLimit { team, running: u32c(running), limit: u32c(limit) }
        }
        Deferral::RateLimited { provider, until } => WaitReason::RateLimited { provider_id: provider, until },
        Deferral::ProviderBusy { provider, running, limit } => {
            WaitReason::ProviderConcurrency { provider_id: provider, running: u32c(running), limit: u32c(limit) }
        }
        Deferral::ModelBusy { model, running, limit } => {
            WaitReason::ModelConcurrency { model, running: u32c(running), limit: u32c(limit) }
        }
        Deferral::CpuBusy { projected_percent, limit_percent } => WaitReason::Cpu { projected_percent, limit_percent },
        Deferral::LowMemory { free_mb, needed_mb } => WaitReason::Memory { free_mb, needed_mb },
        Deferral::LowDisk { free_mb, min_mb } => WaitReason::Disk { free_mb, min_mb },
        Deferral::GpuBusy { usage_percent, limit_percent } => WaitReason::Gpu { usage_percent, limit_percent },
        Deferral::FileConflict { with, path } => WaitReason::FileConflict { with_task: with, path },
    }
}

/// English text of a hold reason that is not a task-engine deferral.
#[must_use]
pub fn hold_text(r: &WaitReason) -> String {
    match r {
        WaitReason::Paused => "waiting: the run is paused".into(),
        WaitReason::AgentBudget { spawned, limit } => format!("waiting: run agent limit {spawned}/{limit}"),
        other => format!("waiting: {other:?}"),
    }
}

/// Model key used for model slots (`<provider id>/<model>`).
#[must_use]
pub fn model_key(m: &ModelRef) -> String {
    format!("{}/{}", m.provider_id, m.model)
}

impl ParallelScheduler {
    /// New scheduler with default settings (call [`Self::load_settings`]).
    #[must_use]
    pub fn new(deps: ParallelDeps) -> Self {
        Self {
            deps,
            settings: RwLock::new(SchedulerSettings::default()),
            queues: Mutex::new(HashMap::new()),
            task_runs: Mutex::new(HashMap::new()),
            decisions: Mutex::new(HashMap::new()),
            rate: Mutex::new(BTreeMap::new()),
            gate: Mutex::new(GateState::default()),
            risks: Mutex::new(BTreeMap::new()),
            risk_checked: Mutex::new(HashMap::new()),
            resume_branches: Mutex::new(HashMap::new()),
            model_cache: Mutex::new(HashMap::new()),
            last_counts: Mutex::new(None),
            active_count: Mutex::new(0),
            listening: Mutex::new(false),
        }
    }

    /// Current settings.
    #[must_use]
    pub fn settings(&self) -> SchedulerSettings {
        self.settings.read().clone()
    }

    /// Loads persisted settings (missing → defaults).
    pub async fn load_settings(&self) -> OrchResult<()> {
        let doc: Option<String> = self
            .deps
            .db
            .call(|c| {
                use orkestr_storage::rusqlite::OptionalExtension;
                Ok(c.query_row("SELECT doc FROM scheduler_settings WHERE id = 1", [], |r| r.get(0)).optional()?)
            })
            .await?;
        if let Some(doc) = doc {
            match serde_json::from_str::<SchedulerSettings>(&doc) {
                Ok(s) if s.validate().is_ok() => *self.settings.write() = s,
                Ok(_) | Err(_) => tracing::warn!("stored scheduler settings are invalid; using defaults"),
            }
        }
        Ok(())
    }

    /// Validates, persists and applies new settings (`scheduler.setLimits`).
    pub async fn set_settings(&self, s: SchedulerSettings) -> OrchResult<SchedulerSettings> {
        s.validate().map_err(OrchestratorError::Invalid)?;
        let doc = serde_json::to_string(&s)?;
        let at = now().to_rfc3339();
        self.deps
            .db
            .call(move |c| {
                c.execute(
                    "INSERT INTO scheduler_settings (id, doc, updated_at) VALUES (1, ?1, ?2)
                     ON CONFLICT(id) DO UPDATE SET doc = excluded.doc, updated_at = excluded.updated_at",
                    [doc, at],
                )?;
                Ok(())
            })
            .await?;
        *self.settings.write() = s.clone();
        tracing::info!(global = s.max_parallel_global, per_project = s.max_parallel_per_project, "scheduler settings updated");
        let counts = self.counts();
        self.publish(SchedulerUpdatedPayload { counts, settings_changed: true }).await;
        Ok(s)
    }

    async fn publish(&self, p: SchedulerUpdatedPayload) {
        *self.last_counts.lock() = Some(p.counts);
        if let Err(e) = self.deps.events.publish(NewEvent::new(EventKind::SchedulerUpdated(p))).await {
            tracing::debug!(error = %e, "could not publish SchedulerUpdated");
        }
    }

    /// Publishes `SchedulerUpdated` when the counters changed.
    pub async fn publish_if_changed(&self) {
        let counts = self.counts();
        if *self.last_counts.lock() == Some(counts) {
            return;
        }
        self.publish(SchedulerUpdatedPayload { counts, settings_changed: false }).await;
    }

    fn counts(&self) -> SchedulerCounts {
        let queues = self.queues.lock();
        let decisions = self.decisions.lock();
        let (mut queued, mut waiting) = (0u32, 0u32);
        let mut seen = HashSet::new();
        for q in queues.values().filter(|q| q.updated.elapsed() < QUEUE_TTL) {
            for e in &q.entries {
                if !seen.insert(e.sched.task_id) {
                    continue;
                }
                let held = q.hold.is_some() || decisions.get(&e.sched.task_id).is_some_and(|d| d.reason.is_some());
                if held {
                    waiting += 1;
                } else {
                    queued += 1;
                }
            }
        }
        SchedulerCounts { running: u32c(*self.active_count.lock()), queued, waiting }
    }

    // ------------------------------------------------------------------
    // rate limits
    // ------------------------------------------------------------------

    /// Records a rate limit of `provider`: back-off for `retry_after` seconds,
    /// or the configured base doubling with repeated hits (≤ 10 min).
    pub fn note_rate_limited(&self, provider: &str, retry_after: Option<u64>) {
        let base = u64::from(self.settings.read().rate_limit_backoff_secs.max(1));
        let mut rate = self.rate.lock();
        let entry = rate.entry(provider.to_owned()).or_insert(RateState { until: now(), hits: 0, last_hit: Instant::now() });
        if entry.last_hit.elapsed() > BACKOFF_RESET {
            entry.hits = 0;
        }
        entry.hits = entry.hits.saturating_add(1);
        entry.last_hit = Instant::now();
        let secs = retry_after
            .filter(|s| *s > 0)
            .unwrap_or_else(|| base.saturating_mul(1u64 << entry.hits.saturating_sub(1).min(6)))
            .min(MAX_BACKOFF_SECS);
        let until = now() + chrono::Duration::seconds(i64::try_from(secs).unwrap_or(600));
        if until > entry.until {
            entry.until = until;
        }
        tracing::info!(provider, secs, hits = entry.hits, "provider rate limited: new agents back off");
    }

    /// Providers currently in back-off → until.
    #[must_use]
    pub fn rate_limited(&self) -> BTreeMap<String, Timestamp> {
        let at = now();
        self.rate.lock().iter().filter(|(_, s)| s.until > at).map(|(p, s)| (p.clone(), s.until)).collect()
    }

    /// Clears a provider's back-off (tests, manual reset).
    pub fn clear_rate_limit(&self, provider: &str) {
        self.rate.lock().remove(provider);
    }

    /// Listens for `ProviderRateLimited` events (idempotent; ends with the bus
    /// or when the scheduler is dropped).
    pub fn spawn_rate_listener(self: &Arc<Self>) {
        {
            let mut l = self.listening.lock();
            if *l {
                return;
            }
            *l = true;
        }
        let mut sub = match self.deps.events.subscribe([orkestr_protocol::topic::GLOBAL]) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "scheduler: cannot listen for rate limits");
                return;
            }
        };
        let weak: Weak<Self> = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(r) = sub.recv().await {
                let orkestr_events::Received::Event(ev) = r else { continue };
                let EventKind::ProviderRateLimited(p) = &ev.kind else { continue };
                let Some(me) = weak.upgrade() else { break };
                me.note_rate_limited(&p.provider_id.to_string(), p.retry_after_secs);
            }
        });
    }

    // ------------------------------------------------------------------
    // request gate (API concurrency)
    // ------------------------------------------------------------------

    fn provider_limit(&self, provider: &str) -> Option<usize> {
        let s = self.settings.read();
        if let Some(c) = s.provider_caps.iter().find(|c| c.provider_id == provider) {
            return Some(c.max as usize);
        }
        drop(s);
        self.deps.hub.provider_slots().get(provider).copied()
    }

    fn model_limit(&self, key: &str) -> Option<usize> {
        self.settings
            .read()
            .model_caps
            .iter()
            .find(|c| format!("{}/{}", c.provider_id, c.model) == key)
            .map(|c| c.max as usize)
    }

    fn semaphore(&self, key: &str, limit: usize) -> Arc<Semaphore> {
        let mut g = self.gate.lock();
        match g.sems.get(key) {
            Some((l, s)) if *l == limit => Arc::clone(s),
            _ => {
                let s = Arc::new(Semaphore::new(limit.max(1)));
                g.sems.insert(key.to_owned(), (limit, Arc::clone(&s)));
                s
            }
        }
    }

    fn bump(&self, map: fn(&mut GateState) -> &mut HashMap<String, u32>, key: &str, up: bool) {
        let mut g = self.gate.lock();
        let m = map(&mut g);
        let v = m.entry(key.to_owned()).or_default();
        *v = if up { v.saturating_add(1) } else { v.saturating_sub(1) };
    }

    /// Waits for the provider (and model) request permits of `model`.
    /// `None` when cancelled.
    async fn acquire(self: &Arc<Self>, model: &ModelRef, cancel: &CancellationToken) -> Option<InFlight> {
        let provider = model.provider_id.to_string();
        let key = model_key(model);
        let mut permits = Vec::new();
        let gates: Vec<(String, Option<usize>)> =
            vec![(provider.clone(), self.provider_limit(&provider)), (format!("model:{key}"), self.model_limit(&key))];
        self.bump(|g| &mut g.queued, &provider, true);
        for (k, limit) in gates {
            let Some(limit) = limit else { continue };
            let sem = self.semaphore(&k, limit);
            let got = tokio::select! {
                p = sem.acquire_owned() => p.ok(),
                () = cancel.cancelled() => None,
            };
            match got {
                Some(p) => permits.push(p),
                None => {
                    self.bump(|g| &mut g.queued, &provider, false);
                    return None;
                }
            }
        }
        self.bump(|g| &mut g.queued, &provider, false);
        self.bump(|g| &mut g.in_flight, &provider, true);
        Some(InFlight { sched: Arc::downgrade(self), provider, _permits: permits })
    }

    // ------------------------------------------------------------------
    // admission
    // ------------------------------------------------------------------

    fn limits(&self, queues: &HashMap<ManagerRunId, RunQueue>) -> SchedulerLimits {
        let s = self.settings.read().clone();
        let mut provider_slots = self.deps.hub.provider_slots();
        for c in &s.provider_caps {
            provider_slots.insert(c.provider_id.clone(), c.max as usize);
        }
        let mut project_overrides: BTreeMap<ProjectId, usize> =
            s.project_caps.iter().map(|c| (c.project_id, c.max as usize)).collect();
        for q in queues.values() {
            let base = project_overrides.get(&q.project_id).copied().unwrap_or(s.max_parallel_per_project as usize);
            project_overrides.insert(q.project_id, base.min(q.run_cap.max(1)));
        }
        SchedulerLimits {
            max_parallel_global: s.max_parallel_global as usize,
            max_parallel_per_project: s.max_parallel_per_project as usize,
            project_overrides,
            team_caps: s.team_caps.iter().map(|c| (c.team.clone(), c.max as usize)).collect(),
            provider_slots,
            model_slots: s.model_caps.iter().map(|c| (format!("{}/{}", c.provider_id, c.model), c.max as usize)).collect(),
            rate_limited: self.rate_limited(),
            max_cpu_percent: s.max_cpu_percent,
            min_free_ram_mb: s.min_free_ram_mb,
            min_free_disk_mb: s.min_free_disk_mb,
            max_gpu_percent: s.max_gpu_percent,
            aging: if s.aging_minutes == 0 {
                AgingPolicy::disabled()
            } else {
                AgingPolicy { interval: chrono::Duration::minutes(i64::from(s.aging_minutes)), cap: TaskPriority::High }
            },
            fair_share: s.fair_share,
            ..SchedulerLimits::default()
        }
    }

    /// The effective scheduler limits right now (tests, diagnostics).
    #[must_use]
    pub fn current_limits(&self) -> SchedulerLimits {
        let queues = self.queues.lock();
        self.limits(&queues)
    }

    /// Registers the run's ready tasks and decides — over the ready tasks of
    /// every live run — which of this run's tasks may start now. `active` are
    /// all tasks holding an agent slot (`Running`/`Waiting`, every project).
    pub fn admit(
        &self,
        run_id: ManagerRunId,
        project_id: ProjectId,
        run_cap: usize,
        candidates: Vec<Candidate>,
        active: &[SchedTask],
    ) -> AdmitOutcome {
        let at = now();
        let mine: HashSet<TaskId> = candidates.iter().map(|c| c.sched.task_id).collect();
        {
            let mut tr = self.task_runs.lock();
            for id in &mine {
                tr.insert(*id, run_id);
            }
        }
        *self.active_count.lock() = active.len();
        let mut queues = self.queues.lock();
        queues.insert(
            run_id,
            RunQueue { project_id, run_cap, paused: false, hold: None, entries: candidates, updated: Instant::now() },
        );
        queues.retain(|_, q| q.updated.elapsed() < QUEUE_TTL);
        let active_ids: HashSet<TaskId> = active.iter().map(|t| t.task_id).collect();
        let mut seen = HashSet::new();
        let pool: Vec<SchedTask> = queues
            .values()
            .filter(|q| !q.paused && q.hold.is_none())
            .flat_map(|q| q.entries.iter())
            .filter(|e| !active_ids.contains(&e.sched.task_id) && seen.insert(e.sched.task_id))
            .map(|e| e.sched.clone())
            .collect();
        let limits = self.limits(&queues);
        drop(queues);
        let plan = Scheduler::new(limits).next_runnable(&pool, active, &self.deps.resources.snapshot(), at);
        let mut decisions = self.decisions.lock();
        for (pos, id) in plan.start.iter().enumerate() {
            decisions.insert(*id, Decision { reason: None, text: None, position: u32c(pos + 1) });
        }
        let offset = plan.start.len();
        for (i, (id, d)) in plan.deferred.iter().enumerate() {
            decisions.insert(
                *id,
                Decision { reason: Some(wait_reason(d)), text: Some(d.to_string()), position: u32c(offset + i + 1) },
            );
        }
        drop(decisions);
        AdmitOutcome {
            start: plan.start.into_iter().filter(|id| mine.contains(id)).collect(),
            deferred: plan.deferred.into_iter().filter(|(id, _)| mine.contains(id)).collect(),
        }
    }

    /// Registers the run's ready tasks as held back for a run-level reason
    /// (paused, agent budget) — they are not admitted and do not reserve slots.
    pub fn hold(&self, run_id: ManagerRunId, project_id: ProjectId, run_cap: usize, candidates: Vec<Candidate>, why: WaitReason) {
        let text = hold_text(&why);
        {
            let mut decisions = self.decisions.lock();
            for (i, c) in candidates.iter().enumerate() {
                decisions.insert(
                    c.sched.task_id,
                    Decision { reason: Some(why.clone()), text: Some(text.clone()), position: u32c(i + 1) },
                );
            }
        }
        {
            let mut tr = self.task_runs.lock();
            for c in &candidates {
                tr.insert(c.sched.task_id, run_id);
            }
        }
        self.queues.lock().insert(
            run_id,
            RunQueue {
                project_id,
                run_cap,
                paused: matches!(why, WaitReason::Paused),
                hold: Some(why),
                entries: candidates,
                updated: Instant::now(),
            },
        );
    }

    /// Removes a run (its supervisor exited).
    pub fn unregister(&self, run_id: ManagerRunId) {
        let removed = self.queues.lock().remove(&run_id);
        if let Some(q) = removed {
            let mut d = self.decisions.lock();
            for e in &q.entries {
                d.remove(&e.sched.task_id);
            }
        }
        self.task_runs.lock().retain(|_, r| *r != run_id);
    }

    /// The run a task was scheduled by, if live.
    #[must_use]
    pub fn run_of(&self, task: TaskId) -> Option<ManagerRunId> {
        let run = self.task_runs.lock().get(&task).copied()?;
        self.queues.lock().contains_key(&run).then_some(run)
    }

    /// English reason the run's tasks wait, if all of them are held back
    /// (for the run card).
    #[must_use]
    pub fn run_wait_summary(&self, run_id: ManagerRunId) -> Option<String> {
        let queues = self.queues.lock();
        let q = queues.get(&run_id)?;
        if q.entries.is_empty() {
            return None;
        }
        let decisions = self.decisions.lock();
        let texts: Vec<String> = q
            .entries
            .iter()
            .filter_map(|e| decisions.get(&e.sched.task_id).and_then(|d| d.text.clone()))
            .collect();
        let first = texts.first()?.clone();
        Some(if texts.len() > 1 { format!("{} task(s) {first}", texts.len()) } else { first })
    }

    /// Remembers the branch an interrupted task's work continues from.
    pub fn mark_resume(&self, task: TaskId, branch: String) {
        self.resume_branches.lock().insert(task, branch);
    }

    /// Takes the resume branch of a task (once).
    #[must_use]
    pub fn take_resume(&self, task: TaskId) -> Option<String> {
        self.resume_branches.lock().remove(&task)
    }

    /// Predicted head model of an agent for `role` in `project` (cached 30 s):
    /// explicit model, else the template's preferred model, else the router.
    pub async fn predict_model(
        &self,
        models: &Arc<dyn ModelGateway>,
        project: &Project,
        role: &AgentRole,
        explicit: Option<ModelRef>,
        preferred: Option<ModelRef>,
    ) -> Option<ModelRef> {
        if let Some(m) = explicit.or(preferred) {
            return Some(m);
        }
        let key = (project.id, crate::util::role_name(role));
        if let Some((at, m)) = self.model_cache.lock().get(&key) {
            if at.elapsed() < MODEL_CACHE_TTL {
                return m.clone();
            }
        }
        let routed = models
            .route(&RouteRequest {
                purpose: Some(key.1.clone()),
                required: orkestr_providers::RequiredCapabilities { tools: true, ..Default::default() },
                policy: project.privacy.ai.clone(),
                est_input_tokens: 4_000,
                max_output_tokens: 8_192,
                ..RouteRequest::default()
            })
            .await
            .ok()
            .and_then(|c| c.into_iter().next());
        self.model_cache.lock().insert(key, (Instant::now(), routed.clone()));
        routed
    }

    // ------------------------------------------------------------------
    // merge risks
    // ------------------------------------------------------------------

    /// Flags agents in separate worktrees whose file sets overlap and confirms
    /// with `git merge-tree` when both have branches. Throttled per project.
    /// Returns newly detected (or changed) risks; they are also published.
    pub async fn check_merge_risks(&self, project: &Project, items: &[RiskItem], git: bool) -> Vec<MergeRisk> {
        if !self.settings.read().merge_risk_checks {
            return Vec::new();
        }
        {
            let mut checked = self.risk_checked.lock();
            if checked.get(&project.id).is_some_and(|t| t.elapsed() < RISK_CHECK_INTERVAL) {
                return Vec::new();
            }
            checked.insert(project.id, Instant::now());
        }
        let ids: HashSet<TaskId> = items.iter().map(|i| i.task_id).collect();
        // Forget risks of tasks that are no longer in flight in this project.
        self.risks.lock().retain(|(a, b), r| r.project_id != project.id || (ids.contains(a) && ids.contains(b)));
        let root = project.root_path.as_ref().map(std::path::PathBuf::from).filter(|p| p.is_dir());
        let mut fresh = Vec::new();
        let mut previews = 0usize;
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                let (Some(wa), Some(wb)) = (&a.worktree, &b.worktree) else { continue };
                if wa == wb {
                    continue; // same worktree: the scheduler never runs them together
                }
                let overlap: BTreeSet<String> = a
                    .files
                    .iter()
                    .filter(|f| first_overlap(std::slice::from_ref(*f), &b.files).is_some())
                    .cloned()
                    .chain(b.files.iter().filter(|f| first_overlap(std::slice::from_ref(*f), &a.files).is_some()).cloned())
                    .collect();
                if overlap.is_empty() {
                    continue;
                }
                let mut conflicts = Vec::new();
                if let (true, Some(root), Some(ba), Some(bb)) = (git, &root, &a.branch, &b.branch) {
                    if previews < MAX_RISK_PREVIEWS {
                        previews += 1;
                        match self.deps.vcs.git().merge_preview(root, ba, Some(bb)).await {
                            Ok(p) => conflicts = p.conflicts.into_iter().map(|c| c.path).collect(),
                            Err(e) => tracing::debug!(error = %e, "merge-risk preview failed"),
                        }
                    }
                }
                let key = if a.task_id < b.task_id { (a.task_id, b.task_id) } else { (b.task_id, a.task_id) };
                let risk = MergeRisk {
                    project_id: project.id,
                    task_a: key.0,
                    task_b: key.1,
                    branch_a: if key.0 == a.task_id { a.branch.clone() } else { b.branch.clone() },
                    branch_b: if key.0 == a.task_id { b.branch.clone() } else { a.branch.clone() },
                    files: overlap.into_iter().take(50).collect(),
                    conflicts,
                    detected_at: now(),
                };
                let changed = {
                    let mut risks = self.risks.lock();
                    let changed = risks.get(&key).is_none_or(|old| old.files != risk.files || old.conflicts != risk.conflicts);
                    if changed {
                        risks.insert(key, risk.clone());
                    }
                    changed
                };
                if changed {
                    fresh.push(risk);
                }
            }
        }
        for r in &fresh {
            tracing::info!(project_id = %r.project_id, task_a = %r.task_a, task_b = %r.task_b, files = r.files.len(), conflicts = r.conflicts.len(), "merge risk detected");
            let ev = NewEvent::new(EventKind::MergeRiskDetected(MergeRiskPayload { risk: r.clone() })).project(r.project_id);
            if let Err(e) = self.deps.events.publish(ev).await {
                tracing::debug!(error = %e, "could not publish MergeRiskDetected");
            }
        }
        fresh
    }

    /// Known merge risks (optionally of one project).
    #[must_use]
    pub fn merge_risks(&self, project: Option<ProjectId>) -> Vec<MergeRisk> {
        self.risks.lock().values().filter(|r| project.is_none_or(|p| r.project_id == p)).cloned().collect()
    }

    // ------------------------------------------------------------------
    // status / explain
    // ------------------------------------------------------------------

    async fn active_tasks(&self) -> OrchResult<Vec<orkestr_domain::Task>> {
        Ok(self
            .deps
            .tasks
            .list(ListTasksParams { status: Some(ACTIVE_STATUSES.to_vec()), ..ListTasksParams::default() })
            .await?)
    }

    fn resources_view(&self) -> (ResourceSnapshot, SchedulerResources) {
        let r = self.deps.resources.snapshot();
        let sampled = r != ResourceSnapshot::unconstrained();
        (
            r,
            SchedulerResources {
                sampled,
                cpu_percent: if sampled { r.cpu_usage_percent } else { 0.0 },
                logical_cores: r.logical_cores,
                ram_available_mb: if sampled { r.ram_available_mb } else { 0 },
                ram_total_mb: if sampled { r.ram_total_mb } else { 0 },
                disk_free_mb: r.disk_free_mb,
                gpu_percent: r.gpu_usage_percent,
            },
        )
    }

    /// `scheduler.status`.
    pub async fn status(&self, project: Option<ProjectId>) -> OrchResult<SchedulerStatus> {
        let at = now();
        let active = self.active_tasks().await?;
        *self.active_count.lock() = active.len();
        let settings = self.settings();
        let limits = self.current_limits();
        let aging = limits.aging;
        let task_runs = self.task_runs.lock().clone();
        let running: Vec<SchedEntry> = active
            .iter()
            .filter(|t| project.is_none_or(|p| t.project_id == p))
            .map(|t| {
                let s = SchedTask::from_task(t);
                SchedEntry {
                    task_id: t.id,
                    project_id: t.project_id,
                    run_id: task_runs.get(&t.id).copied(),
                    title: t.title.clone(),
                    state: SchedEntryState::Running,
                    status: t.status,
                    priority: t.priority,
                    effective_priority: t.priority,
                    since: t.started_at.unwrap_or(t.updated_at),
                    provider_id: s.provider,
                    model: s.model,
                    team: s.team,
                    agent_id: t.agent_id,
                    reason: None,
                    reason_text: None,
                    position: None,
                }
            })
            .collect();
        let active_ids: HashSet<TaskId> = active.iter().map(|t| t.id).collect();
        let mut queue: Vec<SchedEntry> = Vec::new();
        let mut project_queued: BTreeMap<ProjectId, u32> = BTreeMap::new();
        {
            let queues = self.queues.lock();
            let decisions = self.decisions.lock();
            let mut seen = HashSet::new();
            for (run_id, q) in queues.iter().filter(|(_, q)| q.updated.elapsed() < QUEUE_TTL) {
                for e in &q.entries {
                    let id = e.sched.task_id;
                    if active_ids.contains(&id) || !seen.insert(id) {
                        continue;
                    }
                    *project_queued.entry(q.project_id).or_default() += 1;
                    if project.is_some_and(|p| p != q.project_id) {
                        continue;
                    }
                    let d = decisions.get(&id);
                    let reason = q.hold.clone().or_else(|| d.and_then(|d| d.reason.clone()));
                    let text = q.hold.as_ref().map(hold_text).or_else(|| d.and_then(|d| d.text.clone()));
                    queue.push(SchedEntry {
                        task_id: id,
                        project_id: q.project_id,
                        run_id: Some(*run_id),
                        title: e.title.clone(),
                        state: if reason.is_some() { SchedEntryState::Waiting } else { SchedEntryState::Queued },
                        status: TaskStatus::Ready,
                        priority: e.sched.priority,
                        effective_priority: effective_priority(e.sched.priority, e.sched.enqueued_at, at, aging),
                        since: e.sched.enqueued_at,
                        provider_id: e.sched.provider.clone(),
                        model: e.sched.model.clone(),
                        team: e.sched.team.clone(),
                        agent_id: None,
                        reason,
                        reason_text: text,
                        position: d.map(|d| d.position),
                    });
                }
            }
        }
        queue.sort_by(|a, b| {
            a.position.unwrap_or(u32::MAX).cmp(&b.position.unwrap_or(u32::MAX)).then(a.since.cmp(&b.since))
        });
        let counts = self.counts();

        // Providers.
        let mut providers: BTreeMap<String, ProviderLoad> = BTreeMap::new();
        for p in self.deps.hub.providers().into_iter().filter(|p| p.enabled) {
            providers.insert(
                p.id.clone(),
                ProviderLoad {
                    provider_id: p.id.clone(),
                    name: Some(p.name),
                    running_agents: 0,
                    agent_limit: limits.provider_slots.get(&p.id).map(|v| u32c(*v)),
                    in_flight: 0,
                    queued_requests: 0,
                    rate_limited_until: None,
                    rate_limit_hits: 0,
                },
            );
        }
        let blank = |id: &str| ProviderLoad {
            provider_id: id.to_owned(),
            name: None,
            running_agents: 0,
            agent_limit: limits.provider_slots.get(id).map(|v| u32c(*v)),
            in_flight: 0,
            queued_requests: 0,
            rate_limited_until: None,
            rate_limit_hits: 0,
        };
        for t in &active {
            if let Some(m) = &t.model {
                let id = m.provider_id.to_string();
                providers.entry(id.clone()).or_insert_with(|| blank(&id)).running_agents += 1;
            }
        }
        {
            let g = self.gate.lock();
            for (id, n) in &g.in_flight {
                providers.entry(id.clone()).or_insert_with(|| blank(id)).in_flight = *n;
            }
            for (id, n) in &g.queued {
                providers.entry(id.clone()).or_insert_with(|| blank(id)).queued_requests = *n;
            }
        }
        for (id, st) in self.rate.lock().iter() {
            let e = providers.entry(id.clone()).or_insert_with(|| blank(id));
            e.rate_limit_hits = st.hits;
            e.rate_limited_until = (st.until > at).then_some(st.until);
        }

        // Projects.
        let mut projects: BTreeMap<ProjectId, ProjectLoad> = BTreeMap::new();
        let cap = |p: ProjectId| {
            u32c(limits.project_overrides.get(&p).copied().unwrap_or(limits.max_parallel_per_project))
        };
        for t in &active {
            projects
                .entry(t.project_id)
                .or_insert_with(|| ProjectLoad { project_id: t.project_id, running: 0, queued: 0, limit: cap(t.project_id) })
                .running += 1;
        }
        for (p, n) in project_queued {
            projects.entry(p).or_insert_with(|| ProjectLoad { project_id: p, running: 0, queued: 0, limit: cap(p) }).queued = n;
        }

        Ok(SchedulerStatus {
            settings,
            counts,
            running,
            queue,
            providers: providers.into_values().collect(),
            projects: projects.into_values().filter(|l| project.is_none_or(|p| p == l.project_id)).collect(),
            resources: self.resources_view().1,
            merge_risks: self.merge_risks(project),
            updated_at: at,
        })
    }

    /// `scheduler.explain`.
    pub async fn explain(&self, task_id: TaskId) -> OrchResult<SchedulerExplanation> {
        let task = self.deps.tasks.get(task_id).await?;
        let status = self.status(Some(task.project_id)).await?;
        let entry = status.running.iter().chain(status.queue.iter()).find(|e| e.task_id == task_id).cloned();
        let queued_candidate = {
            let queues = self.queues.lock();
            queues
                .values()
                .filter(|q| q.updated.elapsed() < QUEUE_TTL)
                .flat_map(|q| q.entries.iter())
                .find(|e| e.sched.task_id == task_id)
                .map(|e| e.sched.clone())
        };
        let scheduled = queued_candidate.is_some() || entry.as_ref().is_some_and(|e| e.run_id.is_some());
        let candidate = queued_candidate.unwrap_or_else(|| SchedTask::from_task(&task));
        let active: Vec<SchedTask> = self
            .active_tasks()
            .await?
            .iter()
            .filter(|t| t.id != task_id)
            .map(SchedTask::from_task)
            .collect();
        let (resources, _) = self.resources_view();
        let checks: Vec<AdmissionCheckResult> = Scheduler::new(self.current_limits())
            .evaluate(&candidate, &active, &resources, now())
            .into_iter()
            .map(|c| AdmissionCheckResult {
                check: c.name.to_owned(),
                ok: c.deferral.is_none(),
                reason: c.deferral.as_ref().map(wait_reason),
                reason_text: c.deferral.as_ref().map(ToString::to_string),
            })
            .collect();
        let first_fail = checks.iter().find(|c| !c.ok).and_then(|c| c.reason_text.clone());
        let summary = match task.status {
            TaskStatus::Running | TaskStatus::Waiting => match task.agent_id {
                Some(a) => format!("running: agent {a} works on it"),
                None => "running".into(),
            },
            TaskStatus::Ready => match (&entry, scheduled) {
                (Some(e), true) if e.reason_text.is_some() => e.reason_text.clone().unwrap_or_default(),
                (_, true) => first_fail.unwrap_or_else(|| "queued: starts on the next scheduling pass".into()),
                (_, false) => format!(
                    "ready, but no live Manager run schedules it — start an agent for it or run its plan{}",
                    first_fail.map(|f| format!(" ({f})")).unwrap_or_default()
                ),
            },
            TaskStatus::Planned | TaskStatus::Blocked => "not ready: waiting for its prerequisites".into(),
            TaskStatus::Interrupted => "interrupted: resume it (or its run) to continue".into(),
            TaskStatus::Recovering => "recovering: about to be queued again".into(),
            TaskStatus::Review | TaskStatus::Testing => "done by the agent: waiting for review / tests".into(),
            other => format!("finished ({other:?})"),
        };
        Ok(SchedulerExplanation { task_id, status: task.status, entry, scheduled, checks, summary })
    }
}

/// Request permits held while a model request streams.
struct InFlight {
    sched: Weak<ParallelScheduler>,
    provider: String,
    _permits: Vec<OwnedSemaphorePermit>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(s) = self.sched.upgrade() {
            s.bump(|g| &mut g.in_flight, &self.provider, false);
        }
    }
}

// ---------------------------------------------------------------------------
// gated gateway
// ---------------------------------------------------------------------------

/// Model gateway with API concurrency limits (per provider: the adapter's
/// `maxConcurrency` or the scheduler override; per model: scheduler model
/// caps) and rate-limit observation.
pub struct GatedGateway {
    inner: Arc<dyn ModelGateway>,
    sched: Arc<ParallelScheduler>,
}

impl std::fmt::Debug for GatedGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GatedGateway")
    }
}

impl GatedGateway {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn ModelGateway>, sched: Arc<ParallelScheduler>) -> Self {
        Self { inner, sched }
    }
}

#[async_trait]
impl ModelGateway for GatedGateway {
    async fn route(&self, req: &RouteRequest) -> Result<Vec<ModelRef>, String> {
        self.inner.route(req).await
    }

    fn chat(
        &self,
        chain: Vec<ModelRef>,
        request: ChatRequest,
        hook: Option<Arc<dyn HandoffHook>>,
        cancel: CancellationToken,
    ) -> ChatStream {
        let Some(head) = chain.first().cloned() else {
            return self.inner.chat(chain, request, hook, cancel);
        };
        let inner = Arc::clone(&self.inner);
        let sched = Arc::clone(&self.sched);
        let fut = async move {
            let Some(guard) = sched.acquire(&head, &cancel).await else {
                return futures::stream::iter([ChatEvent::Error(ProviderError::Cancelled)]).boxed();
            };
            let observer = Arc::downgrade(&sched);
            inner
                .chat(chain, request, hook, cancel)
                .inspect(move |ev| {
                    let _hold = &guard; // permits live as long as the stream
                    let Some(s) = observer.upgrade() else { return };
                    match ev {
                        ChatEvent::Error(ProviderError::RateLimited { retry_after_secs }) => {
                            s.note_rate_limited(&guard.provider, *retry_after_secs);
                        }
                        ChatEvent::Switched { from, reason, .. } if reason == "rate_limited" => {
                            s.note_rate_limited(&from.provider_id.to_string(), None);
                        }
                        _ => {}
                    }
                })
                .boxed()
        };
        futures::stream::once(fut).flatten().boxed()
    }

    fn provider(&self, id: ProviderId) -> Option<ProviderInfo> {
        self.inner.provider(id)
    }

    fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.inner.adapter(id)
    }
}

// ---------------------------------------------------------------------------
// restart recovery
// ---------------------------------------------------------------------------

impl Manager {
    /// The global scheduler.
    #[must_use]
    pub fn parallel(&self) -> &Arc<ParallelScheduler> {
        &self.inner.parallel
    }

    /// Resumes work interrupted by a core restart when the policy allows
    /// (see module docs). Call after crash-recovery hooks ran.
    pub async fn auto_resume_interrupted(&self) -> AutoResumeReport {
        let mut report = AutoResumeReport::default();
        let settings = self.inner.parallel.settings();
        let cfg = self.inner.config().await;
        let all_stopped = self.inner.deps.security.emergency.flags().all_stopped;
        let max = settings.max_auto_resume_attempts;
        let ru = crate::util::is_ru(&self.inner.deps.config.ui_language.get());

        let runs = match self.inner.store.runs(None, Some(vec![ManagerRunStatus::Interrupted]), 200).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "auto-resume: cannot list interrupted runs");
                Vec::new()
            }
        };
        let mut run_tasks: HashSet<TaskId> = HashSet::new();
        if let Ok(open) = self
            .inner
            .store
            .runs(
                None,
                Some(vec![
                    ManagerRunStatus::Running,
                    ManagerRunStatus::Paused,
                    ManagerRunStatus::Interrupted,
                    ManagerRunStatus::Failed,
                ]),
                500,
            )
            .await
        {
            run_tasks.extend(open.iter().flat_map(|r| r.tasks.iter().map(|t| t.task_id)));
        }

        for run in runs {
            if self.inner.runs.lock().contains_key(&run.id) {
                continue;
            }
            let paused = run.status_reason.as_deref() == Some(PAUSED_INTERRUPTED_REASON);
            let mut over = false;
            for t in &run.tasks {
                if self.interruptions(t.task_id).await > max {
                    over = true;
                }
            }
            let policy_ok = match settings.auto_resume {
                AutoResumePolicy::Off => false,
                AutoResumePolicy::Always => true,
                AutoResumePolicy::FollowApproval => {
                    run.approval_policy != ApprovalMode::Ask && self.inner.policy(&cfg) != ApprovalMode::Ask
                }
            };
            let why_not = if all_stopped {
                Some(if ru { "активна экстренная остановка" } else { "Stop All AI is active" })
            } else if paused {
                Some(if ru { "вы поставили запуск на паузу" } else { "you had paused it" })
            } else if over {
                Some(if ru { "лимит автоматических возобновлений исчерпан" } else { "the automatic resume limit is reached" })
            } else if !policy_ok {
                Some(if ru { "политика подтверждений требует вашего решения" } else { "the approval policy asks for your decision" })
            } else {
                None
            };
            let extras = || MessageExtras { run_id: Some(run.id), plan_id: Some(run.plan_id), ..MessageExtras::default() };
            if why_not.is_none() {
                match self.start_run(ManagerRunParams { plan_id: None, run_id: Some(run.id), limits: None }).await {
                    Ok(_) => {
                        tracing::info!(run_id = %run.id, "interrupted run resumed automatically after restart");
                        report.resumed_runs.push(run.id);
                        let text = if ru {
                            "Запуск прерван перезапуском Orkestr и возобновлён автоматически с последних контрольных точек."
                        } else {
                            "The run was interrupted by an Orkestr restart and resumed automatically from the last checkpoints."
                        };
                        self.inner.system_event(run.project_id, text, extras()).await;
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(run_id = %run.id, error = %e, "automatic resume failed; waiting for the user");
                    }
                }
            }
            report.waiting_runs.push(run.id);
            let why = why_not.unwrap_or(if ru { "не удалось возобновить" } else { "it could not be resumed" });
            let text = if ru {
                format!("Запуск прерван перезапуском Orkestr и ждёт вас ({why}). Возобновите его, когда будете готовы.")
            } else {
                format!("The run was interrupted by an Orkestr restart and waits for you ({why}). Resume it when ready.")
            };
            self.inner.system_event(run.project_id, &text, extras()).await;
        }

        // Stand-alone agent tasks (not part of a Manager run).
        let interrupted = self
            .inner
            .deps
            .tasks
            .list(ListTasksParams { status: Some(vec![TaskStatus::Interrupted]), ..ListTasksParams::default() })
            .await
            .unwrap_or_default();
        for task in interrupted {
            if run_tasks.contains(&task.id) {
                continue;
            }
            let Some(agent_id) = task.agent_id else { continue };
            let Ok(agent) = self.inner.deps.agents.get(agent_id).await else { continue };
            let Some(template_id) = agent.template_id else { continue };
            let Ok(project) = self.inner.project(task.project_id).await else { continue };
            let policy_ok = match settings.auto_resume {
                AutoResumePolicy::Off => false,
                AutoResumePolicy::Always => true,
                AutoResumePolicy::FollowApproval => project.approval_mode != ApprovalMode::Ask,
            };
            if all_stopped || !policy_ok || self.interruptions(task.id).await > max {
                report.waiting_tasks.push(task.id);
                continue;
            }
            if let Err(e) = self.inner.deps.tasks.retry(task.id, TaskActor::SystemRecovery).await {
                tracing::warn!(task_id = %task.id, error = %e, "auto-resume: retry failed");
                report.waiting_tasks.push(task.id);
                continue;
            }
            let base = agent
                .worktree
                .as_ref()
                .filter(|_| agent.worktree_policy == WorktreePolicy::DedicatedWorktree)
                .map(|w| w.branch.clone())
                .filter(|b| b.starts_with("agent/"));
            let params = SpawnAgentParams {
                project_id: project.id,
                template_id,
                task_id: Some(task.id),
                new_task: None,
                model: agent.model.clone(),
                worktree_policy: Some(agent.worktree_policy),
                name: None,
                max_turns: Some(agent.max_turns).filter(|t| *t > 0),
                base_branch: base,
                approval_mode: None,
                budget: agent.budget.clone(),
            };
            match self.inner.deps.agents.spawn(params).await {
                Ok((a, _)) => {
                    tracing::info!(task_id = %task.id, agent_id = %a.id, "interrupted task resumed with a new agent");
                    report.resumed_tasks.push(task.id);
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, error = %e, "auto-resume: agent spawn failed; waiting for the user");
                    report.waiting_tasks.push(task.id);
                }
            }
        }
        if !report.resumed_runs.is_empty() || !report.resumed_tasks.is_empty() || !report.waiting_runs.is_empty() {
            tracing::info!(
                resumed_runs = report.resumed_runs.len(),
                waiting_runs = report.waiting_runs.len(),
                resumed_tasks = report.resumed_tasks.len(),
                waiting_tasks = report.waiting_tasks.len(),
                "restart recovery finished"
            );
        }
        report
    }

    /// How often a task was interrupted (persisted history).
    async fn interruptions(&self, task: TaskId) -> u32 {
        match self.inner.deps.tasks.history(task).await {
            Ok((h, _)) => u32c(h.iter().filter(|e| e.to == TaskStatus::Interrupted).count()),
            Err(_) => 0,
        }
    }
}

impl Inner {
    /// Scheduling view of an active task for the supervisor (agents with their
    /// own worktree never conflict on files when git is available).
    pub(crate) fn active_sched(t: &orkestr_domain::Task, git: bool) -> SchedTask {
        let mut s = SchedTask::from_task(t);
        if !git {
            s.worktree = None;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_reasons_map_one_to_one() {
        let d = Deferral::ProviderBusy { provider: "p".into(), running: 2, limit: 2 };
        assert_eq!(wait_reason(&d), WaitReason::ProviderConcurrency { provider_id: "p".into(), running: 2, limit: 2 });
        assert_eq!(d.to_string(), "waiting: provider concurrency 2/2 (p)");
        assert_eq!(hold_text(&WaitReason::Paused), "waiting: the run is paused");
        assert_eq!(hold_text(&WaitReason::AgentBudget { spawned: 3, limit: 3 }), "waiting: run agent limit 3/3");
    }
}
