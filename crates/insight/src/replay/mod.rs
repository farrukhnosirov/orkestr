//! Execution Replay (§21, §68.50).
//!
//! [`gather`] reads the persisted event log for a time window (binary search
//! on `seq` → forward paging, bounded by [`MAX_REPLAY_SCAN`]); [`build`] turns
//! the events plus the stored run / plan / request / task into a
//! [`ReplayTimeline`]; [`filter`] applies the agent / phase filters;
//! [`state_at`] and [`step`] implement step-through.
//!
//! Relevance is decided by identity, never by text: the run's plan and run
//! ids, its task ids, agents spawned for those tasks (discovered while
//! scanning), approvals they requested, and — as a second pass — any other
//! step-worthy event sharing a correlation id with an included event.
//! Message deltas (`AgentMessage`, `ManagerMessage`) are ephemeral and never
//! persisted, and hidden chain-of-thought is never stored at all, so a replay
//! only ever shows visible, recorded actions.

use std::collections::{BTreeMap, HashMap, HashSet};

use orkestr_domain::{
    AgentId, AgentRole, AgentState, ApprovalId, ApprovalStatus, CorrelationId, ProjectId, ReviewState, Task, TaskId,
    TaskStatus, Timestamp, ToolCallId, ToolCallStatus,
};
use orkestr_events::{EventBus, ReplayQuery};
use orkestr_protocol::event::{Event, EventKind, StopReason};
use orkestr_protocol::methods::consensus::ConsensusStatus;
use orkestr_protocol::methods::manager::{ManagerChatMessage, ManagerPlan, ManagerRun, ManagerRunStatus, PlanStatus};
use orkestr_protocol::methods::merge::MergeProposalStatus;
use orkestr_protocol::methods::replay::{
    ReplayAgent, ReplayAgentState, ReplayDirection, ReplayPhase, ReplayPhaseSummary, ReplayRunSummary, ReplayState,
    ReplayStep, ReplayStepResult, ReplayStepStatus, ReplaySubject, ReplayTaskRef, ReplayTaskState, ReplayTimeline,
    MAX_REPLAY_SCAN, MAX_REPLAY_STEPS,
};
use orkestr_protocol::methods::reviews::ReviewStatus;
use orkestr_protocol::methods::teams::TeamLeadStatus;

use crate::error::InsightResult;

/// Max characters of the request text shown in the first step.
pub const MAX_REQUEST_CHARS: usize = 2000;
/// Page size used while scanning the log.
const PAGE: u32 = 1000;

/// Stored records a timeline is built around.
#[derive(Debug, Clone)]
pub struct ReplaySeeds {
    /// Project.
    pub project_id: ProjectId,
    /// Subject kind.
    pub subject: ReplaySubject,
    /// The run (run subjects).
    pub run: Option<ManagerRun>,
    /// Its plan.
    pub plan: Option<ManagerPlan>,
    /// The user message that led to the plan.
    pub request: Option<ManagerChatMessage>,
    /// The task (task subjects).
    pub task: Option<Task>,
}

impl ReplaySeeds {
    /// Time window of the subject: `(from, to)`; `to = None` = up to now.
    #[must_use]
    pub fn window(&self) -> (Timestamp, Option<Timestamp>) {
        let slack = chrono::Duration::seconds(2);
        let mut from: Option<Timestamp> = None;
        let mut consider = |t: Timestamp| from = Some(from.map_or(t, |f: Timestamp| f.min(t)));
        if let Some(r) = &self.request {
            consider(r.created_at);
        }
        if let Some(p) = &self.plan {
            consider(p.created_at);
        }
        if let Some(r) = &self.run {
            consider(r.started_at);
        }
        if let Some(t) = &self.task {
            consider(t.created_at);
        }
        let from = from.unwrap_or_else(orkestr_domain::now) - slack;
        let to = match (&self.run, &self.task) {
            (Some(r), _) => r.finished_at.map(|f| f + chrono::Duration::seconds(30)),
            (None, Some(t)) if matches!(t.status, TaskStatus::Completed | TaskStatus::Cancelled) => {
                Some(t.updated_at + chrono::Duration::seconds(30))
            }
            _ => None,
        };
        (from, to)
    }
}

/// Events read from the log.
#[derive(Debug, Clone, Default)]
pub struct Gathered {
    /// Events in ascending `seq` order.
    pub events: Vec<Event>,
    /// Events read.
    pub scanned: u32,
    /// The scan cap was hit.
    pub truncated: bool,
}

/// Reads every persisted event with `from <= ts <= to` (bounded).
pub async fn gather(
    bus: &EventBus,
    from: Timestamp,
    to: Option<Timestamp>,
    max_scan: usize,
) -> InsightResult<Gathered> {
    let last = bus.last_seq();
    let mut out = Gathered::default();
    if last == 0 {
        return Ok(out);
    }
    // Binary search for the first seq whose event is at/after `from`.
    let (mut lo, mut hi) = (1u64, last + 1);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let page = bus
            .replay(&ReplayQuery { from_seq: mid, to_seq: None, limit: 1, topics: None, newest_first: false })
            .await?;
        match page.events.first() {
            Some(e) if e.ts < from => lo = e.seq + 1,
            Some(e) => hi = mid.min(e.seq),
            None => hi = mid,
        }
    }
    let mut cursor = lo;
    'outer: loop {
        let page = bus
            .replay(&ReplayQuery { from_seq: cursor, to_seq: None, limit: PAGE, topics: None, newest_first: false })
            .await?;
        if page.events.is_empty() {
            break;
        }
        for e in page.events {
            cursor = e.seq + 1;
            out.scanned = out.scanned.saturating_add(1);
            if to.is_some_and(|t| e.ts > t) {
                break 'outer;
            }
            if e.ts >= from {
                out.events.push(e);
            }
            if out.scanned as usize >= max_scan {
                out.truncated = true;
                break 'outer;
            }
        }
        if !page.has_more {
            break;
        }
    }
    Ok(out)
}

/// Gathers with the default cap.
pub async fn gather_default(bus: &EventBus, seeds: &ReplaySeeds) -> InsightResult<Gathered> {
    let (from, to) = seeds.window();
    gather(bus, from, to, MAX_REPLAY_SCAN).await
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

struct Ctx {
    plan_id: Option<orkestr_domain::ManagerPlanId>,
    run_id: Option<orkestr_domain::ManagerRunId>,
    tasks: Vec<ReplayTaskRef>,
    task_set: HashSet<TaskId>,
    agents: Vec<ReplayAgent>,
    agent_set: HashSet<AgentId>,
    approvals: HashSet<ApprovalId>,
}

impl Ctx {
    fn task_title(&self, id: TaskId) -> String {
        self.tasks.iter().find(|t| t.task_id == id).map(|t| t.title.clone()).unwrap_or_default()
    }

    fn agent_name(&self, id: AgentId) -> String {
        self.agents.iter().find(|a| a.agent_id == id).map(|a| a.name.clone()).unwrap_or_default()
    }

    fn add_agent(&mut self, id: AgentId, name: &str, role: Option<AgentRole>, task: Option<TaskId>) {
        if let Some(a) = self.agents.iter_mut().find(|a| a.agent_id == id) {
            if a.name.is_empty() {
                name.clone_into(&mut a.name);
            }
            if a.role.is_none() {
                a.role = role;
            }
            a.task_id = a.task_id.or(task);
        } else {
            self.agents.push(ReplayAgent { agent_id: id, name: name.to_owned(), role, task_id: task, steps: 0 });
        }
        self.agent_set.insert(id);
    }

    /// Identity-based relevance; may register discovered agents / approvals.
    fn relevant(&mut self, e: &Event) -> bool {
        match &e.kind {
            EventKind::ManagerPlanUpdated(p) => return Some(p.plan.id) == self.plan_id,
            EventKind::ManagerRunUpdated(r) => return Some(r.run.id) == self.run_id,
            EventKind::AgentSpawned(a) => {
                let hit =
                    a.agent.task_id.is_some_and(|t| self.task_set.contains(&t)) || self.agent_set.contains(&a.agent.id);
                if hit {
                    self.add_agent(a.agent.id, &a.agent.name, Some(a.agent.role.clone()), a.agent.task_id);
                }
                return hit;
            }
            EventKind::ApprovalResolved(a) => return self.approvals.contains(&a.approval_id),
            // Phase 2 kinds scoped by their payload rather than the envelope.
            EventKind::MergeProposalUpdated(m) => {
                let p = &m.proposal;
                return (p.run_id.is_some() && p.run_id == self.run_id)
                    || p.branches.iter().any(|b| b.task_id.is_some_and(|t| self.task_set.contains(&t)));
            }
            EventKind::MergeRiskDetected(m) => {
                return self.task_set.contains(&m.risk.task_a) || self.task_set.contains(&m.risk.task_b);
            }
            EventKind::TeamLeadRunUpdated(l) => {
                let lead = &l.lead;
                let hit = self.task_set.contains(&lead.parent_task_id)
                    || (lead.run_id.is_some() && lead.run_id == self.run_id);
                if hit {
                    // The lead's sub-tasks (and so their executors) belong to the replay.
                    for st in &lead.subtasks {
                        if self.task_set.insert(st.task_id) {
                            self.tasks.push(ReplayTaskRef {
                                task_id: st.task_id,
                                title: st.title.clone(),
                                status: st.status,
                            });
                        }
                    }
                    if let Some(a) = lead.lead_agent_id {
                        self.agent_set.insert(a);
                    }
                }
                return hit;
            }
            _ => {}
        }
        if !step_worthy(&e.kind) {
            return false;
        }
        let hit = e.task_id.is_some_and(|t| self.task_set.contains(&t))
            || e.agent_id.is_some_and(|a| self.agent_set.contains(&a));
        if hit {
            if let EventKind::ApprovalRequested(a) = &e.kind {
                self.approvals.insert(a.approval.id);
            }
        }
        hit
    }
}

/// Kinds that can become steps (everything else is noise for a replay).
fn step_worthy(kind: &EventKind) -> bool {
    match kind {
        EventKind::TaskCreated(_)
        | EventKind::TaskStatusChanged(_)
        | EventKind::AgentSpawned(_)
        | EventKind::AgentStarted(_)
        | EventKind::AgentStateChanged(_)
        | EventKind::AgentStopped(_)
        | EventKind::ToolCallStarted(_)
        | EventKind::ToolCallFinished(_)
        | EventKind::FileChanged(_)
        | EventKind::BuildStarted(_)
        | EventKind::BuildFailed(_)
        | EventKind::BuildSucceeded(_)
        | EventKind::TestRunFinished(_)
        | EventKind::ReviewFinished(_)
        | EventKind::ApprovalRequested(_)
        | EventKind::ApprovalResolved(_)
        | EventKind::SnapshotCreated(_)
        | EventKind::RollbackPerformed(_)
        | EventKind::DecisionRecorded(_)
        | EventKind::EmergencyStop(_)
        | EventKind::ManagerPlanUpdated(_)
        | EventKind::ManagerRunUpdated(_)
        | EventKind::ReviewUpdated(_)
        | EventKind::ConsensusUpdated(_)
        | EventKind::MergeProposalUpdated(_)
        | EventKind::MergeRiskDetected(_)
        | EventKind::TeamLeadRunUpdated(_) => true,
        // Later append-only merge kinds are still recognised by name.
        other => other.name().contains("Merge"),
    }
}

fn base_step(e: &Event, phase: ReplayPhase, kind: &str, title: String) -> ReplayStep {
    ReplayStep {
        index: 0,
        seq: Some(e.seq),
        ts: e.ts,
        phase,
        kind: kind.to_owned(),
        status: ReplayStepStatus::Info,
        title,
        detail: None,
        agent_id: e.agent_id,
        task_id: e.task_id,
        correlation_id: e.correlation_id,
        files: Vec::new(),
        duration_ms: None,
        task_status: None,
        agent_state: None,
        tool_status: None,
        approval_id: None,
        approval_status: None,
        verdict: None,
        tests_passed: None,
        tests_failed: None,
        run_status: None,
    }
}

fn tool_tone(s: ToolCallStatus) -> ReplayStepStatus {
    match s {
        ToolCallStatus::Succeeded => ReplayStepStatus::Success,
        ToolCallStatus::Failed | ToolCallStatus::Denied => ReplayStepStatus::Error,
        ToolCallStatus::AwaitingApproval | ToolCallStatus::Cancelled => ReplayStepStatus::Warning,
        ToolCallStatus::Pending | ToolCallStatus::Running => ReplayStepStatus::Info,
    }
}

fn run_tone(s: ManagerRunStatus) -> ReplayStepStatus {
    match s {
        ManagerRunStatus::Completed => ReplayStepStatus::Success,
        ManagerRunStatus::Failed => ReplayStepStatus::Error,
        ManagerRunStatus::Cancelled | ManagerRunStatus::Interrupted | ManagerRunStatus::Paused => {
            ReplayStepStatus::Warning
        }
        ManagerRunStatus::Running => ReplayStepStatus::Info,
    }
}

fn plan_kind(s: PlanStatus) -> &'static str {
    match s {
        PlanStatus::Proposed => "PlanProposed",
        PlanStatus::Approved => "PlanApproved",
        PlanStatus::Rejected => "PlanRejected",
        PlanStatus::Superseded => "PlanSuperseded",
        PlanStatus::Running => "PlanRunning",
        PlanStatus::Completed => "PlanCompleted",
        PlanStatus::Failed => "PlanFailed",
        PlanStatus::Cancelled => "PlanCancelled",
    }
}

/// Builds the full (unfiltered) timeline.
#[must_use]
pub fn build(seeds: &ReplaySeeds, gathered: &Gathered) -> ReplayTimeline {
    let mut ctx = Ctx {
        plan_id: seeds.plan.as_ref().map(|p| p.id).or_else(|| seeds.run.as_ref().map(|r| r.plan_id)),
        run_id: seeds.run.as_ref().map(|r| r.id),
        tasks: Vec::new(),
        task_set: HashSet::new(),
        agents: Vec::new(),
        agent_set: HashSet::new(),
        approvals: HashSet::new(),
    };
    if let Some(run) = &seeds.run {
        for t in &run.tasks {
            ctx.tasks.push(ReplayTaskRef { task_id: t.task_id, title: t.title.clone(), status: t.status });
            ctx.task_set.insert(t.task_id);
            if let Some(a) = t.agent_id {
                ctx.add_agent(a, "", Some(t.role.clone()), Some(t.task_id));
            }
        }
    }
    if let Some(task) = &seeds.task {
        if !ctx.task_set.contains(&task.id) {
            ctx.tasks.push(ReplayTaskRef { task_id: task.id, title: task.title.clone(), status: task.status });
            ctx.task_set.insert(task.id);
        }
        if let Some(a) = task.agent_id {
            ctx.add_agent(a, "", None, Some(task.id));
        }
    }

    // Pass 1: identity; pass 2: shared correlation ids.
    let mut included: Vec<bool> = gathered.events.iter().map(|e| ctx.relevant(e)).collect();
    let corr: HashSet<CorrelationId> =
        gathered.events.iter().zip(&included).filter(|(_, inc)| **inc).filter_map(|(e, _)| e.correlation_id).collect();
    if !corr.is_empty() {
        for (e, inc) in gathered.events.iter().zip(included.iter_mut()) {
            if !*inc
                && step_worthy(&e.kind)
                && !matches!(e.kind, EventKind::ManagerPlanUpdated(_) | EventKind::ManagerRunUpdated(_))
                && e.correlation_id.is_some_and(|c| corr.contains(&c))
                && e.project_id.is_none_or(|p| p == seeds.project_id)
            {
                *inc = true;
            }
        }
    }

    let mut steps: Vec<ReplayStep> = Vec::new();
    if let Some(req) = &seeds.request {
        steps.push(base_step_synthetic(
            req.created_at,
            ReplayPhase::Request,
            "UserRequest",
            truncate(&req.text, MAX_REQUEST_CHARS),
        ));
    } else if let Some(plan) = &seeds.plan {
        steps.push(base_step_synthetic(
            plan.created_at,
            ReplayPhase::Request,
            "UserRequest",
            truncate(&plan.goal, MAX_REQUEST_CHARS),
        ));
    }

    let mut tool_steps: HashMap<ToolCallId, usize> = HashMap::new();
    let mut approval_titles: HashMap<ApprovalId, String> = HashMap::new();
    let mut merge_status: HashMap<orkestr_domain::MergeProposalId, MergeProposalStatus> = HashMap::new();
    let mut lead_status: HashMap<String, TeamLeadStatus> = HashMap::new();
    let mut last_plan_status: Option<PlanStatus> = None;
    let mut last_run_status: Option<ManagerRunStatus> = None;
    let mut merge_reported = false;
    let mut task_titles: HashMap<TaskId, String> = ctx.tasks.iter().map(|t| (t.task_id, t.title.clone())).collect();

    for (e, _) in gathered.events.iter().zip(&included).filter(|(_, inc)| **inc) {
        match &e.kind {
            EventKind::ManagerPlanUpdated(p) => {
                if last_plan_status == Some(p.plan.status) {
                    continue;
                }
                last_plan_status = Some(p.plan.status);
                let mut s = base_step(e, ReplayPhase::Plan, plan_kind(p.plan.status), p.plan.goal.clone());
                s.detail = Some(p.plan.summary.clone()).filter(|x| !x.is_empty());
                s.status = match p.plan.status {
                    PlanStatus::Approved | PlanStatus::Completed => ReplayStepStatus::Success,
                    PlanStatus::Rejected | PlanStatus::Cancelled | PlanStatus::Superseded => ReplayStepStatus::Warning,
                    PlanStatus::Failed => ReplayStepStatus::Error,
                    PlanStatus::Proposed | PlanStatus::Running => ReplayStepStatus::Info,
                };
                steps.push(s);
            }
            EventKind::ManagerRunUpdated(r) => {
                let run = &r.run;
                if last_run_status != Some(run.status) {
                    last_run_status = Some(run.status);
                    let mut s = base_step(e, ReplayPhase::Run, "RunUpdated", run.goal.clone());
                    s.run_status = Some(run.status);
                    s.status = run_tone(run.status);
                    s.detail = run.status_reason.clone();
                    steps.push(s);
                }
                if !merge_reported {
                    if let Some(report) = &run.report {
                        merge_reported = true;
                        for mp in &report.merge_proposals {
                            let mut s = base_step(e, ReplayPhase::Merge, "MergeProposal", mp.title.clone());
                            s.task_id = Some(mp.task_id);
                            s.detail = Some(mp.branch.clone());
                            s.files.clone_from(&mp.files_changed);
                            steps.push(s);
                        }
                    }
                }
            }
            EventKind::TaskCreated(t) => {
                task_titles.insert(t.task.id, t.task.title.clone());
                let mut s = base_step(e, ReplayPhase::Plan, "TaskCreated", t.task.title.clone());
                s.task_id = Some(t.task.id);
                s.task_status = Some(t.task.status);
                steps.push(s);
            }
            EventKind::TaskStatusChanged(t) => {
                let phase = match t.to {
                    TaskStatus::Running => ReplayPhase::Delegation,
                    TaskStatus::Testing => ReplayPhase::Tests,
                    TaskStatus::Review => ReplayPhase::Reviews,
                    _ => ReplayPhase::Run,
                };
                let title = task_titles.get(&t.task_id).cloned().unwrap_or_else(|| ctx.task_title(t.task_id));
                let mut s = base_step(e, phase, "TaskStatusChanged", title);
                s.task_id = Some(t.task_id);
                s.task_status = Some(t.to);
                s.detail.clone_from(&t.reason);
                s.status = match t.to {
                    TaskStatus::Completed => ReplayStepStatus::Success,
                    TaskStatus::Failed => ReplayStepStatus::Error,
                    TaskStatus::Cancelled | TaskStatus::Interrupted | TaskStatus::Blocked => ReplayStepStatus::Warning,
                    _ => ReplayStepStatus::Info,
                };
                steps.push(s);
            }
            EventKind::AgentSpawned(a) => {
                let mut s = base_step(e, ReplayPhase::Agents, "AgentSpawned", a.agent.name.clone());
                s.agent_id = Some(a.agent.id);
                s.task_id = a.agent.task_id;
                s.agent_state = Some(a.agent.state);
                s.detail = a.agent.model.as_ref().map(|m| format!("{}/{}", m.provider_id, m.model));
                steps.push(s);
            }
            EventKind::AgentStarted(a) => {
                let mut s = base_step(e, ReplayPhase::Delegation, "AgentStarted", ctx.agent_name(a.agent_id));
                s.agent_id = Some(a.agent_id);
                s.task_id = a.task_id.or(e.task_id);
                if let Some(t) = s.task_id {
                    s.detail = task_titles.get(&t).cloned();
                }
                steps.push(s);
            }
            EventKind::AgentStateChanged(a) => {
                if !matches!(a.to, AgentState::Error | AgentState::Waiting | AgentState::Completed) {
                    continue;
                }
                let mut s = base_step(e, ReplayPhase::Agents, "AgentStateChanged", ctx.agent_name(a.agent_id));
                s.agent_id = Some(a.agent_id);
                s.agent_state = Some(a.to);
                s.detail.clone_from(&a.activity);
                s.status = match a.to {
                    AgentState::Error => ReplayStepStatus::Error,
                    AgentState::Waiting => ReplayStepStatus::Warning,
                    _ => ReplayStepStatus::Success,
                };
                steps.push(s);
            }
            EventKind::AgentStopped(a) => {
                let mut s = base_step(e, ReplayPhase::Agents, "AgentStopped", ctx.agent_name(a.agent_id));
                s.agent_id = Some(a.agent_id);
                s.agent_state = Some(AgentState::Stopped);
                s.detail.clone_from(&a.detail);
                s.status = match a.reason {
                    StopReason::Completed => ReplayStepStatus::Success,
                    StopReason::Error | StopReason::EmergencyStop => ReplayStepStatus::Error,
                    _ => ReplayStepStatus::Warning,
                };
                steps.push(s);
            }
            EventKind::ToolCallStarted(t) => {
                let mut s = base_step(e, ReplayPhase::Tools, "ToolCall", t.tool_call.tool.clone());
                s.agent_id = Some(t.tool_call.agent_id);
                s.task_id = t.tool_call.task_id.or(e.task_id);
                s.tool_status = Some(t.tool_call.status);
                s.approval_id = t.tool_call.approval_id;
                s.status = tool_tone(t.tool_call.status);
                tool_steps.insert(t.tool_call.id, steps.len());
                steps.push(s);
            }
            EventKind::ToolCallFinished(t) => {
                if let Some(&idx) = tool_steps.get(&t.tool_call_id) {
                    let s = &mut steps[idx];
                    s.tool_status = Some(t.status);
                    s.status = tool_tone(t.status);
                    s.duration_ms = Some(t.duration_ms);
                    s.detail.clone_from(&t.output_summary);
                } else {
                    let mut s = base_step(e, ReplayPhase::Tools, "ToolCall", String::new());
                    s.tool_status = Some(t.status);
                    s.status = tool_tone(t.status);
                    s.duration_ms = Some(t.duration_ms);
                    s.detail.clone_from(&t.output_summary);
                    steps.push(s);
                }
            }
            EventKind::FileChanged(f) => {
                let mut s = base_step(e, ReplayPhase::Files, "FileChanged", f.path.clone());
                s.agent_id = f.agent_id.or(e.agent_id);
                s.files.push(f.path.clone());
                if let Some(old) = &f.old_path {
                    s.files.push(old.clone());
                }
                s.detail = Some(format!("{:?}", f.change));
                steps.push(s);
            }
            EventKind::SnapshotCreated(sn) => {
                let mut s = base_step(e, ReplayPhase::Files, "SnapshotCreated", sn.snapshot.label.clone());
                s.task_id = sn.snapshot.task_id.or(e.task_id);
                steps.push(s);
            }
            EventKind::RollbackPerformed(r) => {
                let mut s = base_step(e, ReplayPhase::Files, "RollbackPerformed", r.snapshot_id.to_string());
                s.status = ReplayStepStatus::Warning;
                s.detail = Some(r.files_restored.to_string());
                steps.push(s);
            }
            EventKind::BuildStarted(b) => {
                steps.push(base_step(e, ReplayPhase::Tests, "BuildStarted", b.command.clone()))
            }
            EventKind::BuildSucceeded(b) | EventKind::BuildFailed(b) => {
                let ok = matches!(e.kind, EventKind::BuildSucceeded(_));
                let mut s = base_step(
                    e,
                    ReplayPhase::Tests,
                    if ok { "BuildSucceeded" } else { "BuildFailed" },
                    b.summary.clone(),
                );
                s.duration_ms = Some(b.duration_ms);
                s.status = if ok { ReplayStepStatus::Success } else { ReplayStepStatus::Error };
                steps.push(s);
            }
            EventKind::TestRunFinished(t) => {
                let mut s = base_step(e, ReplayPhase::Tests, "TestRunFinished", t.summary.clone());
                s.tests_passed = Some(t.passed);
                s.tests_failed = Some(t.failed);
                s.duration_ms = Some(t.duration_ms);
                s.status = if t.failed > 0 { ReplayStepStatus::Error } else { ReplayStepStatus::Success };
                steps.push(s);
            }
            EventKind::ReviewFinished(r) => {
                let mut s = base_step(e, ReplayPhase::Reviews, "ReviewFinished", r.summary.clone());
                s.verdict = Some(r.verdict);
                if let Some(a) = r.reviewer_agent_id {
                    s.agent_id = Some(a);
                }
                s.detail = Some(r.findings.to_string());
                s.status = match r.verdict {
                    ReviewState::Approved | ReviewState::NotRequired => ReplayStepStatus::Success,
                    ReviewState::ChangesRequested | ReviewState::Pending => ReplayStepStatus::Warning,
                    ReviewState::Rejected => ReplayStepStatus::Error,
                };
                steps.push(s);
            }
            EventKind::ApprovalRequested(a) => {
                approval_titles.insert(a.approval.id, a.approval.action_summary.clone());
                let mut s =
                    base_step(e, ReplayPhase::Approvals, "ApprovalRequested", a.approval.action_summary.clone());
                s.approval_id = Some(a.approval.id);
                s.approval_status = Some(a.approval.status);
                s.detail = Some(a.approval.reason.clone()).filter(|r| !r.is_empty());
                s.status = ReplayStepStatus::Warning;
                steps.push(s);
            }
            EventKind::ApprovalResolved(a) => {
                let title = approval_titles.get(&a.approval_id).cloned().unwrap_or_default();
                let mut s = base_step(e, ReplayPhase::Approvals, "ApprovalResolved", title);
                s.approval_id = Some(a.approval_id);
                s.approval_status = Some(a.status);
                s.status = match a.status {
                    ApprovalStatus::Approved => ReplayStepStatus::Success,
                    ApprovalStatus::Rejected => ReplayStepStatus::Error,
                    _ => ReplayStepStatus::Warning,
                };
                steps.push(s);
            }
            EventKind::DecisionRecorded(d) => {
                steps.push(base_step(e, ReplayPhase::Plan, "DecisionRecorded", d.decision.title.clone()));
            }
            EventKind::EmergencyStop(_) => {
                let mut s = base_step(e, ReplayPhase::Run, "EmergencyStop", String::new());
                s.status = ReplayStepStatus::Error;
                steps.push(s);
            }
            // Failed reviews (finished ones arrive as `ReviewFinished`).
            EventKind::ReviewUpdated(r) if r.review.status == ReviewStatus::Failed => {
                let mut s = base_step(e, ReplayPhase::Reviews, "ReviewUpdated", r.review.summary.clone());
                s.detail = r.review.error.clone();
                s.status = ReplayStepStatus::Error;
                steps.push(s);
            }
            EventKind::ConsensusUpdated(c) if c.run.status.is_final() => {
                let mut s = base_step(e, ReplayPhase::Plan, "ConsensusUpdated", truncate(&c.run.question, 200));
                s.status = if c.run.status == ConsensusStatus::Finished {
                    ReplayStepStatus::Success
                } else {
                    ReplayStepStatus::Error
                };
                s.detail = c.run.error.clone();
                steps.push(s);
            }
            EventKind::TeamLeadRunUpdated(l) => {
                let lead = &l.lead;
                if lead_status.insert(lead.id.clone(), lead.status) == Some(lead.status) {
                    continue;
                }
                let mut s = base_step(e, ReplayPhase::Delegation, "TeamLeadRunUpdated", lead.title.clone());
                s.task_id = Some(lead.parent_task_id);
                s.agent_id = lead.lead_agent_id.or(s.agent_id);
                s.detail = Some(format!("{:?}", lead.status));
                s.status = match lead.status {
                    TeamLeadStatus::Completed => ReplayStepStatus::Success,
                    TeamLeadStatus::Failed => ReplayStepStatus::Error,
                    TeamLeadStatus::Cancelled => ReplayStepStatus::Warning,
                    _ => ReplayStepStatus::Info,
                };
                steps.push(s);
            }
            EventKind::MergeProposalUpdated(m) => {
                let p = &m.proposal;
                // One step per status change, not per progress update.
                if merge_status.insert(p.id, p.status) == Some(p.status) {
                    continue;
                }
                let mut s = base_step(e, ReplayPhase::Merge, "MergeProposalUpdated", p.target.clone());
                // `<Status>` or `<Status>: <technical detail>` (the UI translates the status).
                s.detail = Some(match &p.status_detail {
                    Some(d) => format!("{:?}: {}", p.status, truncate(d, 300)),
                    None => format!("{:?}", p.status),
                });
                s.files = p.conflicts.iter().flat_map(|c| c.paths.iter().cloned()).take(50).collect();
                s.status = match p.status {
                    MergeProposalStatus::Completed => ReplayStepStatus::Success,
                    MergeProposalStatus::Failed | MergeProposalStatus::Rejected => ReplayStepStatus::Error,
                    MergeProposalStatus::Conflicted
                    | MergeProposalStatus::Aborted
                    | MergeProposalStatus::AwaitingApproval => ReplayStepStatus::Warning,
                    _ => ReplayStepStatus::Info,
                };
                steps.push(s);
            }
            EventKind::MergeRiskDetected(m) => {
                let r = &m.risk;
                let mut s = base_step(e, ReplayPhase::Merge, "MergeRiskDetected", String::new());
                s.task_id = Some(r.task_a);
                s.files = r.files.iter().take(50).cloned().collect();
                s.detail = Some(format!("{} ↔ {}", ctx.task_title(r.task_a), ctx.task_title(r.task_b)));
                s.status = ReplayStepStatus::Warning;
                steps.push(s);
            }
            other if other.name().contains("Merge") => {
                steps.push(base_step(e, ReplayPhase::Merge, other.name(), String::new()));
            }
            _ => {}
        }
    }

    // Stable time order (synthetic request first on ties).
    steps.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.seq.unwrap_or(0).cmp(&b.seq.unwrap_or(0))));
    let mut truncated = gathered.truncated;
    if steps.len() > MAX_REPLAY_STEPS {
        steps.truncate(MAX_REPLAY_STEPS);
        truncated = true;
    }
    for (i, s) in steps.iter_mut().enumerate() {
        s.index = u32::try_from(i).unwrap_or(u32::MAX);
    }
    // Fill names discovered later, count steps per agent, final task statuses.
    for a in &mut ctx.agents {
        a.steps = u32::try_from(steps.iter().filter(|s| s.agent_id == Some(a.agent_id)).count()).unwrap_or(u32::MAX);
    }
    let title = seeds
        .run
        .as_ref()
        .map(|r| r.goal.clone())
        .or_else(|| seeds.task.as_ref().map(|t| t.title.clone()))
        .unwrap_or_default();
    let total = u32::try_from(steps.len()).unwrap_or(u32::MAX);
    let mut tl = ReplayTimeline {
        project_id: seeds.project_id,
        subject: seeds.subject,
        run_id: seeds.run.as_ref().map(|r| r.id),
        plan_id: ctx.plan_id,
        task_id: seeds.task.as_ref().map(|t| t.id),
        title,
        run_status: seeds.run.as_ref().map(|r| r.status),
        task_status: seeds.task.as_ref().map(|t| t.status),
        started_at: steps.first().map(|s| s.ts),
        ended_at: steps.last().map(|s| s.ts),
        steps,
        agents: ctx.agents,
        tasks: ctx.tasks,
        phases: Vec::new(),
        total_steps: total,
        events_scanned: gathered.scanned,
        truncated,
    };
    tl.phases = phase_summary(&tl.steps);
    tl
}

fn base_step_synthetic(ts: Timestamp, phase: ReplayPhase, kind: &str, title: String) -> ReplayStep {
    ReplayStep {
        index: 0,
        seq: None,
        ts,
        phase,
        kind: kind.to_owned(),
        status: ReplayStepStatus::Info,
        title,
        detail: None,
        agent_id: None,
        task_id: None,
        correlation_id: None,
        files: Vec::new(),
        duration_ms: None,
        task_status: None,
        agent_state: None,
        tool_status: None,
        approval_id: None,
        approval_status: None,
        verdict: None,
        tests_passed: None,
        tests_failed: None,
        run_status: None,
    }
}

fn phase_summary(steps: &[ReplayStep]) -> Vec<ReplayPhaseSummary> {
    let mut map: BTreeMap<ReplayPhase, ReplayPhaseSummary> = BTreeMap::new();
    for s in steps {
        let e = map.entry(s.phase).or_insert(ReplayPhaseSummary { phase: s.phase, steps: 0, first_index: s.index });
        e.steps += 1;
    }
    map.into_values().collect()
}

/// Applies the agent / phase filters and renumbers the steps.
#[must_use]
pub fn filter(mut tl: ReplayTimeline, agent: Option<AgentId>, phases: Option<&[ReplayPhase]>) -> ReplayTimeline {
    if agent.is_none() && phases.is_none_or(<[ReplayPhase]>::is_empty) {
        return tl;
    }
    let agent_task = agent.and_then(|a| tl.agents.iter().find(|x| x.agent_id == a).and_then(|x| x.task_id));
    tl.steps.retain(|s| {
        let agent_ok = match agent {
            None => true,
            Some(a) => {
                s.agent_id == Some(a) || (s.agent_id.is_none() && s.task_id.is_some() && s.task_id == agent_task)
            }
        };
        let phase_ok = phases.is_none_or(|p| p.is_empty() || p.contains(&s.phase));
        agent_ok && phase_ok
    });
    for (i, s) in tl.steps.iter_mut().enumerate() {
        s.index = u32::try_from(i).unwrap_or(u32::MAX);
    }
    tl.started_at = tl.steps.first().map(|s| s.ts);
    tl.ended_at = tl.steps.last().map(|s| s.ts);
    tl.phases = phase_summary(&tl.steps);
    tl
}

/// State reconstructed up to and including step `index`.
#[must_use]
pub fn state_at(tl: &ReplayTimeline, index: usize) -> ReplayState {
    let mut st = ReplayState {
        tasks: tl
            .tasks
            .iter()
            .map(|t| ReplayTaskState { task_id: t.task_id, title: t.title.clone(), status: None })
            .collect(),
        ..ReplayState::default()
    };
    let names: HashMap<AgentId, String> = tl.agents.iter().map(|a| (a.agent_id, a.name.clone())).collect();
    for s in tl.steps.iter().take(index.saturating_add(1)) {
        if let (Some(t), Some(status)) = (s.task_id, s.task_status) {
            match st.tasks.iter_mut().find(|x| x.task_id == t) {
                Some(x) => x.status = Some(status),
                None => st.tasks.push(ReplayTaskState { task_id: t, title: s.title.clone(), status: Some(status) }),
            }
        }
        if let Some(a) = s.agent_id {
            let spawned = s.kind == "AgentSpawned";
            if spawned && !st.agents.iter().any(|x| x.agent_id == a) {
                st.agents.push(ReplayAgentState {
                    agent_id: a,
                    name: names.get(&a).cloned().unwrap_or_else(|| s.title.clone()),
                    state: s.agent_state,
                    active: true,
                });
            } else if let Some(x) = st.agents.iter_mut().find(|x| x.agent_id == a) {
                if let Some(state) = s.agent_state {
                    x.state = Some(state);
                }
                if s.kind == "AgentStopped" {
                    x.active = false;
                }
            }
        }
        match s.kind.as_str() {
            "FileChanged" => {
                for f in &s.files {
                    if !st.files_changed.contains(f) {
                        st.files_changed.push(f.clone());
                    }
                }
            }
            "ToolCall" => {
                st.tool_calls += 1;
                if matches!(s.tool_status, Some(ToolCallStatus::Failed | ToolCallStatus::Denied)) {
                    st.tool_failures += 1;
                }
            }
            "ApprovalRequested" => st.approvals_pending += 1,
            "ApprovalResolved" => {
                st.approvals_pending = st.approvals_pending.saturating_sub(1);
                st.approvals_resolved += 1;
            }
            "TestRunFinished" => {
                st.tests_passed += s.tests_passed.unwrap_or(0);
                st.tests_failed += s.tests_failed.unwrap_or(0);
            }
            "ReviewFinished" => st.reviews += 1,
            _ => {}
        }
        if let Some(r) = s.run_status {
            st.run_status = Some(r);
        }
    }
    st
}

/// Moves the cursor and returns the step with its state.
#[must_use]
pub fn step(
    tl: &ReplayTimeline,
    index: u32,
    direction: ReplayDirection,
    phase: Option<ReplayPhase>,
) -> ReplayStepResult {
    let total = tl.steps.len();
    if total == 0 {
        return ReplayStepResult {
            step: None,
            index: 0,
            total: 0,
            has_prev: false,
            has_next: false,
            state: state_at(tl, 0),
        };
    }
    let last = total - 1;
    let cur = (index as usize).min(last);
    let next = match direction {
        ReplayDirection::First => 0,
        ReplayDirection::Last => last,
        ReplayDirection::Prev => cur.saturating_sub(1),
        ReplayDirection::Next => (cur + 1).min(last),
        ReplayDirection::Goto => cur,
        ReplayDirection::NextPhase => match phase {
            Some(p) => tl
                .steps
                .iter()
                .position(|s| s.phase == p && s.index as usize > cur)
                .or_else(|| tl.steps.iter().position(|s| s.phase == p))
                .unwrap_or(cur),
            None => {
                tl.steps.iter().position(|s| s.index as usize > cur && s.phase != tl.steps[cur].phase).unwrap_or(cur)
            }
        },
    };
    ReplayStepResult {
        step: tl.steps.get(next).cloned(),
        index: u32::try_from(next).unwrap_or(u32::MAX),
        total: u32::try_from(total).unwrap_or(u32::MAX),
        has_prev: next > 0,
        has_next: next < last,
        state: state_at(tl, next),
    }
}

/// Picker entry of a run.
#[must_use]
pub fn run_summary(run: &ManagerRun) -> ReplayRunSummary {
    ReplayRunSummary {
        run_id: run.id,
        plan_id: run.plan_id,
        goal: run.goal.clone(),
        status: run.status,
        started_at: run.started_at,
        finished_at: run.finished_at,
        tasks: run
            .tasks
            .iter()
            .map(|t| ReplayTaskRef { task_id: t.task_id, title: t.title.clone(), status: t.status })
            .collect(),
        agents_spawned: run.agents_spawned,
        cost_usd: run.cost_usd,
    }
}

/// The user request that led to a plan: the newest user message of the
/// plan's conversation created at or before the plan.
#[must_use]
pub fn request_for_plan(plan: &ManagerPlan, messages: &[ManagerChatMessage]) -> Option<ManagerChatMessage> {
    use orkestr_protocol::methods::manager::ManagerMessageRole;
    messages
        .iter()
        .filter(|m| m.role == ManagerMessageRole::User && m.created_at <= plan.created_at)
        .max_by_key(|m| m.created_at)
        .cloned()
}

#[cfg(test)]
mod tests;
