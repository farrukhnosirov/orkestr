//! Resource-aware scheduler (§11, §38, §68.24–26, task-state-machine.md §7.2–7.3).
//!
//! Pure logic: given the queued `Ready` candidates, the currently active
//! tasks, the limits and a resource snapshot, [`Scheduler::next_runnable`]
//! returns which tasks may start now (with **backfilling**: an inadmissible
//! head does not block admissible tasks behind it) and why the others were
//! deferred ([`Deferral`], with the numbers behind the decision so the UI can
//! say "waiting: provider concurrency 2/2").
//!
//! Service order: effective priority (base priority + aging), then — with
//! [`SchedulerLimits::fair_share`] — the project with the fewest running and
//! already admitted tasks first (fairness across projects, §12/§13), then
//! FIFO.
//!
//! Admission checks, in order ([`Scheduler::evaluate`] reports all of them):
//! 1. global parallel cap; 2. per-project cap; 3. per-team cap;
//! 4. provider rate-limit back-off; 5. provider concurrency slots;
//! 6. model concurrency slots; 7. CPU threshold (current usage + projected
//!    load of admitted tasks); 8. free-RAM reserve; 9. free-disk reserve;
//! 10. GPU threshold (when the host reports GPU usage);
//! 11. **file-overlap conflict avoidance** — two tasks of the same project
//!     whose file sets overlap never run concurrently unless both have their
//!     own, different worktrees (then the merge manager / merge-risk check
//!     handles it later).

use std::collections::BTreeMap;

use orkestr_domain::{ProjectId, Task, TaskId, TaskPriority, Timestamp};

use crate::queue::{effective_priority, AgingPolicy, PriorityQueue, QueueEntry};

/// Snapshot of host resources (from `orkestr-node`/platform sampling).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceSnapshot {
    /// Current total CPU usage, 0–100.
    pub cpu_usage_percent: f32,
    /// Logical cores.
    pub logical_cores: u32,
    /// Total RAM in MiB.
    pub ram_total_mb: u64,
    /// Available RAM in MiB.
    pub ram_available_mb: u64,
    /// Free space on the data volume in MiB (`None` = unknown, never limits).
    pub disk_free_mb: Option<u64>,
    /// GPU utilisation 0–100 (`None` = not sampled, never limits).
    pub gpu_usage_percent: Option<f32>,
}

impl ResourceSnapshot {
    /// A snapshot that never limits (tests, unknown resources).
    #[must_use]
    pub fn unconstrained() -> Self {
        Self {
            cpu_usage_percent: 0.0,
            logical_cores: 64,
            ram_total_mb: u64::MAX / 4,
            ram_available_mb: u64::MAX / 4,
            disk_free_mb: None,
            gpu_usage_percent: None,
        }
    }
}

/// Scheduling limits.
#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerLimits {
    /// Max tasks running at once across all projects.
    pub max_parallel_global: usize,
    /// Default max running tasks per project.
    pub max_parallel_per_project: usize,
    /// Per-project overrides.
    pub project_overrides: BTreeMap<ProjectId, usize>,
    /// Max running tasks per team (tasks tagged `team:<name>`); absent = unlimited.
    pub team_caps: BTreeMap<String, usize>,
    /// Concurrency slots per provider id (absent provider = unlimited).
    pub provider_slots: BTreeMap<String, usize>,
    /// Concurrency slots per model (`<provider id>/<model>`; absent = unlimited).
    pub model_slots: BTreeMap<String, usize>,
    /// Providers in rate-limit back-off → until when (no new work before).
    pub rate_limited: BTreeMap<String, Timestamp>,
    /// Do not start new work above this projected CPU usage (percent).
    pub max_cpu_percent: f32,
    /// RAM that must stay free after admitting a task (MiB).
    pub min_free_ram_mb: u64,
    /// Disk space that must stay free on the data volume (MiB).
    pub min_free_disk_mb: u64,
    /// Do not start new work while the GPU is busier than this (percent).
    pub max_gpu_percent: Option<f32>,
    /// RAM assumed for a task without an estimate (MiB).
    pub default_task_ram_mb: u64,
    /// CPU weight (cores) assumed for a task without an estimate.
    pub default_cpu_weight: f32,
    /// Aging for queue order.
    pub aging: AgingPolicy,
    /// Prefer the least-loaded project among equally urgent candidates.
    pub fair_share: bool,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_parallel_global: 8,
            max_parallel_per_project: 4,
            project_overrides: BTreeMap::new(),
            team_caps: BTreeMap::new(),
            provider_slots: BTreeMap::new(),
            model_slots: BTreeMap::new(),
            rate_limited: BTreeMap::new(),
            max_cpu_percent: 85.0,
            min_free_ram_mb: 2048,
            min_free_disk_mb: 1024,
            max_gpu_percent: None,
            default_task_ram_mb: 512,
            default_cpu_weight: 0.5,
            aging: AgingPolicy::default(),
            fair_share: true,
        }
    }
}

/// Scheduling-relevant view of a task (queued or active).
#[derive(Debug, Clone, PartialEq)]
pub struct SchedTask {
    /// Task.
    pub task_id: TaskId,
    /// Project.
    pub project_id: ProjectId,
    /// Base priority.
    pub priority: TaskPriority,
    /// Time it became `Ready`.
    pub enqueued_at: Timestamp,
    /// Provider id whose slot it needs.
    pub provider: Option<String>,
    /// Model key (`<provider id>/<model>`) whose slot it needs.
    pub model: Option<String>,
    /// Team (`team:<name>` tag).
    pub team: Option<String>,
    /// Files it touches (relative to the project root): predicted + actual.
    pub files: Vec<String>,
    /// Its own worktree path, if any.
    pub worktree: Option<String>,
    /// RAM estimate (MiB).
    pub ram_mb: Option<u64>,
    /// CPU estimate (cores).
    pub cpu_weight: Option<f32>,
}

/// Prefix of the task tag naming its team.
pub const TEAM_TAG_PREFIX: &str = "team:";

/// Team of a task from its `team:<name>` tag.
#[must_use]
pub fn team_of(tags: &[String]) -> Option<String> {
    tags.iter().find_map(|t| t.strip_prefix(TEAM_TAG_PREFIX)).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

impl SchedTask {
    /// A candidate with only the identity fields set.
    #[must_use]
    pub fn new(task_id: TaskId, project_id: ProjectId, priority: TaskPriority, enqueued_at: Timestamp) -> Self {
        Self {
            task_id,
            project_id,
            priority,
            enqueued_at,
            provider: None,
            model: None,
            team: None,
            files: Vec::new(),
            worktree: None,
            ram_mb: None,
            cpu_weight: None,
        }
    }

    /// The scheduling view of a persisted task (model, team tag, touched
    /// files and worktree as recorded on the task).
    #[must_use]
    pub fn from_task(t: &Task) -> Self {
        Self {
            provider: t.model.as_ref().map(|m| m.provider_id.to_string()),
            model: t.model.as_ref().map(|m| format!("{}/{}", m.provider_id, m.model)),
            team: team_of(&t.tags),
            files: t.files_touched.clone(),
            worktree: t.worktree.as_ref().map(|w| w.path.clone()),
            ..Self::new(t.id, t.project_id, t.priority, t.ready_since.unwrap_or(t.updated_at))
        }
    }
}

/// Why a candidate was not started (with the numbers behind the decision).
#[derive(Debug, Clone, PartialEq)]
pub enum Deferral {
    /// Global parallel cap reached.
    GlobalLimit {
        /// Running + admitted.
        running: usize,
        /// Cap.
        limit: usize,
    },
    /// Project parallel cap reached.
    ProjectLimit {
        /// Running + admitted in the project.
        running: usize,
        /// Cap.
        limit: usize,
    },
    /// Team parallel cap reached.
    TeamLimit {
        /// Team.
        team: String,
        /// Running + admitted in the team.
        running: usize,
        /// Cap.
        limit: usize,
    },
    /// The provider is in rate-limit back-off.
    RateLimited {
        /// Provider.
        provider: String,
        /// Back-off end.
        until: Timestamp,
    },
    /// No free provider slot.
    ProviderBusy {
        /// Provider.
        provider: String,
        /// Running + admitted on it.
        running: usize,
        /// Slots.
        limit: usize,
    },
    /// No free model slot.
    ModelBusy {
        /// Model key.
        model: String,
        /// Running + admitted on it.
        running: usize,
        /// Slots.
        limit: usize,
    },
    /// CPU above threshold.
    CpuBusy {
        /// Projected usage with this task (percent).
        projected_percent: f32,
        /// Threshold.
        limit_percent: f32,
    },
    /// Not enough free RAM.
    LowMemory {
        /// Free RAM before this task (MiB).
        free_mb: u64,
        /// Task estimate + reserve (MiB).
        needed_mb: u64,
    },
    /// Not enough free disk space.
    LowDisk {
        /// Free disk (MiB).
        free_mb: u64,
        /// Reserve (MiB).
        min_mb: u64,
    },
    /// GPU above threshold.
    GpuBusy {
        /// Current usage.
        usage_percent: f32,
        /// Threshold.
        limit_percent: f32,
    },
    /// Overlaps files with an active/admitted task in the same worktree.
    FileConflict {
        /// The conflicting task.
        with: TaskId,
        /// One overlapping path.
        path: String,
    },
}

impl Deferral {
    /// Stable machine code (`globalLimit`, `providerBusy`, …).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Deferral::GlobalLimit { .. } => "globalLimit",
            Deferral::ProjectLimit { .. } => "projectLimit",
            Deferral::TeamLimit { .. } => "teamLimit",
            Deferral::RateLimited { .. } => "rateLimited",
            Deferral::ProviderBusy { .. } => "providerBusy",
            Deferral::ModelBusy { .. } => "modelBusy",
            Deferral::CpuBusy { .. } => "cpuBusy",
            Deferral::LowMemory { .. } => "lowMemory",
            Deferral::LowDisk { .. } => "lowDisk",
            Deferral::GpuBusy { .. } => "gpuBusy",
            Deferral::FileConflict { .. } => "fileConflict",
        }
    }
}

impl std::fmt::Display for Deferral {
    /// English explanation for logs and the Manager ("waiting: provider concurrency 2/2").
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Deferral::GlobalLimit { running, limit } => write!(f, "waiting: global parallel limit {running}/{limit}"),
            Deferral::ProjectLimit { running, limit } => write!(f, "waiting: project parallel limit {running}/{limit}"),
            Deferral::TeamLimit { team, running, limit } => write!(f, "waiting: team {team} limit {running}/{limit}"),
            Deferral::RateLimited { provider, until } => {
                write!(f, "waiting: provider {provider} is rate-limited until {}", until.format("%H:%M:%S"))
            }
            Deferral::ProviderBusy { provider, running, limit } => {
                write!(f, "waiting: provider concurrency {running}/{limit} ({provider})")
            }
            Deferral::ModelBusy { model, running, limit } => {
                write!(f, "waiting: model concurrency {running}/{limit} ({model})")
            }
            Deferral::CpuBusy { projected_percent, limit_percent } => {
                write!(f, "waiting: CPU would reach {projected_percent:.0}% (limit {limit_percent:.0}%)")
            }
            Deferral::LowMemory { free_mb, needed_mb } => {
                write!(f, "waiting: {free_mb} MiB RAM free, {needed_mb} MiB needed")
            }
            Deferral::LowDisk { free_mb, min_mb } => write!(f, "waiting: {free_mb} MiB disk free (reserve {min_mb} MiB)"),
            Deferral::GpuBusy { usage_percent, limit_percent } => {
                write!(f, "waiting: GPU at {usage_percent:.0}% (limit {limit_percent:.0}%)")
            }
            Deferral::FileConflict { with, path } => write!(f, "waiting: {path} is being edited by task {with}"),
        }
    }
}

/// Output of [`Scheduler::next_runnable`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedulePlan {
    /// Tasks to start now, in order.
    pub start: Vec<TaskId>,
    /// Deferred candidates with reasons, in service order.
    pub deferred: Vec<(TaskId, Deferral)>,
}

/// One admission check of [`Scheduler::evaluate`].
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionCheck {
    /// Check name (`global`, `project`, `team`, `rateLimit`, `provider`,
    /// `model`, `cpu`, `memory`, `disk`, `gpu`, `files`).
    pub name: &'static str,
    /// `None` = passes.
    pub deferral: Option<Deferral>,
}

/// Normalizes a path for overlap checks (`\` → `/`, no `./`, no trailing `/`,
/// ASCII-lowercase because Windows file systems are case-insensitive).
fn norm(p: &str) -> String {
    let mut s = p.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s.to_ascii_lowercase()
}

/// Whether two paths overlap (equal, or one is a directory prefix of the other).
#[must_use]
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// First overlapping path between two file sets, if any.
#[must_use]
pub fn first_overlap(a: &[String], b: &[String]) -> Option<String> {
    a.iter().find(|fa| b.iter().any(|fb| paths_overlap(fa, fb))).cloned()
}

/// First overlapping path between two tasks that would conflict, if any.
#[must_use]
pub fn file_conflict(a: &SchedTask, b: &SchedTask) -> Option<String> {
    if a.project_id != b.project_id {
        return None;
    }
    if let (Some(wa), Some(wb)) = (&a.worktree, &b.worktree) {
        if norm(wa) != norm(wb) {
            return None; // separate worktrees: merge manager resolves later (§16)
        }
    }
    first_overlap(&a.files, &b.files)
}

/// Load counters while a plan is built.
#[derive(Debug, Clone)]
struct Load<'a> {
    running: Vec<&'a SchedTask>,
    per_project: BTreeMap<ProjectId, usize>,
    per_team: BTreeMap<String, usize>,
    per_provider: BTreeMap<String, usize>,
    per_model: BTreeMap<String, usize>,
    cpu: f32,
    free_ram: u64,
}

impl<'a> Load<'a> {
    fn new(active: &'a [SchedTask], resources: &ResourceSnapshot) -> Self {
        let mut load = Self {
            running: Vec::new(),
            per_project: BTreeMap::new(),
            per_team: BTreeMap::new(),
            per_provider: BTreeMap::new(),
            per_model: BTreeMap::new(),
            cpu: resources.cpu_usage_percent,
            free_ram: resources.ram_available_mb,
        };
        for t in active {
            load.count(t);
        }
        load
    }

    fn count(&mut self, t: &'a SchedTask) {
        *self.per_project.entry(t.project_id).or_default() += 1;
        if let Some(team) = &t.team {
            *self.per_team.entry(team.clone()).or_default() += 1;
        }
        if let Some(p) = &t.provider {
            *self.per_provider.entry(p.clone()).or_default() += 1;
        }
        if let Some(m) = &t.model {
            *self.per_model.entry(m.clone()).or_default() += 1;
        }
        self.running.push(t);
    }
}

/// The scheduler (stateless; limits are configuration).
#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    /// Limits.
    pub limits: SchedulerLimits,
}

impl Scheduler {
    /// New scheduler.
    #[must_use]
    pub fn new(limits: SchedulerLimits) -> Self {
        Self { limits }
    }

    fn ram(&self, t: &SchedTask) -> u64 {
        t.ram_mb.unwrap_or(self.limits.default_task_ram_mb)
    }

    fn cpu_percent(&self, t: &SchedTask, cores: u32) -> f32 {
        let w = t.cpu_weight.unwrap_or(self.limits.default_cpu_weight).max(0.0);
        // Precision loss is irrelevant for core counts.
        #[allow(clippy::cast_precision_loss)]
        let cores = cores.max(1) as f32;
        w / cores * 100.0
    }

    fn project_cap(&self, p: ProjectId) -> usize {
        self.limits.project_overrides.get(&p).copied().unwrap_or(self.limits.max_parallel_per_project)
    }

    /// Orders candidates with the priority queue (aging at `now`), without
    /// fairness.
    #[must_use]
    pub fn order<'a>(&self, candidates: &'a [SchedTask], now: Timestamp) -> Vec<&'a SchedTask> {
        let q = PriorityQueue::from_entries(
            self.limits.aging,
            candidates.iter().map(|c| QueueEntry {
                task_id: c.task_id,
                priority: c.priority,
                enqueued_at: c.enqueued_at,
            }),
        );
        let by_id: BTreeMap<TaskId, &SchedTask> = candidates.iter().map(|c| (c.task_id, c)).collect();
        q.ordered(now).into_iter().filter_map(|(e, _)| by_id.get(&e.task_id).copied()).collect()
    }

    /// Every admission check for `c` against the current load.
    fn checks(&self, c: &SchedTask, load: &Load<'_>, resources: &ResourceSnapshot, now: Timestamp) -> Vec<AdmissionCheck> {
        let l = &self.limits;
        let mut out = Vec::with_capacity(11);
        let mut push = |name: &'static str, deferral: Option<Deferral>| out.push(AdmissionCheck { name, deferral });

        let running = load.running.len();
        push(
            "global",
            (running >= l.max_parallel_global)
                .then_some(Deferral::GlobalLimit { running, limit: l.max_parallel_global }),
        );
        let in_project = load.per_project.get(&c.project_id).copied().unwrap_or(0);
        let cap = self.project_cap(c.project_id);
        push("project", (in_project >= cap).then_some(Deferral::ProjectLimit { running: in_project, limit: cap }));
        push(
            "team",
            c.team.as_ref().and_then(|team| {
                let limit = *l.team_caps.get(team)?;
                let running = load.per_team.get(team).copied().unwrap_or(0);
                (running >= limit).then(|| Deferral::TeamLimit { team: team.clone(), running, limit })
            }),
        );
        push(
            "rateLimit",
            c.provider.as_ref().and_then(|p| {
                let until = *l.rate_limited.get(p)?;
                (until > now).then(|| Deferral::RateLimited { provider: p.clone(), until })
            }),
        );
        push(
            "provider",
            c.provider.as_ref().and_then(|p| {
                let limit = *l.provider_slots.get(p)?;
                let running = load.per_provider.get(p).copied().unwrap_or(0);
                (running >= limit).then(|| Deferral::ProviderBusy { provider: p.clone(), running, limit })
            }),
        );
        push(
            "model",
            c.model.as_ref().and_then(|m| {
                let limit = *l.model_slots.get(m)?;
                let running = load.per_model.get(m).copied().unwrap_or(0);
                (running >= limit).then(|| Deferral::ModelBusy { model: m.clone(), running, limit })
            }),
        );
        // Progress guarantee: CPU / GPU load caused by other programs never
        // blocks the first agent (nothing of ours runs), otherwise a busy host
        // would starve the queue forever. RAM and disk reserves always hold.
        let idle = load.running.is_empty();
        let projected = load.cpu + self.cpu_percent(c, resources.logical_cores);
        push(
            "cpu",
            (!idle && projected > l.max_cpu_percent)
                .then_some(Deferral::CpuBusy { projected_percent: projected, limit_percent: l.max_cpu_percent }),
        );
        let needed = self.ram(c).saturating_add(l.min_free_ram_mb);
        push(
            "memory",
            (load.free_ram.saturating_sub(self.ram(c)) < l.min_free_ram_mb)
                .then_some(Deferral::LowMemory { free_mb: load.free_ram, needed_mb: needed }),
        );
        push(
            "disk",
            resources
                .disk_free_mb
                .filter(|free| *free < l.min_free_disk_mb)
                .map(|free_mb| Deferral::LowDisk { free_mb, min_mb: l.min_free_disk_mb }),
        );
        push(
            "gpu",
            match (resources.gpu_usage_percent, l.max_gpu_percent) {
                (Some(usage), Some(limit)) if !idle && usage > limit => {
                    Some(Deferral::GpuBusy { usage_percent: usage, limit_percent: limit })
                }
                _ => None,
            },
        );
        push(
            "files",
            load.running
                .iter()
                .filter(|r| r.task_id != c.task_id)
                .find_map(|r| file_conflict(c, r).map(|path| Deferral::FileConflict { with: r.task_id, path })),
        );
        out
    }

    /// Runs every admission check for `candidate` against the active tasks
    /// (dry run, nothing admitted) — the data behind `scheduler.explain`.
    #[must_use]
    pub fn evaluate(
        &self,
        candidate: &SchedTask,
        active: &[SchedTask],
        resources: &ResourceSnapshot,
        now: Timestamp,
    ) -> Vec<AdmissionCheck> {
        let load = Load::new(active, resources);
        self.checks(candidate, &load, resources, now)
    }

    /// Decides which queued candidates may start now.
    #[must_use]
    pub fn next_runnable(
        &self,
        candidates: &[SchedTask],
        active: &[SchedTask],
        resources: &ResourceSnapshot,
        now: Timestamp,
    ) -> SchedulePlan {
        let mut plan = SchedulePlan::default();
        let mut load = Load::new(active, resources);
        let mut remaining: Vec<(usize, &SchedTask, TaskPriority)> = self
            .order(candidates, now)
            .into_iter()
            .enumerate()
            .map(|(i, c)| (i, c, effective_priority(c.priority, c.enqueued_at, now, self.limits.aging)))
            .collect();

        while !remaining.is_empty() {
            let pick = if self.limits.fair_share {
                // Highest effective priority; among equals the least-loaded
                // project (running + admitted); then queue order.
                remaining
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        b.2.cmp(&a.2)
                            .then_with(|| {
                                let la = load.per_project.get(&a.1.project_id).copied().unwrap_or(0);
                                let lb = load.per_project.get(&b.1.project_id).copied().unwrap_or(0);
                                la.cmp(&lb)
                            })
                            .then(a.0.cmp(&b.0))
                    })
                    .map_or(0, |(i, _)| i)
            } else {
                0
            };
            let (_, c, _) = remaining.remove(pick);
            let deferral = self.checks(c, &load, resources, now).into_iter().find_map(|ch| ch.deferral);
            match deferral {
                Some(d) => plan.deferred.push((c.task_id, d)),
                None => {
                    plan.start.push(c.task_id);
                    load.cpu += self.cpu_percent(c, resources.logical_cores);
                    load.free_ram = load.free_ram.saturating_sub(self.ram(c));
                    load.count(c);
                }
            }
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orkestr_domain::now;

    fn task(project: ProjectId, priority: TaskPriority, at: Timestamp) -> SchedTask {
        SchedTask { ram_mb: Some(100), cpu_weight: Some(0.1), ..SchedTask::new(TaskId::new(), project, priority, at) }
    }

    fn sched(f: impl FnOnce(&mut SchedulerLimits)) -> Scheduler {
        let mut l = SchedulerLimits { aging: AgingPolicy::disabled(), ..SchedulerLimits::default() };
        f(&mut l);
        Scheduler::new(l)
    }

    #[test]
    fn global_and_project_limits() {
        let t0 = now();
        let (p1, p2) = (ProjectId::new(), ProjectId::new());
        let cands: Vec<_> =
            (0..4).map(|_| task(p1, TaskPriority::Normal, t0)).chain([task(p2, TaskPriority::Low, t0)]).collect();
        let s = sched(|l| {
            l.max_parallel_global = 3;
            l.max_parallel_per_project = 2;
        });
        let plan = s.next_runnable(&cands, &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![cands[0].task_id, cands[1].task_id, cands[4].task_id], "backfills p2");
        assert_eq!(plan.deferred[0], (cands[2].task_id, Deferral::ProjectLimit { running: 2, limit: 2 }));
        // With a project override and an active task, the global cap binds.
        let s = sched(|l| {
            l.max_parallel_global = 2;
            l.project_overrides.insert(p1, 10);
        });
        let active = vec![task(p2, TaskPriority::Normal, t0)];
        let plan = s.next_runnable(&cands, &active, &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![cands[0].task_id]);
        assert!(plan.deferred.iter().all(|(_, d)| *d == Deferral::GlobalLimit { running: 2, limit: 2 }));
        assert_eq!(plan.deferred[0].1.to_string(), "waiting: global parallel limit 2/2");
    }

    #[test]
    fn priority_order_and_backfill() {
        let t0 = now();
        let p = ProjectId::new();
        let low = task(p, TaskPriority::Low, t0);
        let crit = task(p, TaskPriority::Critical, t0 + Duration::seconds(5));
        let s = sched(|l| l.max_parallel_per_project = 1);
        let plan = s.next_runnable(&[low.clone(), crit.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![crit.task_id]);
        assert_eq!(plan.deferred, vec![(low.task_id, Deferral::ProjectLimit { running: 1, limit: 1 })]);
    }

    #[test]
    fn provider_and_model_slots() {
        let t0 = now();
        let p = ProjectId::new();
        let mut a = task(p, TaskPriority::High, t0);
        a.provider = Some("anthropic".into());
        let mut b = task(p, TaskPriority::Normal, t0);
        b.provider = Some("anthropic".into());
        let mut c = task(p, TaskPriority::Low, t0);
        c.provider = Some("ollama".into());
        let s = sched(|l| {
            l.provider_slots.insert("anthropic".into(), 1);
        });
        let plan = s.next_runnable(&[a.clone(), b.clone(), c.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![a.task_id, c.task_id], "unlisted provider is unlimited");
        let d = Deferral::ProviderBusy { provider: "anthropic".into(), running: 1, limit: 1 };
        assert_eq!(d.to_string(), "waiting: provider concurrency 1/1 (anthropic)");
        assert_eq!(plan.deferred, vec![(b.task_id, d)]);

        // Model slots are independent of provider slots.
        let mut x = task(p, TaskPriority::High, t0);
        x.model = Some("prov/big".into());
        let mut y = task(p, TaskPriority::Normal, t0);
        y.model = Some("prov/big".into());
        let mut z = task(p, TaskPriority::Normal, t0);
        z.model = Some("prov/small".into());
        let s = sched(|l| {
            l.model_slots.insert("prov/big".into(), 1);
        });
        let plan = s.next_runnable(&[x.clone(), y.clone(), z.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![x.task_id, z.task_id]);
        assert_eq!(plan.deferred[0].1.code(), "modelBusy");
    }

    #[test]
    fn rate_limited_providers_back_off_until_the_deadline() {
        let t0 = now();
        let p = ProjectId::new();
        let mut a = task(p, TaskPriority::High, t0);
        a.provider = Some("openai".into());
        let mut b = task(p, TaskPriority::Normal, t0);
        b.provider = Some("mock".into());
        let until = t0 + Duration::seconds(30);
        let s = sched(|l| {
            l.rate_limited.insert("openai".into(), until);
        });
        let plan = s.next_runnable(&[a.clone(), b.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![b.task_id]);
        assert_eq!(plan.deferred, vec![(a.task_id, Deferral::RateLimited { provider: "openai".into(), until })]);
        // After the deadline the provider is admitted again.
        let plan = s.next_runnable(&[a.clone()], &[], &ResourceSnapshot::unconstrained(), until + Duration::seconds(1));
        assert_eq!(plan.start, vec![a.task_id]);
    }

    #[test]
    fn team_caps() {
        let t0 = now();
        let p = ProjectId::new();
        let mk = |team: &str| {
            let mut t = task(p, TaskPriority::Normal, t0);
            t.team = Some(team.into());
            t
        };
        let (a, b, c) = (mk("web"), mk("web"), mk("mobile"));
        let s = sched(|l| {
            l.team_caps.insert("web".into(), 1);
        });
        let plan = s.next_runnable(&[a.clone(), b.clone(), c.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![a.task_id, c.task_id]);
        assert_eq!(plan.deferred, vec![(b.task_id, Deferral::TeamLimit { team: "web".into(), running: 1, limit: 1 })]);
        assert_eq!(team_of(&["x".into(), "team: web ".into()]), Some("web".into()));
        assert_eq!(team_of(&["team:".into()]), None);
    }

    #[test]
    fn cpu_ram_disk_gpu_thresholds() {
        let t0 = now();
        let p = ProjectId::new();
        let mut heavy = task(p, TaskPriority::High, t0);
        heavy.cpu_weight = Some(4.0); // 50 % of 8 cores
        let light = task(p, TaskPriority::Normal, t0);
        let res = ResourceSnapshot {
            cpu_usage_percent: 40.0,
            logical_cores: 8,
            ram_total_mb: 16_000,
            ram_available_mb: 8_000,
            disk_free_mb: Some(50_000),
            gpu_usage_percent: None,
        };
        let s = sched(|_| {});
        // One agent already runs (cheap): the heavy one would exceed 85 %.
        let busy = task(p, TaskPriority::Normal, t0);
        let mut tiny = busy.clone();
        tiny.cpu_weight = Some(0.0);
        let plan = s.next_runnable(&[heavy.clone(), light.clone()], std::slice::from_ref(&tiny), &res, t0);
        assert_eq!(plan.start, vec![light.task_id]);
        assert_eq!(plan.deferred[0].1.code(), "cpuBusy");
        // Nothing of ours runs: host load never starves the queue.
        let hot = ResourceSnapshot { cpu_usage_percent: 99.0, ..res };
        let plan = s.next_runnable(&[heavy.clone(), light.clone()], &[], &hot, t0);
        assert_eq!(plan.start, vec![heavy.task_id], "the first agent always starts");
        assert_eq!(plan.deferred[0].1.code(), "cpuBusy", "but not a second one");

        let mut big = task(p, TaskPriority::High, t0);
        big.ram_mb = Some(6_500);
        let plan = s.next_runnable(&[big.clone(), light.clone()], &[], &res, t0);
        assert_eq!(plan.deferred, vec![(big.task_id, Deferral::LowMemory { free_mb: 8_000, needed_mb: 8_548 })]);
        assert_eq!(plan.start, vec![light.task_id]);

        // Admitted tasks consume the budget for the ones after them.
        let mut a = task(p, TaskPriority::High, t0);
        a.ram_mb = Some(3_000);
        let mut b = task(p, TaskPriority::Normal, t0);
        b.ram_mb = Some(3_000);
        let plan = s.next_runnable(&[a.clone(), b.clone()], &[], &res, t0);
        assert_eq!(plan.start, vec![a.task_id]);
        assert_eq!(plan.deferred[0].1.code(), "lowMemory");

        // Disk reserve and GPU threshold.
        let low_disk = ResourceSnapshot { disk_free_mb: Some(100), ..res };
        let plan = s.next_runnable(std::slice::from_ref(&light), &[], &low_disk, t0);
        assert_eq!(plan.deferred, vec![(light.task_id, Deferral::LowDisk { free_mb: 100, min_mb: 1024 })]);
        let busy_gpu = ResourceSnapshot { gpu_usage_percent: Some(99.0), ..res };
        assert_eq!(s.next_runnable(std::slice::from_ref(&light), &[], &busy_gpu, t0).start.len(), 1, "no GPU limit configured");
        let s = sched(|l| l.max_gpu_percent = Some(90.0));
        assert_eq!(s.next_runnable(std::slice::from_ref(&light), &[], &busy_gpu, t0).start.len(), 1, "progress guarantee");
        assert_eq!(
            s.next_runnable(std::slice::from_ref(&light), std::slice::from_ref(&tiny), &busy_gpu, t0).deferred[0].1.code(),
            "gpuBusy"
        );
    }

    #[test]
    fn file_overlap_conflicts() {
        let t0 = now();
        let p = ProjectId::new();
        let mut running = task(p, TaskPriority::Normal, t0);
        running.files = vec!["src/api".into()];
        let mut same = task(p, TaskPriority::High, t0);
        same.files = vec!["SRC\\api\\users.rs".into()];
        let mut other = task(p, TaskPriority::Normal, t0);
        other.files = vec!["src/apiary.rs".into(), "docs/x.md".into()];
        let s = sched(|_| {});
        let plan =
            s.next_runnable(&[same.clone(), other.clone()], &[running.clone()], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![other.task_id], "src/apiary.rs does not overlap src/api");
        assert_eq!(
            plan.deferred,
            vec![(same.task_id, Deferral::FileConflict { with: running.task_id, path: "SRC\\api\\users.rs".into() })]
        );

        // Separate worktrees may overlap.
        running.worktree = Some("/wt/a".into());
        same.worktree = Some("/wt/b".into());
        let plan = s.next_runnable(&[same.clone()], &[running.clone()], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![same.task_id]);
        // Same worktree (or only one has a worktree) conflicts.
        same.worktree = Some("/wt/a/".into());
        let plan = s.next_runnable(&[same.clone()], &[running.clone()], &ResourceSnapshot::unconstrained(), t0);
        assert!(plan.start.is_empty());

        // Two candidates conflicting with each other: only the first starts.
        let mut x = task(p, TaskPriority::High, t0);
        x.files = vec!["Cargo.toml".into()];
        let mut y = task(p, TaskPriority::Normal, t0);
        y.files = vec!["./cargo.toml".into()];
        let plan = s.next_runnable(&[y.clone(), x.clone()], &[], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![x.task_id]);
        // Different projects never conflict.
        let mut z = y.clone();
        z.project_id = ProjectId::new();
        assert!(file_conflict(&x, &z).is_none());
    }

    #[test]
    fn fair_share_interleaves_projects_under_the_global_cap() {
        let t0 = now();
        let (busy, quiet) = (ProjectId::new(), ProjectId::new());
        // The busy project queued 6 tasks first; the quiet one 2 later.
        let mut cands: Vec<SchedTask> = (0..6).map(|i| task(busy, TaskPriority::Normal, t0 + Duration::milliseconds(i))).collect();
        cands.extend((0..2).map(|i| task(quiet, TaskPriority::Normal, t0 + Duration::seconds(10 + i))));
        let s = sched(|l| {
            l.max_parallel_global = 4;
            l.max_parallel_per_project = 10;
        });
        let plan = s.next_runnable(&cands, &[], &ResourceSnapshot::unconstrained(), t0 + Duration::seconds(20));
        let quiet_started = plan.start.iter().filter(|id| cands[6..].iter().any(|c| c.task_id == **id)).count();
        assert_eq!(plan.start.len(), 4);
        assert_eq!(quiet_started, 2, "the quiet project gets its fair share: {:?}", plan.start);
        // Without fair share pure FIFO starves the quiet project.
        let s = sched(|l| {
            l.max_parallel_global = 4;
            l.max_parallel_per_project = 10;
            l.fair_share = false;
        });
        let plan = s.next_runnable(&cands, &[], &ResourceSnapshot::unconstrained(), t0 + Duration::seconds(20));
        assert!(plan.start.iter().all(|id| cands[..6].iter().any(|c| c.task_id == *id)));
        // Active load counts too: a project already running more yields.
        let active: Vec<SchedTask> = (0..3).map(|_| task(busy, TaskPriority::Normal, t0)).collect();
        let s = sched(|l| {
            l.max_parallel_global = 4;
            l.max_parallel_per_project = 10;
        });
        let plan = s.next_runnable(&cands, &active, &ResourceSnapshot::unconstrained(), t0 + Duration::seconds(20));
        assert_eq!(plan.start, vec![cands[6].task_id]);
        // Priority still beats fairness.
        let mut urgent = task(busy, TaskPriority::Critical, t0);
        urgent.files.clear();
        let mut c2 = cands.clone();
        c2.push(urgent.clone());
        let plan = s.next_runnable(&c2, &active, &ResourceSnapshot::unconstrained(), t0 + Duration::seconds(20));
        assert_eq!(plan.start, vec![urgent.task_id]);
    }

    #[test]
    fn evaluate_reports_every_check() {
        let t0 = now();
        let p = ProjectId::new();
        let mut c = task(p, TaskPriority::Normal, t0);
        c.provider = Some("x".into());
        c.files = vec!["a.rs".into()];
        let mut r = task(p, TaskPriority::Normal, t0);
        r.provider = Some("x".into());
        r.files = vec!["a.rs".into()];
        let s = sched(|l| {
            l.provider_slots.insert("x".into(), 1);
            l.max_parallel_per_project = 1;
        });
        let checks = s.evaluate(&c, &[r.clone()], &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(checks.len(), 11);
        let failing: Vec<&str> = checks.iter().filter(|ch| ch.deferral.is_some()).map(|ch| ch.name).collect();
        assert_eq!(failing, vec!["project", "provider", "files"]);
        // A task never conflicts with itself (it may be listed as active).
        let checks = s.evaluate(&c, &[c.clone()], &ResourceSnapshot::unconstrained(), t0);
        assert!(checks.iter().find(|ch| ch.name == "files").is_some_and(|ch| ch.deferral.is_none()));
    }

    #[test]
    fn overlap_rules() {
        assert!(paths_overlap("src", "src/a.rs"));
        assert!(paths_overlap("src/a.rs", "./src/a.rs"));
        assert!(paths_overlap("src/", "src"));
        assert!(!paths_overlap("src/a", "src/ab"));
        assert!(!paths_overlap("", "src"));
        assert_eq!(first_overlap(&["a/b.rs".into()], &["x".into(), "a".into()]), Some("a/b.rs".into()));
    }
}
