//! Resource-aware scheduler (§11, §38, task-state-machine.md §7.2–7.3).
//!
//! Pure logic: given the queued `Ready` candidates, the currently active
//! tasks, the limits and a resource snapshot, [`Scheduler::next_runnable`]
//! returns which tasks may start now (in queue order, with **backfilling**:
//! an inadmissible head does not block admissible tasks behind it) and why
//! the others were deferred.
//!
//! Admission checks, in order:
//! 1. global parallel cap; 2. per-project parallel cap; 3. provider
//!    concurrency slots; 4. CPU threshold (current usage + projected load of
//!    admitted tasks); 5. free-RAM reserve; 6. **file-overlap conflict
//!    avoidance** — two tasks of the same project whose file sets overlap
//!    never run concurrently unless both have their own, different worktrees.

use std::collections::BTreeMap;

use orkestr_domain::{ProjectId, TaskId, TaskPriority, Timestamp};

use crate::queue::{AgingPolicy, PriorityQueue, QueueEntry};

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
}

impl ResourceSnapshot {
    /// A snapshot that never limits (tests, unknown resources).
    #[must_use]
    pub fn unconstrained() -> Self {
        Self { cpu_usage_percent: 0.0, logical_cores: 64, ram_total_mb: u64::MAX / 4, ram_available_mb: u64::MAX / 4 }
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
    /// Concurrency slots per provider id (absent provider = unlimited).
    pub provider_slots: BTreeMap<String, usize>,
    /// Do not start new work above this projected CPU usage (percent).
    pub max_cpu_percent: f32,
    /// RAM that must stay free after admitting a task (MiB).
    pub min_free_ram_mb: u64,
    /// RAM assumed for a task without an estimate (MiB).
    pub default_task_ram_mb: u64,
    /// CPU weight (cores) assumed for a task without an estimate.
    pub default_cpu_weight: f32,
    /// Aging for queue order.
    pub aging: AgingPolicy,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_parallel_global: 8,
            max_parallel_per_project: 4,
            project_overrides: BTreeMap::new(),
            provider_slots: BTreeMap::new(),
            max_cpu_percent: 85.0,
            min_free_ram_mb: 2048,
            default_task_ram_mb: 512,
            default_cpu_weight: 0.5,
            aging: AgingPolicy::default(),
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
    /// Files it touches (relative to the project root).
    pub files: Vec<String>,
    /// Its own worktree path, if any.
    pub worktree: Option<String>,
    /// RAM estimate (MiB).
    pub ram_mb: Option<u64>,
    /// CPU estimate (cores).
    pub cpu_weight: Option<f32>,
}

/// Why a candidate was not started.
#[derive(Debug, Clone, PartialEq)]
pub enum Deferral {
    /// Global parallel cap reached.
    GlobalLimit,
    /// Project parallel cap reached.
    ProjectLimit,
    /// No free provider slot.
    ProviderBusy {
        /// Provider.
        provider: String,
    },
    /// CPU above threshold.
    CpuBusy,
    /// Not enough free RAM.
    LowMemory,
    /// Overlaps files with an active/admitted task in the same worktree.
    FileConflict {
        /// The conflicting task.
        with: TaskId,
        /// One overlapping path.
        path: String,
    },
}

/// Output of [`Scheduler::next_runnable`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedulePlan {
    /// Tasks to start now, in order.
    pub start: Vec<TaskId>,
    /// Deferred candidates with reasons, in queue order.
    pub deferred: Vec<(TaskId, Deferral)>,
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
    a.files.iter().find(|fa| b.files.iter().any(|fb| paths_overlap(fa, fb))).cloned()
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

    /// Orders candidates with the priority queue (aging at `now`).
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

    /// Decides which queued candidates may start now.
    #[must_use]
    pub fn next_runnable(
        &self,
        candidates: &[SchedTask],
        active: &[SchedTask],
        resources: &ResourceSnapshot,
        now: Timestamp,
    ) -> SchedulePlan {
        let l = &self.limits;
        let mut plan = SchedulePlan::default();
        let mut running: Vec<&SchedTask> = active.iter().collect();
        let mut per_project: BTreeMap<ProjectId, usize> = BTreeMap::new();
        let mut per_provider: BTreeMap<&str, usize> = BTreeMap::new();
        for t in active {
            *per_project.entry(t.project_id).or_default() += 1;
            if let Some(p) = &t.provider {
                *per_provider.entry(p.as_str()).or_default() += 1;
            }
        }
        let mut cpu = resources.cpu_usage_percent;
        let mut free_ram = resources.ram_available_mb;

        for c in self.order(candidates, now) {
            let deferral = if running.len() >= l.max_parallel_global {
                Some(Deferral::GlobalLimit)
            } else if per_project.get(&c.project_id).copied().unwrap_or(0)
                >= l.project_overrides.get(&c.project_id).copied().unwrap_or(l.max_parallel_per_project)
            {
                Some(Deferral::ProjectLimit)
            } else if let Some(p) = c.provider.as_deref().filter(|p| {
                l.provider_slots.get(*p).is_some_and(|slots| per_provider.get(*p).copied().unwrap_or(0) >= *slots)
            }) {
                Some(Deferral::ProviderBusy { provider: p.to_owned() })
            } else if cpu + self.cpu_percent(c, resources.logical_cores) > l.max_cpu_percent {
                Some(Deferral::CpuBusy)
            } else if free_ram.saturating_sub(self.ram(c)) < l.min_free_ram_mb {
                Some(Deferral::LowMemory)
            } else {
                running
                    .iter()
                    .find_map(|r| file_conflict(c, r).map(|path| Deferral::FileConflict { with: r.task_id, path }))
            };
            match deferral {
                Some(d) => plan.deferred.push((c.task_id, d)),
                None => {
                    plan.start.push(c.task_id);
                    *per_project.entry(c.project_id).or_default() += 1;
                    if let Some(p) = &c.provider {
                        *per_provider.entry(p.as_str()).or_default() += 1;
                    }
                    cpu += self.cpu_percent(c, resources.logical_cores);
                    free_ram = free_ram.saturating_sub(self.ram(c));
                    running.push(c);
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
        SchedTask {
            task_id: TaskId::new(),
            project_id: project,
            priority,
            enqueued_at: at,
            provider: None,
            files: Vec::new(),
            worktree: None,
            ram_mb: Some(100),
            cpu_weight: Some(0.1),
        }
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
        assert_eq!(plan.deferred[0], (cands[2].task_id, Deferral::ProjectLimit));
        // With a project override and an active task, the global cap binds.
        let s = sched(|l| {
            l.max_parallel_global = 2;
            l.project_overrides.insert(p1, 10);
        });
        let active = vec![task(p2, TaskPriority::Normal, t0)];
        let plan = s.next_runnable(&cands, &active, &ResourceSnapshot::unconstrained(), t0);
        assert_eq!(plan.start, vec![cands[0].task_id]);
        assert!(plan.deferred.iter().all(|(_, d)| *d == Deferral::GlobalLimit));
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
        assert_eq!(plan.deferred, vec![(low.task_id, Deferral::ProjectLimit)]);
    }

    #[test]
    fn provider_slots() {
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
        assert_eq!(plan.deferred, vec![(b.task_id, Deferral::ProviderBusy { provider: "anthropic".into() })]);
    }

    #[test]
    fn cpu_and_ram_thresholds() {
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
        };
        let s = sched(|_| {});
        let plan = s.next_runnable(&[heavy.clone(), light.clone()], &[], &res, t0);
        assert_eq!(plan.start, vec![light.task_id]);
        assert_eq!(plan.deferred, vec![(heavy.task_id, Deferral::CpuBusy)]);

        let mut big = task(p, TaskPriority::High, t0);
        big.ram_mb = Some(6_500);
        let plan = s.next_runnable(&[big.clone(), light.clone()], &[], &res, t0);
        assert_eq!(plan.deferred, vec![(big.task_id, Deferral::LowMemory)]);
        assert_eq!(plan.start, vec![light.task_id]);

        // Admitted tasks consume the budget for the ones after them.
        let mut a = task(p, TaskPriority::High, t0);
        a.ram_mb = Some(3_000);
        let mut b = task(p, TaskPriority::Normal, t0);
        b.ram_mb = Some(3_000);
        let plan = s.next_runnable(&[a.clone(), b.clone()], &[], &res, t0);
        assert_eq!(plan.start, vec![a.task_id]);
        assert_eq!(plan.deferred, vec![(b.task_id, Deferral::LowMemory)]);
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
    fn overlap_rules() {
        assert!(paths_overlap("src", "src/a.rs"));
        assert!(paths_overlap("src/a.rs", "./src/a.rs"));
        assert!(paths_overlap("src/", "src"));
        assert!(!paths_overlap("src/a", "src/ab"));
        assert!(!paths_overlap("", "src"));
    }
}
