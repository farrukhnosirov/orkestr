//! `dashboard.*` handlers — Mission Control and Monitoring read models
//! (§27 local part, §32, §68.22, §68.45 lite).
//!
//! `dashboard.snapshot` aggregates existing engines; a failing source is
//! logged and degrades to empty data instead of failing the whole screen.
//! `dashboard.health` measures storage on disk and the process tree the core
//! spawned (terminals, CLI agents) on the blocking pool.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use orkestr_domain::{
    AgentInstance, AgentState, Approval, ApprovalStatus, Project, ProjectId, Task, TaskStatus,
};
use orkestr_protocol::methods::agents::ListAgentsParams;
use orkestr_protocol::methods::dashboard::{
    self as m, CoreHealthResult, DashboardSnapshot, DashboardSnapshotParams, DashboardTotals, OrkestrProcess,
    OrkestrProcessKind, ProjectActivity, ProjectOverview, ProviderHealthRow, SpendOverview, StorageHealth,
};
use orkestr_protocol::methods::manager::ManagerStatusParams;
use orkestr_protocol::methods::nodes::ProcessUsage;
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_protocol::RpcError;
use orkestr_security::ApprovalFilter;
use parking_lot::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::{RpcContext, RpcRouter};

/// Upper bound of files visited when sizing the object store.
const MAX_OBJECT_SCAN: u64 = 200_000;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::Snapshot, _, _>(snapshot).method::<m::Health, _, _>(|ctx: RpcContext, _| health(ctx));
}

fn ms_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Logs a failed source and substitutes its default.
fn or_default<T: Default, E: std::fmt::Display>(source: &str, r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| {
        tracing::warn!(source, error = %e, "dashboard source unavailable");
        T::default()
    })
}

async fn snapshot(ctx: RpcContext, p: DashboardSnapshotParams) -> Result<DashboardSnapshot, RpcError> {
    let start = Instant::now();
    let s = &ctx.services;
    let projects = s.projects.list(p.include_archived.unwrap_or(false)).await.map_err(crate::error::storage_to_rpc)?;
    let agents = or_default("agents", s.agents.list(ListAgentsParams::default()).await);
    let tasks = or_default("tasks", s.tasks.list(ListTasksParams::default()).await);
    let approvals = or_default(
        "approvals",
        s.security
            .approvals
            .list(&ApprovalFilter { status: Some(ApprovalStatus::Pending), project_id: None, limit: Some(1000) })
            .await,
    );
    let live_runs: Vec<ProjectId> = match s.manager.status(ManagerStatusParams::default()).await {
        Ok(st) => {
            let live: HashSet<_> = st.live_run_ids.iter().copied().collect();
            st.runs.iter().filter(|r| live.contains(&r.id)).map(|r| r.project_id).collect()
        }
        Err(e) => {
            tracing::warn!(error = %e, "dashboard source unavailable: manager");
            Vec::new()
        }
    };

    let mut spend_by_project = HashMap::new();
    for proj in &projects {
        if let Ok(sum) = s.cost.summary(Some(proj.id)).await {
            spend_by_project.insert(proj.id, (sum.today.cost_usd, sum.month.cost_usd));
        }
    }
    let spend = match s.cost.summary(None).await {
        Ok(sum) => SpendOverview {
            today_usd: sum.today.cost_usd,
            today_estimated_usd: sum.today.estimated_cost_usd,
            month_usd: sum.month.cost_usd,
            month_estimated_usd: sum.month.estimated_cost_usd,
            today_tokens: sum.today.input_tokens.saturating_add(sum.today.output_tokens),
            budget_used: sum.budget_used,
        },
        Err(e) => {
            tracing::warn!(error = %e, "dashboard source unavailable: cost");
            SpendOverview::default()
        }
    };

    let (rows, totals) = aggregate(&AggregateInput {
        projects: &projects,
        agents: &agents,
        tasks: &tasks,
        approvals: &approvals,
        live_runs: &live_runs,
        spend: &spend_by_project,
    });

    Ok(DashboardSnapshot {
        generated_at: orkestr_domain::now(),
        uptime_ms: s.uptime_ms(),
        core_version: s.version.clone(),
        projects: rows,
        totals,
        spend,
        providers: s.providers.list().iter().map(provider_row).collect(),
        emergency_stopped: s.security.emergency.flags().all_stopped,
        took_ms: ms_since(start),
    })
}

/// Inputs of [`aggregate`] (borrowed engine data).
pub(crate) struct AggregateInput<'a> {
    pub projects: &'a [Project],
    pub agents: &'a [AgentInstance],
    pub tasks: &'a [Task],
    pub approvals: &'a [Approval],
    /// Project of every live Manager run.
    pub live_runs: &'a [ProjectId],
    /// `(today, month)` spend per project.
    pub spend: &'a HashMap<ProjectId, (f64, f64)>,
}

/// Whether an agent's run is still going (mirrors the UI's `isActive`).
pub(crate) fn agent_active(state: AgentState) -> bool {
    !matches!(state, AgentState::Completed | AgentState::Error | AgentState::Stopped)
}

fn task_running(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Testing)
}

fn task_failed(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Failed | TaskStatus::Interrupted)
}

/// Activity rule: attention beats active beats idle.
pub(crate) fn activity_of(row: &ProjectOverview) -> ProjectActivity {
    if row.failed_tasks > 0 || row.pending_approvals > 0 {
        ProjectActivity::Attention
    } else if row.active_agents > 0 || row.running_tasks > 0 || row.live_runs > 0 {
        ProjectActivity::Active
    } else {
        ProjectActivity::Idle
    }
}

fn activity_rank(a: ProjectActivity) -> u8 {
    match a {
        ProjectActivity::Attention => 0,
        ProjectActivity::Active => 1,
        ProjectActivity::Idle => 2,
    }
}

/// Folds engine data into per-project rows (most active first) and totals.
pub(crate) fn aggregate(input: &AggregateInput<'_>) -> (Vec<ProjectOverview>, DashboardTotals) {
    let mut rows: Vec<ProjectOverview> = input
        .projects
        .iter()
        .map(|p| {
            let (today, month) = input.spend.get(&p.id).copied().unwrap_or((0.0, 0.0));
            ProjectOverview {
                project_id: p.id,
                name: p.name.clone(),
                color: p.color.clone(),
                activity: ProjectActivity::Idle,
                active_agents: 0,
                total_agents: 0,
                running_tasks: 0,
                open_tasks: 0,
                failed_tasks: 0,
                completed_tasks: 0,
                pending_approvals: 0,
                live_runs: 0,
                spend_today_usd: today,
                spend_month_usd: month,
            }
        })
        .collect();
    let index: HashMap<ProjectId, usize> = rows.iter().enumerate().map(|(i, r)| (r.project_id, i)).collect();
    let mut totals = DashboardTotals { projects: count(rows.len()), ..DashboardTotals::default() };

    for a in input.agents {
        let active = agent_active(a.state);
        totals.total_agents += 1;
        if active {
            totals.active_agents += 1;
        }
        if let Some(row) = index.get(&a.project_id).and_then(|&i| rows.get_mut(i)) {
            row.total_agents += 1;
            if active {
                row.active_agents += 1;
            }
        }
    }
    for t in input.tasks {
        let running = task_running(t.status);
        let failed = task_failed(t.status);
        if running {
            totals.running_tasks += 1;
        }
        if failed {
            totals.failed_tasks += 1;
        }
        if let Some(row) = index.get(&t.project_id).and_then(|&i| rows.get_mut(i)) {
            if running {
                row.running_tasks += 1;
            }
            if failed {
                row.failed_tasks += 1;
            }
            if t.status == TaskStatus::Completed {
                row.completed_tasks += 1;
            }
            if !t.status.is_terminal() {
                row.open_tasks += 1;
            }
        }
    }
    for ap in input.approvals.iter().filter(|a| a.status == ApprovalStatus::Pending) {
        totals.pending_approvals += 1;
        if let Some(row) = ap.project_id.and_then(|pid| index.get(&pid)).and_then(|&i| rows.get_mut(i)) {
            row.pending_approvals += 1;
        }
    }
    for pid in input.live_runs {
        totals.live_runs += 1;
        if let Some(row) = index.get(pid).and_then(|&i| rows.get_mut(i)) {
            row.live_runs += 1;
        }
    }
    for row in &mut rows {
        row.activity = activity_of(row);
    }
    rows.sort_by(|a, b| {
        activity_rank(a.activity)
            .cmp(&activity_rank(b.activity))
            .then(b.active_agents.cmp(&a.active_agents))
            .then(b.spend_month_usd.total_cmp(&a.spend_month_usd))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    (rows, totals)
}

fn wire_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn provider_row(v: &orkestr_providers::ProviderView) -> ProviderHealthRow {
    ProviderHealthRow {
        provider_id: v.provider.id,
        name: v.provider.name.clone(),
        kind: wire_name(&v.provider.kind),
        enabled: v.provider.enabled,
        is_local: v.provider.is_local,
        health: v.status.health,
        auth_state: v.status.auth_state,
        latency_ms: v.status.latency_ms,
        avg_latency_ms: v.status.avg_latency_ms,
        rate_limited_until: v.status.rate_limited_until,
        quota_exhausted_until: v.status.quota_exhausted_until,
        circuit_open: v.status.circuit_open,
    }
}

// ── health ──────────────────────────────────────────────────────────────

async fn health(ctx: RpcContext) -> Result<CoreHealthResult, RpcError> {
    let start = Instant::now();
    let s = ctx.services.clone();
    let db_path = s.db.path().map(Path::to_path_buf);
    let objects_root = s.objects.root().to_path_buf();
    let roots: Vec<ProcessRoot> = s
        .terminals
        .list(None)
        .into_iter()
        .map(|t| ProcessRoot {
            kind: if t.owner.is_agent() { OrkestrProcessKind::AgentTerminal } else { OrkestrProcessKind::Terminal },
            id: t.id.to_string(),
            label: t.title.clone(),
            project_id: t.project_id,
            agent_id: t.owner.agent_id(),
            pid: t.pid,
            running: matches!(t.status, orkestr_terminal::TerminalStatus::Running),
        })
        .collect();

    let (storage, (core_process, processes)) = tokio::task::spawn_blocking(move || {
        let storage = storage_health(db_path.as_deref(), &objects_root);
        let procs = process_tree(std::process::id(), &roots);
        (storage, procs)
    })
    .await
    .map_err(|e| RpcError::internal(format!("health probe failed: {e}")))?;

    Ok(CoreHealthResult {
        core_version: s.version.clone(),
        pid: std::process::id(),
        started_at: s.started_at,
        uptime_ms: s.uptime_ms(),
        last_event_seq: s.events.last_seq(),
        rpc_methods: count(orkestr_protocol::methods::registry().len()),
        storage,
        core_process,
        processes,
        took_ms: ms_since(start),
    })
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Sizes of the SQLite files and the object store (bounded walk).
pub(crate) fn storage_health(db: Option<&Path>, objects_root: &Path) -> StorageHealth {
    let mut out = StorageHealth::default();
    if let Some(db) = db {
        out.db_bytes = file_len(db);
        let mut wal = db.as_os_str().to_owned();
        wal.push("-wal");
        out.db_wal_bytes = file_len(Path::new(&wal));
    }
    let mut stack = vec![objects_root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                out.object_count += 1;
                out.object_store_bytes = out.object_store_bytes.saturating_add(meta.len());
                if out.object_count >= MAX_OBJECT_SCAN {
                    out.object_scan_truncated = true;
                    break 'walk;
                }
            }
        }
    }
    out
}

/// A terminal session whose process subtree is measured.
#[derive(Debug, Clone)]
pub(crate) struct ProcessRoot {
    pub kind: OrkestrProcessKind,
    pub id: String,
    pub label: String,
    pub project_id: Option<ProjectId>,
    pub agent_id: Option<orkestr_domain::AgentId>,
    pub pid: Option<u32>,
    pub running: bool,
}

/// Minimal process view used by the pure tree folding (testable without sysinfo).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// CPU in "percent of one core" (sysinfo convention).
    pub cpu_core_percent: f32,
    pub memory_bytes: u64,
}

fn system() -> &'static Mutex<System> {
    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();
    SYSTEM.get_or_init(|| Mutex::new(System::new()))
}

fn process_tree(core_pid: u32, roots: &[ProcessRoot]) -> (Option<ProcessUsage>, Vec<OrkestrProcess>) {
    let procs: Vec<ProcInfo> = {
        let mut sys = system().lock();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        sys.processes()
            .iter()
            .map(|(pid, p)| ProcInfo {
                pid: pid.as_u32(),
                parent: p.parent().map(Pid::as_u32),
                name: p.name().to_string_lossy().into_owned(),
                cpu_core_percent: p.cpu_usage(),
                memory_bytes: p.memory(),
            })
            .collect()
    };
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    fold_tree(core_pid, roots, &procs, cores)
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn machine_percent(core_percent: f32, cores: f32) -> f32 {
    let raw = if core_percent.is_finite() && core_percent > 0.0 { core_percent } else { 0.0 };
    round1((raw / cores.max(1.0)).clamp(0.0, 100.0))
}

/// Folds a process table into: the core's own usage, one row per terminal
/// (its subtree), and one row per other direct child of the core (subtree).
pub(crate) fn fold_tree(
    core_pid: u32,
    roots: &[ProcessRoot],
    procs: &[ProcInfo],
    cores: f32,
) -> (Option<ProcessUsage>, Vec<OrkestrProcess>) {
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        if let Some(parent) = p.parent.filter(|&pp| pp != p.pid) {
            children.entry(parent).or_default().push(p.pid);
        }
    }
    let subtree = |root: u32| -> (f32, u64, u32) {
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        let (mut cpu, mut mem, mut n) = (0.0_f32, 0_u64, 0_u32);
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(p) = by_pid.get(&pid) {
                cpu += if p.cpu_core_percent.is_finite() { p.cpu_core_percent.max(0.0) } else { 0.0 };
                mem = mem.saturating_add(p.memory_bytes);
                n += 1;
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids.iter().copied());
            }
        }
        (cpu, mem, n)
    };

    let core = by_pid.get(&core_pid).map(|p| ProcessUsage {
        pid: p.pid,
        name: p.name.clone(),
        cpu_percent: machine_percent(p.cpu_core_percent, cores),
        cpu_cores: p.cpu_core_percent.max(0.0).round() / 100.0,
        memory_bytes: p.memory_bytes,
    });

    let mut out = Vec::new();
    let mut claimed: HashSet<u32> = HashSet::new();
    for r in roots {
        let alive = r.pid.filter(|pid| by_pid.contains_key(pid));
        let (cpu, mem, n) = alive.map_or((0.0, 0, 0), subtree);
        if let Some(pid) = alive {
            claimed.insert(pid);
        }
        out.push(OrkestrProcess {
            kind: r.kind,
            id: r.id.clone(),
            label: r.label.clone(),
            project_id: r.project_id,
            agent_id: r.agent_id,
            pid: r.pid,
            running: r.running,
            cpu_percent: alive.map(|_| machine_percent(cpu, cores)),
            memory_bytes: alive.map(|_| mem),
            process_count: n,
        });
    }
    // Other direct children of the core (CLI agents, git, tools, PTY hosts not
    // matched above). Terminal shells may hang under a conpty host child: skip
    // any child whose subtree contains a claimed terminal pid.
    let mut direct: Vec<u32> = children.get(&core_pid).cloned().unwrap_or_default();
    direct.sort_unstable();
    for pid in direct {
        if claimed.contains(&pid) {
            continue;
        }
        let contains_claimed = {
            let mut stack = vec![pid];
            let mut seen = HashSet::new();
            let mut found = false;
            while let Some(p) = stack.pop() {
                if !seen.insert(p) {
                    continue;
                }
                if p != pid && claimed.contains(&p) {
                    found = true;
                    break;
                }
                if let Some(k) = children.get(&p) {
                    stack.extend(k.iter().copied());
                }
            }
            found
        };
        if contains_claimed {
            continue;
        }
        let Some(info) = by_pid.get(&pid) else { continue };
        let (cpu, mem, n) = subtree(pid);
        out.push(OrkestrProcess {
            kind: OrkestrProcessKind::Child,
            id: format!("pid:{pid}"),
            label: info.name.clone(),
            project_id: None,
            agent_id: None,
            pid: Some(pid),
            running: true,
            cpu_percent: Some(machine_percent(cpu, cores)),
            memory_bytes: Some(mem),
            process_count: n,
        });
    }
    (core, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{AgentId, ApprovalId};

    fn project(name: &str) -> Project {
        let mut p = Project::new(name, orkestr_domain::now());
        p.color = Some("#22d3ee".into());
        p
    }

    fn task(project_id: ProjectId, status: TaskStatus) -> Task {
        let mut t = Task::new(project_id, "t", orkestr_domain::now());
        t.status = status;
        t
    }

    fn agent(project_id: ProjectId, state: AgentState) -> AgentInstance {
        AgentInstance {
            id: AgentId::new(),
            template_id: None,
            project_id,
            task_id: None,
            team_id: None,
            parent_agent_id: None,
            name: "Backend #1".into(),
            role: orkestr_domain::AgentRole::Backend,
            state,
            model: None,
            worktree: None,
            usage: orkestr_domain::UsageSummary::default(),
            turns_used: 0,
            last_error: None,
            created_at: orkestr_domain::now(),
            started_at: None,
            stopped_at: None,
            last_activity_at: None,
            template_version: None,
            worktree_policy: orkestr_domain::WorktreePolicy::SharedWorktree,
            max_turns: 10,
            budget: None,
            execution_id: None,
            execution_style: None,
            cli_session: None,
            activity: None,
        }
    }

    fn approval(project_id: Option<ProjectId>) -> Approval {
        Approval {
            id: ApprovalId::new(),
            project_id,
            task_id: None,
            agent_id: Some(AgentId::new()),
            tool_call_id: None,
            capability: orkestr_domain::Capability::Terminal,
            action_summary: "rm".into(),
            risk: orkestr_domain::RiskLevel::High,
            reason: String::new(),
            status: ApprovalStatus::Pending,
            requested_at: orkestr_domain::now(),
            expires_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            command_preview: None,
            danger: None,
        }
    }

    #[test]
    fn aggregates_rows_totals_and_activity() {
        let busy = project("Busy");
        let calm = project("Calm");
        let broken = project("Broken");
        let agents = vec![
            agent(busy.id, AgentState::Coding),
            agent(busy.id, AgentState::Completed),
            agent(calm.id, AgentState::Stopped),
        ];
        let tasks = vec![
            task(busy.id, TaskStatus::Running),
            task(busy.id, TaskStatus::Ready),
            task(busy.id, TaskStatus::Completed),
            task(broken.id, TaskStatus::Failed),
            task(calm.id, TaskStatus::Cancelled),
        ];
        let approvals = vec![approval(Some(busy.id)), approval(None)];
        let mut spend = HashMap::new();
        spend.insert(busy.id, (1.5, 12.0));
        let live = vec![busy.id];
        let projects = vec![calm.clone(), busy.clone(), broken.clone()];
        let (rows, totals) = aggregate(&AggregateInput {
            projects: &projects,
            agents: &agents,
            tasks: &tasks,
            approvals: &approvals,
            live_runs: &live,
            spend: &spend,
        });
        assert_eq!(totals.projects, 3);
        assert_eq!(totals.active_agents, 1);
        assert_eq!(totals.total_agents, 3);
        assert_eq!(totals.running_tasks, 1);
        assert_eq!(totals.failed_tasks, 1);
        assert_eq!(totals.pending_approvals, 2);
        assert_eq!(totals.live_runs, 1);

        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // Attention first (Busy has an approval and more agents than Broken), idle last.
        assert_eq!(names, vec!["Busy", "Broken", "Calm"]);
        let b = &rows[0];
        assert_eq!(b.activity, ProjectActivity::Attention);
        assert_eq!((b.active_agents, b.total_agents), (1, 2));
        assert_eq!((b.running_tasks, b.open_tasks, b.completed_tasks), (1, 2, 1));
        assert_eq!(b.pending_approvals, 1);
        assert_eq!(b.live_runs, 1);
        assert!((b.spend_month_usd - 12.0).abs() < f64::EPSILON);
        assert_eq!(rows[1].activity, ProjectActivity::Attention);
        assert_eq!(rows[2].activity, ProjectActivity::Idle);
        assert_eq!(rows[2].open_tasks, 0);
    }

    #[test]
    fn activity_rule() {
        let mut row = ProjectOverview {
            project_id: ProjectId::new(),
            name: "x".into(),
            color: None,
            activity: ProjectActivity::Idle,
            active_agents: 0,
            total_agents: 0,
            running_tasks: 0,
            open_tasks: 0,
            failed_tasks: 0,
            completed_tasks: 0,
            pending_approvals: 0,
            live_runs: 0,
            spend_today_usd: 0.0,
            spend_month_usd: 0.0,
        };
        assert_eq!(activity_of(&row), ProjectActivity::Idle);
        row.live_runs = 1;
        assert_eq!(activity_of(&row), ProjectActivity::Active);
        row.pending_approvals = 1;
        assert_eq!(activity_of(&row), ProjectActivity::Attention);
        assert!(agent_active(AgentState::Idle));
        assert!(agent_active(AgentState::Waiting));
        assert!(!agent_active(AgentState::Error));
    }

    fn proc(pid: u32, parent: Option<u32>, name: &str, cpu: f32, mem: u64) -> ProcInfo {
        ProcInfo { pid, parent, name: name.into(), cpu_core_percent: cpu, memory_bytes: mem }
    }

    #[test]
    fn folds_terminal_subtrees_and_other_children() {
        let core = 100;
        let procs = vec![
            proc(1, None, "init", 5.0, 1),
            proc(core, Some(1), "orkestr", 50.0, 1000),
            // conpty host → shell (terminal pid 201) → cargo
            proc(200, Some(core), "OpenConsole", 1.0, 10),
            proc(201, Some(200), "pwsh", 10.0, 100),
            proc(202, Some(201), "cargo", 100.0, 500),
            // CLI coding agent directly under the core, with a child
            proc(300, Some(core), "claude", 20.0, 300),
            proc(301, Some(300), "node", 20.0, 200),
            // unrelated
            proc(400, Some(1), "browser", 90.0, 9000),
        ];
        let roots = vec![
            ProcessRoot {
                kind: OrkestrProcessKind::Terminal,
                id: "t1".into(),
                label: "PowerShell".into(),
                project_id: None,
                agent_id: None,
                pid: Some(201),
                running: true,
            },
            ProcessRoot {
                kind: OrkestrProcessKind::AgentTerminal,
                id: "t2".into(),
                label: "gone".into(),
                project_id: None,
                agent_id: Some(AgentId::new()),
                pid: Some(999),
                running: false,
            },
        ];
        let (core_usage, rows) = fold_tree(core, &roots, &procs, 4.0);
        let core_usage = core_usage.expect("core sampled");
        assert!((core_usage.cpu_percent - 12.5).abs() < 0.01);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let t1 = &rows[0];
        assert_eq!(t1.process_count, 2);
        assert_eq!(t1.memory_bytes, Some(600));
        assert!((t1.cpu_percent.unwrap_or_default() - 27.5).abs() < 0.01);
        let t2 = &rows[1];
        assert_eq!(t2.cpu_percent, None);
        assert_eq!(t2.process_count, 0);
        // conpty host (200) holds terminal 201 → skipped; claude subtree reported.
        let child = &rows[2];
        assert_eq!(child.kind, OrkestrProcessKind::Child);
        assert_eq!(child.label, "claude");
        assert_eq!(child.process_count, 2);
        assert_eq!(child.memory_bytes, Some(500));
        assert!((child.cpu_percent.unwrap_or_default() - 10.0).abs() < 0.01);
    }

    #[test]
    fn storage_health_sizes_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("orkestr.db");
        std::fs::write(&db, vec![0_u8; 128]).expect("db");
        std::fs::write(dir.path().join("orkestr.db-wal"), vec![0_u8; 64]).expect("wal");
        let objects = dir.path().join("objects");
        std::fs::create_dir_all(objects.join("ab")).expect("objects");
        std::fs::write(objects.join("ab").join("cdef"), vec![0_u8; 10]).expect("obj");
        std::fs::write(objects.join("ab").join("0123"), vec![0_u8; 5]).expect("obj");
        let h = storage_health(Some(&db), &objects);
        assert_eq!(h.db_bytes, 128);
        assert_eq!(h.db_wal_bytes, 64);
        assert_eq!(h.object_count, 2);
        assert_eq!(h.object_store_bytes, 15);
        assert!(!h.object_scan_truncated);
        let missing = storage_health(None, &dir.path().join("nope"));
        assert_eq!(missing, StorageHealth::default());
    }
}
