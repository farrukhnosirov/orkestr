//! Replay tests over synthetic event logs (in-memory event bus).

use std::sync::Arc;

use orkestr_domain::{
    AgentId, AgentInstance, AgentRole, AgentState, Approval, ApprovalId, ApprovalMode, ApprovalStatus, CorrelationId,
    ManagerPlanId, ManagerRunId, ProjectId, ReviewState, Task, TaskId, TaskStatus, Timestamp, ToolCall, ToolCallId,
    ToolCallStatus,
};
use orkestr_events::{EventBus, MemoryEventLog};
use orkestr_protocol::event::*;
use orkestr_protocol::methods::manager::*;
use orkestr_protocol::methods::replay::{ReplayDirection, ReplayPhase, ReplayStepStatus, ReplaySubject};
use serde_json::json;

use super::*;

fn ts(offset_ms: i64) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z").unwrap().with_timezone(&chrono::Utc)
        + chrono::Duration::milliseconds(offset_ms)
}

fn agent(id: AgentId, project: ProjectId, task: TaskId, name: &str) -> AgentInstance {
    let mut v = json!({
        "id": id, "projectId": project, "taskId": task, "name": name, "role": "Backend", "state": "Idle",
        "usage": orkestr_domain::UsageSummary::default(), "turnsUsed": 0, "createdAt": ts(0)
    });
    if let Some(o) = v.as_object_mut() {
        for k in [
            "templateId",
            "teamId",
            "parentAgentId",
            "model",
            "worktree",
            "lastError",
            "startedAt",
            "stoppedAt",
            "lastActivityAt",
        ] {
            o.insert(k.into(), serde_json::Value::Null);
        }
    }
    serde_json::from_value(v).expect("agent json")
}

fn tool_call(id: ToolCallId, agent: AgentId, task: TaskId, tool: &str) -> ToolCall {
    serde_json::from_value(json!({
        "id": id, "executionId": orkestr_domain::ExecutionId::new(), "agentId": agent, "taskId": task, "tool": tool,
        "capability": "Write", "input": {}, "outputSummary": null, "status": "Running", "approvalId": null,
        "startedAt": ts(0), "finishedAt": null
    }))
    .expect("tool call json")
}

fn approval(id: ApprovalId, project: ProjectId, task: TaskId, agent: AgentId) -> Approval {
    serde_json::from_value(json!({
        "id": id, "projectId": project, "taskId": task, "agentId": agent, "toolCallId": null, "capability": "Terminal",
        "actionSummary": "run npm install", "risk": "Medium", "reason": "network", "status": "Pending",
        "requestedAt": ts(0), "expiresAt": null, "resolvedAt": null, "resolvedBy": null, "resolutionNote": null
    }))
    .expect("approval json")
}

struct Fixture {
    project: ProjectId,
    run: ManagerRun,
    plan: ManagerPlan,
    request: ManagerChatMessage,
    t1: TaskId,
    t2: TaskId,
    a1: AgentId,
    a2: AgentId,
    events: Vec<Event>,
}

fn plan(project: ProjectId, id: ManagerPlanId, status: PlanStatus) -> ManagerPlan {
    ManagerPlan {
        id,
        project_id: project,
        conversation_id: None,
        goal: "Add login".into(),
        summary: "Two tasks".into(),
        assumptions: vec![],
        risks: vec![],
        tasks: vec![],
        estimated_cost_usd: None,
        max_parallelism: 2,
        parallelism: 2,
        status,
        source: PlanSource::Structured,
        model: None,
        warnings: vec![],
        options: ManagerRequestOptions::default(),
        run_id: None,
        rejection_reason: None,
        created_at: ts(1_000),
        updated_at: ts(1_000),
    }
}

fn run_task(id: TaskId, title: &str, status: TaskStatus, agent: Option<AgentId>) -> RunTask {
    RunTask {
        task_id: id,
        key: title.to_lowercase(),
        title: title.into(),
        role: AgentRole::Backend,
        status,
        agent_id: agent,
        attempts: 1,
        branch: Some(format!("agent/{title}")),
        worktree_path: None,
        cost_usd: 0.0,
        files_changed: vec![],
        last_error: None,
        awaiting_review: false,
        summary: None,
        pipeline: None,
    }
}

fn run(
    project: ProjectId,
    id: ManagerRunId,
    plan: ManagerPlanId,
    status: ManagerRunStatus,
    tasks: Vec<RunTask>,
    report: Option<RunReport>,
) -> ManagerRun {
    ManagerRun {
        id,
        project_id: project,
        plan_id: plan,
        goal: "Add login".into(),
        status,
        status_reason: None,
        limits: RunLimits {
            max_parallel_agents: 2,
            max_agents: 10,
            max_cost_usd: None,
            time_limit_mins: None,
            max_consecutive_failures: 3,
            max_attempts_per_task: 2,
        },
        approval_policy: ApprovalMode::SemiAuto,
        tasks,
        agents_spawned: 2,
        peak_parallel: 2,
        consecutive_failures: 0,
        cost_usd: 0.12,
        active_secs: 10,
        started_at: ts(2_000),
        updated_at: ts(2_000),
        finished_at: if status.is_final() { Some(ts(20_000)) } else { None },
        report,
    }
}

/// Builds a synthetic run: request → plan → 2 tasks → 2 agents → tools,
/// files, approval, tests, review → completion → merge proposals, plus noise
/// from an unrelated project/agent.
async fn fixture() -> (Fixture, EventBus) {
    let bus = EventBus::with_log(Arc::new(MemoryEventLog::new()), 64).await.unwrap();
    let project = ProjectId::new();
    let other_project = ProjectId::new();
    let plan_id = ManagerPlanId::new();
    let run_id = ManagerRunId::new();
    let (t1, t2) = (TaskId::new(), TaskId::new());
    let (a1, a2, stranger) = (AgentId::new(), AgentId::new(), AgentId::new());
    let corr = CorrelationId::new();
    let request = ManagerChatMessage {
        id: "m1".into(),
        project_id: project,
        conversation_id: orkestr_domain::ConversationId::new(),
        role: ManagerMessageRole::User,
        text: "Please add a login page".into(),
        tool_events: vec![],
        plan_id: None,
        run_id: None,
        model: None,
        usage: None,
        created_at: ts(500),
    };
    let tasks_running =
        vec![run_task(t1, "Backend", TaskStatus::Running, Some(a1)), run_task(t2, "Frontend", TaskStatus::Ready, None)];
    let report = RunReport {
        summary: "done".into(),
        pipelines: vec![],
        completed: 2,
        failed: 0,
        not_done: 0,
        files_changed: vec!["src/login.rs".into()],
        tests: vec![],
        open_issues: vec![],
        cost_usd: 0.12,
        duration_secs: 18,
        merge_proposals: vec![MergeProposal {
            task_id: t1,
            title: "Backend".into(),
            branch: "agent/backend".into(),
            files_changed: vec!["src/login.rs".into()],
            additions: Some(10),
            deletions: Some(0),
            note: String::new(),
        }],
    };
    let final_tasks = vec![
        run_task(t1, "Backend", TaskStatus::Completed, Some(a1)),
        run_task(t2, "Frontend", TaskStatus::Completed, Some(a2)),
    ];
    let mut t1_task = Task::new(project, "Backend", ts(1_500));
    t1_task.id = t1;
    let mut t2_task = Task::new(project, "Frontend", ts(1_500));
    t2_task.id = t2;
    let tc1 = ToolCallId::new();
    let tc2 = ToolCallId::new();
    let ap = ApprovalId::new();

    let mut evs: Vec<(i64, NewEvent)> = vec![
        (1_000, NewEvent::new(EventKind::ManagerPlanUpdated(ManagerPlanPayload { plan: plan(project, plan_id, PlanStatus::Proposed) }))),
        (1_100, NewEvent::new(EventKind::ManagerPlanUpdated(ManagerPlanPayload { plan: plan(project, plan_id, PlanStatus::Proposed) }))),
        (1_400, NewEvent::new(EventKind::ManagerPlanUpdated(ManagerPlanPayload { plan: plan(project, plan_id, PlanStatus::Approved) }))),
        (1_500, NewEvent::new(EventKind::TaskCreated(TaskPayload { task: t1_task }))),
        (1_500, NewEvent::new(EventKind::TaskCreated(TaskPayload { task: t2_task }))),
        (2_000, NewEvent::new(EventKind::ManagerRunUpdated(ManagerRunPayload { run: run(project, run_id, plan_id, ManagerRunStatus::Running, tasks_running.clone(), None) }))),
        (2_100, NewEvent::new(EventKind::ManagerRunUpdated(ManagerRunPayload { run: run(project, run_id, plan_id, ManagerRunStatus::Running, tasks_running, None) }))),
        (2_200, NewEvent::new(EventKind::AgentSpawned(AgentSpawnedPayload { agent: agent(a1, project, t1, "backend-1") }))),
        (2_300, NewEvent::new(EventKind::AgentStarted(AgentStartedPayload { agent_id: a1, task_id: Some(t1) })).project(project)),
        (2_300, NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: t1, from: TaskStatus::Ready, to: TaskStatus::Running, reason: None })).project(project)),
        (2_400, NewEvent::new(EventKind::AgentStateChanged(AgentStateChangedPayload { agent_id: a1, from: AgentState::Idle, to: AgentState::Thinking, activity: None })).project(project)),
        (2_500, NewEvent::new(EventKind::ToolCallStarted(ToolCallStartedPayload { tool_call: tool_call(tc1, a1, t1, "fs.write") })).project(project).correlation(corr)),
        (2_600, NewEvent::new(EventKind::FileChanged(FileChangedPayload { path: "src/login.rs".into(), old_path: None, change: FileChangeKind::Created, actor: FileChangeActor::Agent, agent_id: Some(a1) })).project(project).agent(a1)),
        (2_700, NewEvent::new(EventKind::ToolCallFinished(ToolCallFinishedPayload { tool_call_id: tc1, status: ToolCallStatus::Succeeded, output_summary: Some("wrote 10 lines".into()), duration_ms: 40 })).project(project).agent(a1).task(t1)),
        (2_800, NewEvent::new(EventKind::ApprovalRequested(ApprovalRequestedPayload { approval: approval(ap, project, t1, a1) }))),
        // Correlated with an included tool call but carrying no task/agent ids.
        (2_850, NewEvent::new(EventKind::SnapshotCreated(SnapshotCreatedPayload { snapshot: serde_json::from_value(json!({
            "id": orkestr_domain::SnapshotId::new(), "projectId": project, "workspaceId": orkestr_domain::WorkspaceId::new(), "taskId": null,
            "label": "before tool", "reason": "BeforeRiskyAction", "baseCommit": null, "manifestObjectId": "x", "includesUncommitted": true,
            "fileCount": 1, "sizeBytes": 1, "createdBy": {"type": "System"}, "createdAt": ts(2_850)
        })).unwrap_or_else(|e| panic!("snapshot json: {e}")) })).correlation(corr)),
        (2_900, NewEvent::new(EventKind::ApprovalResolved(ApprovalResolvedPayload { approval_id: ap, status: ApprovalStatus::Approved, resolved_by: None })).project(project)),
        (3_000, NewEvent::new(EventKind::ToolCallStarted(ToolCallStartedPayload { tool_call: tool_call(tc2, a1, t1, "terminal.run") })).project(project)),
        (3_100, NewEvent::new(EventKind::ToolCallFinished(ToolCallFinishedPayload { tool_call_id: tc2, status: ToolCallStatus::Failed, output_summary: Some("exit 1".into()), duration_ms: 900 })).project(project).agent(a1)),
        (3_200, NewEvent::new(EventKind::TestRunFinished(TestRunFinishedPayload { run_id: "r1".into(), passed: 5, failed: 1, skipped: 0, duration_ms: 1200, summary: "5 passed, 1 failed".into() })).project(project).agent(a1).task(t1)),
        (3_300, NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: t1, from: TaskStatus::Running, to: TaskStatus::Review, reason: None })).project(project)),
        (3_400, NewEvent::new(EventKind::ReviewFinished(ReviewFinishedPayload { reviewer_agent_id: None, verdict: ReviewState::Approved, findings: 0, summary: "LGTM".into() })).project(project).task(t1)),
        (3_500, NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: t1, from: TaskStatus::Review, to: TaskStatus::Completed, reason: None })).project(project)),
        (3_600, NewEvent::new(EventKind::AgentStopped(AgentStoppedPayload { agent_id: a1, reason: StopReason::Completed, detail: None })).project(project)),
        (3_700, NewEvent::new(EventKind::AgentSpawned(AgentSpawnedPayload { agent: agent(a2, project, t2, "frontend-1") }))),
        (3_800, NewEvent::new(EventKind::FileChanged(FileChangedPayload { path: "web/login.tsx".into(), old_path: None, change: FileChangeKind::Created, actor: FileChangeActor::Agent, agent_id: Some(a2) })).project(project).agent(a2)),
        (3_900, NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: t2, from: TaskStatus::Running, to: TaskStatus::Completed, reason: None })).project(project)),
        // Noise: another agent in this project, another project, logs.
        (3_950, NewEvent::new(EventKind::FileChanged(FileChangedPayload { path: "unrelated.txt".into(), old_path: None, change: FileChangeKind::Modified, actor: FileChangeActor::User, agent_id: Some(stranger) })).project(project).agent(stranger)),
        (3_960, NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: TaskId::new(), from: TaskStatus::Ready, to: TaskStatus::Running, reason: None })).project(other_project)),
        (3_970, NewEvent::new(EventKind::Log(LogPayload { level: LogLevel::Info, target: "x".into(), message: "noise".into(), fields: serde_json::Value::Null })).correlation(corr)),
        (20_000, NewEvent::new(EventKind::ManagerRunUpdated(ManagerRunPayload { run: run(project, run_id, plan_id, ManagerRunStatus::Completed, final_tasks.clone(), Some(report.clone())) }))),
        // After the window (finished + 30 s): ignored.
        (60_000, NewEvent::new(EventKind::FileChanged(FileChangedPayload { path: "late.rs".into(), old_path: None, change: FileChangeKind::Modified, actor: FileChangeActor::Agent, agent_id: Some(a1) })).project(project).agent(a1)),
    ];
    // Publish with controlled timestamps: publish, then rewrite ts through a
    // second log (the bus stamps `now`), so build a log directly instead.
    let log = Arc::new(MemoryEventLog::new());
    let mut events = Vec::new();
    for (i, (offset, ev)) in evs.drain(..).enumerate() {
        let e = ev.into_event(u64::try_from(i).unwrap() + 1, ts(offset));
        orkestr_events::EventLog::append(log.as_ref(), &e).await.unwrap();
        events.push(e);
    }
    drop(bus);
    let bus = EventBus::with_log(log, 64).await.unwrap();
    let final_run = run(project, run_id, plan_id, ManagerRunStatus::Completed, final_tasks, Some(report));
    (
        Fixture {
            project,
            run: final_run,
            plan: plan(project, plan_id, PlanStatus::Completed),
            request,
            t1,
            t2,
            a1,
            a2,
            events,
        },
        bus,
    )
}

fn seeds(f: &Fixture) -> ReplaySeeds {
    ReplaySeeds {
        project_id: f.project,
        subject: ReplaySubject::Run,
        run: Some(f.run.clone()),
        plan: Some(f.plan.clone()),
        request: Some(f.request.clone()),
        task: None,
    }
}

#[tokio::test]
async fn run_timeline_follows_the_spec_order() {
    let (f, bus) = fixture().await;
    let s = seeds(&f);
    let g = gather_default(&bus, &s).await.unwrap();
    assert!(g.events.iter().all(|e| e.ts <= ts(50_000)), "events after the window are not read");
    assert!(!g.truncated);
    let tl = build(&s, &g);
    let kinds: Vec<&str> = tl.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "UserRequest",
            "PlanProposed",
            "PlanApproved",
            "TaskCreated",
            "TaskCreated",
            "RunUpdated",
            "AgentSpawned",
            "AgentStarted",
            "TaskStatusChanged",
            "ToolCall",
            "FileChanged",
            "ApprovalRequested",
            "SnapshotCreated",
            "ApprovalResolved",
            "ToolCall",
            "TestRunFinished",
            "TaskStatusChanged",
            "ReviewFinished",
            "TaskStatusChanged",
            "AgentStopped",
            "AgentSpawned",
            "FileChanged",
            "TaskStatusChanged",
            "RunUpdated",
            "MergeProposal",
        ],
        "noise (other agents, other projects, logs, Thinking states, duplicate plan/run updates) is excluded"
    );
    // Indexes are dense and phases follow the spec's vocabulary.
    assert!(tl.steps.iter().enumerate().all(|(i, s)| s.index as usize == i));
    assert_eq!(tl.steps[0].title, "Please add a login page");
    assert_eq!(tl.steps[0].phase, ReplayPhase::Request);
    let phases: Vec<ReplayPhase> = tl.phases.iter().map(|p| p.phase).collect();
    assert_eq!(
        phases,
        vec![
            ReplayPhase::Request,
            ReplayPhase::Plan,
            ReplayPhase::Agents,
            ReplayPhase::Delegation,
            ReplayPhase::Tools,
            ReplayPhase::Files,
            ReplayPhase::Tests,
            ReplayPhase::Reviews,
            ReplayPhase::Approvals,
            ReplayPhase::Merge,
            ReplayPhase::Run,
        ]
    );
    // Tool calls are merged with their finish event.
    let tool = tl.steps.iter().find(|s| s.kind == "ToolCall").unwrap();
    assert_eq!(tool.title, "fs.write");
    assert_eq!(tool.tool_status, Some(ToolCallStatus::Succeeded));
    assert_eq!(tool.duration_ms, Some(40));
    assert_eq!(tool.detail.as_deref(), Some("wrote 10 lines"));
    let failed = tl.steps.iter().filter(|s| s.kind == "ToolCall").nth(1).unwrap();
    assert_eq!(failed.status, ReplayStepStatus::Error);
    // Approval resolution carries the request's title.
    let resolved = tl.steps.iter().find(|s| s.kind == "ApprovalResolved").unwrap();
    assert_eq!(resolved.title, "run npm install");
    assert_eq!(resolved.status, ReplayStepStatus::Success);
    // Correlation pulled in the snapshot.
    assert!(tl.steps.iter().any(|s| s.kind == "SnapshotCreated"));
    // Merge proposal step.
    let merge = tl.steps.last().unwrap();
    assert_eq!(merge.phase, ReplayPhase::Merge);
    assert_eq!(merge.detail.as_deref(), Some("agent/backend"));
    // Agents with names discovered from the log.
    assert_eq!(tl.agents.len(), 2);
    assert_eq!(tl.agents.iter().find(|a| a.agent_id == f.a1).unwrap().name, "backend-1");
    assert_eq!(tl.tasks.len(), 2);
    assert_eq!(tl.total_steps, 25);
    assert!(tl.events_scanned >= 25);
    let _ = (f.t2, f.events.len());
}

#[tokio::test]
async fn agent_filter_and_phase_filter() {
    let (f, bus) = fixture().await;
    let s = seeds(&f);
    let g = gather_default(&bus, &s).await.unwrap();
    let full = build(&s, &g);
    let a2_only = filter(full.clone(), Some(f.a2), None);
    let kinds: Vec<&str> = a2_only.steps.iter().map(|s| s.kind.as_str()).collect();
    // Agent-less steps of the agent's task are kept (task status), others dropped.
    assert_eq!(kinds, vec!["TaskCreated", "AgentSpawned", "FileChanged", "TaskStatusChanged"]);
    assert_eq!(a2_only.total_steps, full.total_steps, "total counts the unfiltered timeline");
    assert!(a2_only.steps.iter().enumerate().all(|(i, s)| s.index as usize == i));

    let tools = filter(full, None, Some(&[ReplayPhase::Tools, ReplayPhase::Tests]));
    assert!(tools.steps.iter().all(|s| matches!(s.phase, ReplayPhase::Tools | ReplayPhase::Tests)));
    assert_eq!(tools.phases.len(), 2);
}

#[tokio::test]
async fn step_through_reconstructs_state() {
    let (f, bus) = fixture().await;
    let s = seeds(&f);
    let tl = build(&s, &gather_default(&bus, &s).await.unwrap());
    let first = step(&tl, 0, ReplayDirection::First, None);
    assert_eq!(first.index, 0);
    assert!(!first.has_prev && first.has_next);
    assert!(first.state.tasks.iter().all(|t| t.status.is_none()));

    // Jump to the first Files step.
    let files = step(&tl, 0, ReplayDirection::NextPhase, Some(ReplayPhase::Files));
    let st = &files.state;
    assert_eq!(files.step.as_ref().unwrap().kind, "FileChanged");
    assert_eq!(st.files_changed, vec!["src/login.rs"]);
    assert_eq!(st.tool_calls, 1);
    assert_eq!(st.agents.len(), 1);
    assert!(st.agents[0].active);
    assert_eq!(st.tasks.iter().find(|t| t.task_id == f.t1).unwrap().status, Some(TaskStatus::Running));
    assert_eq!(st.run_status, Some(ManagerRunStatus::Running));

    // Approval pending right after the request, resolved after the resolution.
    let req = tl.steps.iter().find(|s| s.kind == "ApprovalRequested").unwrap().index;
    assert_eq!(step(&tl, req, ReplayDirection::Goto, None).state.approvals_pending, 1);
    let res = step(&tl, req, ReplayDirection::NextPhase, Some(ReplayPhase::Approvals));
    assert_eq!(res.step.as_ref().unwrap().kind, "ApprovalResolved");
    assert_eq!(res.state.approvals_pending, 0);
    assert_eq!(res.state.approvals_resolved, 1);

    let last = step(&tl, 0, ReplayDirection::Last, None);
    assert!(last.has_prev && !last.has_next);
    let st = &last.state;
    assert_eq!(st.files_changed, vec!["src/login.rs", "web/login.tsx"]);
    assert_eq!(st.tool_calls, 2);
    assert_eq!(st.tool_failures, 1);
    assert_eq!((st.tests_passed, st.tests_failed), (5, 1));
    assert_eq!(st.reviews, 1);
    assert_eq!(st.run_status, Some(ManagerRunStatus::Completed));
    assert!(st.tasks.iter().all(|t| t.status == Some(TaskStatus::Completed)));
    let a1 = st.agents.iter().find(|a| a.agent_id == f.a1).unwrap();
    assert!(!a1.active);
    assert_eq!(a1.state, Some(AgentState::Stopped));

    // Prev/Next clamp at the ends; out-of-range cursors clamp too.
    assert_eq!(step(&tl, 0, ReplayDirection::Prev, None).index, 0);
    assert_eq!(step(&tl, 10_000, ReplayDirection::Next, None).index, last.index);
}

#[tokio::test]
async fn task_subject_and_empty_logs() {
    let (f, bus) = fixture().await;
    let mut task = Task::new(f.project, "Backend", ts(1_500));
    task.id = f.t1;
    task.status = TaskStatus::Completed;
    task.updated_at = ts(3_500);
    let s = ReplaySeeds {
        project_id: f.project,
        subject: ReplaySubject::Task,
        run: None,
        plan: None,
        request: None,
        task: Some(task),
    };
    let tl = build(&s, &gather_default(&bus, &s).await.unwrap());
    assert_eq!(tl.subject, ReplaySubject::Task);
    assert_eq!(tl.steps.first().unwrap().kind, "TaskCreated");
    assert!(tl.steps.iter().all(|s| s.task_id == Some(f.t1)
        || s.agent_id == Some(f.a1)
        || s.kind == "SnapshotCreated"
        || s.kind == "ApprovalResolved"));
    assert!(tl.steps.iter().any(|s| s.kind == "ReviewFinished"));
    assert!(!tl.steps.iter().any(|s| s.title == "web/login.tsx"));

    let empty = EventBus::with_log(Arc::new(MemoryEventLog::new()), 8).await.unwrap();
    let g = gather_default(&empty, &s).await.unwrap();
    assert!(g.events.is_empty());
    let tl = build(&s, &g);
    assert!(tl.steps.is_empty());
    let r = step(&tl, 3, ReplayDirection::Next, None);
    assert!(r.step.is_none() && r.total == 0);
}

#[tokio::test]
async fn gather_respects_scan_cap() {
    let (f, bus) = fixture().await;
    let s = seeds(&f);
    let (from, to) = s.window();
    let g = gather(&bus, from, to, 5).await.unwrap();
    assert!(g.truncated);
    assert_eq!(g.scanned, 5);
    assert!(build(&s, &g).truncated);
}

#[test]
fn request_lookup_picks_latest_user_message_before_plan() {
    let p = plan(ProjectId::new(), ManagerPlanId::new(), PlanStatus::Proposed);
    let mk = |text: &str, role: ManagerMessageRole, at: i64| ManagerChatMessage {
        id: text.into(),
        project_id: p.project_id,
        conversation_id: orkestr_domain::ConversationId::new(),
        role,
        text: text.into(),
        tool_events: vec![],
        plan_id: None,
        run_id: None,
        model: None,
        usage: None,
        created_at: ts(at),
    };
    let msgs = vec![
        mk("old", ManagerMessageRole::User, 100),
        mk("reply", ManagerMessageRole::Manager, 200),
        mk("this one", ManagerMessageRole::User, 900),
        mk("later", ManagerMessageRole::User, 5_000),
    ];
    assert_eq!(request_for_plan(&p, &msgs).unwrap().text, "this one");
}

fn proposal(
    project: ProjectId,
    id: orkestr_domain::MergeProposalId,
    run_id: Option<ManagerRunId>,
    status: orkestr_protocol::methods::merge::MergeProposalStatus,
) -> orkestr_protocol::methods::merge::MergeManagerProposal {
    orkestr_protocol::methods::merge::MergeManagerProposal {
        id,
        project_id: project,
        run_id,
        target: "main".into(),
        target_commit: "abc".into(),
        order: vec![],
        branches: vec![],
        conflicts: vec![],
        blockers: vec![],
        recommendation: orkestr_protocol::methods::merge::MergeRecommendation::default(),
        status,
        status_detail: None,
        policy: orkestr_protocol::methods::merge::MergePolicy::default(),
        auto_mergeable: false,
        execution: None,
        created_at: ts(0),
        updated_at: ts(0),
    }
}

#[tokio::test]
async fn merge_proposals_of_the_run_become_one_step_per_status() {
    use orkestr_protocol::methods::merge::MergeProposalStatus as S;
    let (f, _) = fixture().await;
    let log = Arc::new(MemoryEventLog::new());
    // Fixture events up to the run's end, then the merge events, then the rest
    // (appended in timestamp order with fresh seqs).
    let (before, after): (Vec<Event>, Vec<Event>) = f.events.iter().cloned().partition(|e| e.ts <= ts(20_000));
    let mut seq = 0u64;
    for mut e in before {
        seq += 1;
        e.seq = seq;
        orkestr_events::EventLog::append(log.as_ref(), &e).await.unwrap();
    }
    let mine = orkestr_domain::MergeProposalId::new();
    let other = orkestr_domain::MergeProposalId::new();
    let extra = [
        (21_000, proposal(f.project, mine, Some(f.run.id), S::Proposed)),
        (21_100, proposal(f.project, mine, Some(f.run.id), S::Proposed)),
        (21_200, proposal(f.project, other, Some(ManagerRunId::new()), S::Completed)),
        (21_300, proposal(f.project, mine, Some(f.run.id), S::Completed)),
    ];
    for (offset, p) in extra {
        seq += 1;
        let e = NewEvent::new(EventKind::MergeProposalUpdated(MergeProposalPayload { proposal: p }))
            .into_event(seq, ts(offset));
        orkestr_events::EventLog::append(log.as_ref(), &e).await.unwrap();
    }
    for mut e in after {
        seq += 1;
        e.seq = seq;
        orkestr_events::EventLog::append(log.as_ref(), &e).await.unwrap();
    }
    let bus = EventBus::with_log(log, 64).await.unwrap();
    let s = seeds(&f);
    let g = gather_default(&bus, &s).await.unwrap();
    let t = build(&s, &g);
    let merges: Vec<_> =
        t.steps.iter().filter(|s| s.kind == "MergeProposalUpdated").map(|s| (s.status, s.detail.clone())).collect();
    assert_eq!(
        merges,
        vec![(ReplayStepStatus::Info, Some("Proposed".into())), (ReplayStepStatus::Success, Some("Completed".into()))],
        "one step per status change of this run's proposal; other runs' proposals left out"
    );
    assert!(t.steps.iter().filter(|s| s.kind == "MergeProposalUpdated").all(|s| s.phase == ReplayPhase::Merge));
}
