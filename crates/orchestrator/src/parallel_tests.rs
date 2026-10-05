//! Parallel execution hardening tests (§11, §12, §62.5, §68.24–26) over the
//! real engines of the Manager test harness and the offline Mock provider:
//! fairness across projects, admission explanations, provider rate-limit
//! back-off, the API concurrency gate, restart recovery (auto-resume or wait
//! for the user) and concurrent worktree creation by many agents.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_agent_runtime::ModelGateway;
use orkestr_domain::{
    now, AgentRole, ApprovalMode, ManagerRunId, ModelRef, Project, ProjectId, ProviderId, TaskActor, TaskId,
    TaskPriority, TaskStatus, WorktreePolicy,
};
use orkestr_protocol::event::{EventKind, ProviderRateLimitedPayload};
use orkestr_protocol::methods::agents::{NewTaskSpec, SpawnAgentParams};
use orkestr_protocol::methods::manager::{ApprovePlanParams, ManagerRunStatus};
use orkestr_protocol::methods::scheduler::{
    AutoResumePolicy, ProviderCap, SchedEntryState, SchedulerSettings, WaitReason,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::mock::{MockAdapter, MockOptions};
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, ChatStream, ProviderAdapter};
use orkestr_storage::ProjectRepository;
use orkestr_task_engine::SchedTask;
use tokio_util::sync::CancellationToken;

use super::{final_status, harness, manager_deps, ptask, H};
use crate::parallel::{Candidate, GatedGateway};
use crate::Manager;

fn cand(project: ProjectId, title: &str) -> Candidate {
    Candidate { sched: SchedTask::new(TaskId::new(), project, TaskPriority::Normal, now()), title: title.into() }
}

fn active(project: ProjectId) -> SchedTask {
    SchedTask::new(TaskId::new(), project, TaskPriority::Normal, now())
}

async fn set(h: &H, f: impl FnOnce(&mut SchedulerSettings)) {
    let mut s = h.mgr.parallel().settings();
    f(&mut s);
    h.mgr.parallel().set_settings(s).await.unwrap();
}

async fn second_project(h: &H) -> ProjectId {
    let mut p = Project::new("Other", now());
    p.root_path = Some(h.dir.path().join("other").to_string_lossy().into_owned());
    std::fs::create_dir_all(h.dir.path().join("other")).unwrap();
    h.meta.insert(&p).await.unwrap();
    p.id
}

// ---------------------------------------------------------------------------
// admission: caps, fairness, explanations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_slots_are_shared_fairly_between_projects_and_explained() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let other = second_project(&h).await;
    set(&h, |s| {
        s.max_parallel_global = 4;
        s.max_parallel_per_project = 4;
    })
    .await;
    let sched = h.mgr.parallel();
    let (run_a, run_b) = (ManagerRunId::new(), ManagerRunId::new());
    // Project A already runs three agents; both projects queue three tasks.
    let running: Vec<SchedTask> = (0..3).map(|_| active(h.project)).collect();
    let a: Vec<Candidate> = (0..3).map(|i| cand(h.project, &format!("a{i}"))).collect();
    let b: Vec<Candidate> = (0..3).map(|i| cand(other, &format!("b{i}"))).collect();

    let out_a = sched.admit(run_a, h.project, 4, a.clone(), &running);
    assert_eq!(out_a.start.len(), 1, "alone, A gets the last free slot");
    // B registers: the free slot goes to the least-loaded project (B), not to
    // A's older queue — and A's queued tasks are told why they wait.
    let out_b = sched.admit(run_b, other, 4, b.clone(), &running);
    assert_eq!(out_b.start, vec![b[0].sched.task_id], "fair share: B gets the free slot");
    let out_a = sched.admit(run_a, h.project, 4, a.clone(), &running);
    assert!(out_a.start.is_empty(), "A must not take B's slot");
    assert_eq!(out_a.deferred.len(), 3);
    assert!(out_a.deferred.iter().all(|(_, d)| d.to_string() == "waiting: global parallel limit 4/4"), "{out_a:?}");

    // B's slot is taken (now active): everything else waits on the global cap.
    let mut running2 = running.clone();
    let mut started = b[0].sched.clone();
    started.task_id = out_b.start[0];
    running2.push(started);
    let out_b = sched.admit(run_b, other, 4, b[1..].to_vec(), &running2);
    assert!(out_b.start.is_empty());
    assert_eq!(
        crate::parallel::wait_reason(&out_b.deferred[0].1),
        WaitReason::GlobalLimit { running: 4, limit: 4 }
    );

    // Status lists the queue of every project with reasons and positions.
    let status = sched.status(None).await.unwrap();
    let waiting: Vec<_> = status.queue.iter().filter(|e| e.state == SchedEntryState::Waiting).collect();
    assert_eq!(waiting.len(), 5, "{:#?}", status.queue);
    assert!(waiting.iter().all(|e| e.reason_text.as_deref().is_some_and(|t| t.starts_with("waiting: global parallel limit"))));
    assert!(waiting.iter().all(|e| e.position.is_some()));
    assert_eq!(status.counts.waiting, 5);
    let only_b = sched.status(Some(other)).await.unwrap();
    assert!(only_b.queue.iter().all(|e| e.project_id == other));
    assert_eq!(only_b.queue.len(), 2);

    // A user pause holds a run: its tasks wait with the run-level reason and
    // stop competing for slots.
    sched.hold(run_a, h.project, 4, a.clone(), WaitReason::Paused);
    let out_b = sched.admit(run_b, other, 4, b[1..].to_vec(), &running);
    assert_eq!(out_b.start.len(), 1, "A is paused: B may use the free slot");
    let status = sched.status(Some(h.project)).await.unwrap();
    assert!(status.queue.iter().all(|e| e.reason == Some(WaitReason::Paused)));

    // Unregistering a run removes its queue.
    sched.unregister(run_a);
    sched.unregister(run_b);
    assert!(sched.status(None).await.unwrap().queue.is_empty());
}

#[tokio::test]
async fn per_project_team_and_provider_caps_and_file_conflicts() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    set(&h, |s| {
        s.max_parallel_per_project = 2;
        s.team_caps = vec![orkestr_protocol::methods::scheduler::TeamCap { team: "web".into(), max: 1 }];
        s.provider_caps = vec![ProviderCap { provider_id: "p1".into(), max: 1 }];
    })
    .await;
    let sched = h.mgr.parallel();
    let run = ManagerRunId::new();
    let mut c: Vec<Candidate> = (0..4).map(|i| cand(h.project, &format!("t{i}"))).collect();
    c[0].sched.team = Some("web".into());
    c[1].sched.team = Some("web".into());
    c[2].sched.provider = Some("p1".into());
    c[3].sched.provider = Some("p1".into());
    let out = sched.admit(run, h.project, 8, c.clone(), &[]);
    assert_eq!(out.start.len(), 2, "per-project cap 2: {out:?}");
    let reasons: Vec<String> = out.deferred.iter().map(|(_, d)| d.to_string()).collect();
    assert!(reasons.iter().any(|r| r.contains("team web")), "{reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("project parallel limit 2/2")), "{reasons:?}");

    // Same file in the same checkout: never two agents at once.
    sched.unregister(run);
    let run2 = ManagerRunId::new();
    let mut x = cand(h.project, "x");
    x.sched.files = vec!["src/lib.rs".into()];
    let mut busy = active(h.project);
    busy.files = vec!["src/lib.rs".into()];
    let out = sched.admit(run2, h.project, 8, vec![x.clone()], &[busy.clone()]);
    assert!(out.start.is_empty());
    assert_eq!(
        crate::parallel::wait_reason(&out.deferred[0].1),
        WaitReason::FileConflict { with_task: busy.task_id, path: "src/lib.rs".into() }
    );
    // Separate worktrees may edit the same file in parallel.
    x.sched.worktree = Some("wt-x".into());
    busy.worktree = Some("wt-busy".into());
    let out = sched.admit(run2, h.project, 8, vec![x], &[busy]);
    assert_eq!(out.start.len(), 1);
}

#[tokio::test]
async fn explain_reports_every_check_for_a_task() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    set(&h, |s| s.max_parallel_per_project = 1).await;
    let task = h
        .tasks
        .create(
            CreateTaskParams { project_id: h.project, title: "Queued task".into(), ..CreateTaskParams::default() },
            TaskActor::User,
        )
        .await
        .unwrap();
    let ready = h.tasks.transition(task.id, TaskStatus::Ready, TaskActor::User, None, None).await.unwrap();
    assert_eq!(ready.task.status, TaskStatus::Ready);
    // Not scheduled by any run yet.
    let e = h.mgr.parallel().explain(task.id).await.unwrap();
    assert!(!e.scheduled);
    assert_eq!(e.checks.len(), 11);
    assert!(e.summary.starts_with("ready, but no live Manager run"), "{}", e.summary);
    // Another task of the project runs (fills the project cap of 1).
    let busy = h
        .tasks
        .create(
            CreateTaskParams { project_id: h.project, title: "Busy task".into(), ..CreateTaskParams::default() },
            TaskActor::User,
        )
        .await
        .unwrap();
    h.tasks.transition(busy.id, TaskStatus::Ready, TaskActor::User, None, None).await.unwrap();
    let busy = h.tasks.transition(busy.id, TaskStatus::Running, TaskActor::User, None, None).await.unwrap().task;
    // Scheduled by a run and held back by the project cap.
    let run = ManagerRunId::new();
    let mut c = cand(h.project, "Queued task");
    c.sched.task_id = task.id;
    let out = h.mgr.parallel().admit(run, h.project, 4, vec![c], &[SchedTask::from_task(&busy)]);
    assert!(out.start.is_empty());
    let e = h.mgr.parallel().explain(task.id).await.unwrap();
    assert!(e.scheduled);
    assert_eq!(e.summary, "waiting: project parallel limit 1/1");
    let failing: Vec<&str> = e.checks.iter().filter(|c| !c.ok).map(|c| c.check.as_str()).collect();
    assert_eq!(failing, vec!["project"]);
    assert_eq!(e.entry.unwrap().state, SchedEntryState::Waiting);
    // Settings validation and persistence.
    let bad = SchedulerSettings { max_parallel_global: 0, ..SchedulerSettings::default() };
    assert!(h.mgr.parallel().set_settings(bad).await.is_err());
    let restarted = Manager::new(manager_deps(&h, Arc::clone(&h.gateway) as Arc<dyn ModelGateway>));
    restarted.init().await.unwrap();
    assert_eq!(restarted.parallel().settings().max_parallel_per_project, 1, "settings survive a restart");
}

// ---------------------------------------------------------------------------
// rate limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rate_limited_providers_back_off_with_growing_delays() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    set(&h, |s| s.rate_limit_backoff_secs = 10).await;
    let sched = h.mgr.parallel();
    let secs = |p: &str| {
        let until = sched.rate_limited()[p];
        (until - now()).num_seconds()
    };
    sched.note_rate_limited("p", None);
    assert!((8..=10).contains(&secs("p")), "first back-off = base: {}", secs("p"));
    sched.note_rate_limited("p", None);
    assert!((18..=20).contains(&secs("p")), "doubles: {}", secs("p"));
    sched.note_rate_limited("p", None);
    assert!((38..=40).contains(&secs("p")), "doubles again: {}", secs("p"));
    for _ in 0..20 {
        sched.note_rate_limited("p", None);
    }
    assert!(secs("p") <= 600, "capped at 10 min");
    // A provider-given retry-after wins (but never shortens a running back-off).
    sched.note_rate_limited("q", Some(3));
    assert!((1..=3).contains(&secs("q")));

    // Tasks routed to a backing-off provider wait; others are admitted.
    let run = ManagerRunId::new();
    let mut c1 = cand(h.project, "uses p");
    c1.sched.provider = Some("p".into());
    let c2 = cand(h.project, "other provider");
    let out = sched.admit(run, h.project, 4, vec![c1.clone(), c2.clone()], &[]);
    assert_eq!(out.start, vec![c2.sched.task_id]);
    assert!(matches!(
        crate::parallel::wait_reason(&out.deferred[0].1),
        WaitReason::RateLimited { ref provider_id, .. } if provider_id == "p"
    ));
    sched.clear_rate_limit("p");
    let out = sched.admit(run, h.project, 4, vec![c1], &[]);
    assert_eq!(out.start.len(), 1, "back-off over → admitted");

    // `ProviderRateLimited` events from the provider layer feed the back-off.
    let pid = ProviderId::new();
    h.events
        .publish(NewEvent::new(EventKind::ProviderRateLimited(ProviderRateLimitedPayload {
            provider_id: pid,
            model: Some("m".into()),
            retry_after_secs: Some(42),
        })))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !sched.rate_limited().contains_key(&pid.to_string()) {
        assert!(tokio::time::Instant::now() < deadline, "listener did not record the event");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let st = sched.status(None).await.unwrap();
    let load = st.providers.iter().find(|p| p.provider_id == pid.to_string()).unwrap();
    assert!(load.rate_limited_until.is_some());
    assert_eq!(load.rate_limit_hits, 1);
}

// ---------------------------------------------------------------------------
// API concurrency gate
// ---------------------------------------------------------------------------

/// Mock that counts concurrent requests (held for `hold`).
struct Counting {
    inner: MockAdapter,
    now: AtomicUsize,
    max: AtomicUsize,
    hold: Duration,
}

#[async_trait]
impl ProviderAdapter for Counting {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn descriptor(&self) -> orkestr_providers::ProviderDescriptor {
        self.inner.descriptor()
    }
    async fn auth_state(&self) -> orkestr_providers::AuthStatus {
        self.inner.auth_state().await
    }
    async fn list_models(&self) -> Result<Vec<orkestr_domain::Model>, orkestr_providers::ProviderError> {
        self.inner.list_models().await
    }
    async fn health(&self) -> orkestr_providers::HealthReport {
        self.inner.health().await
    }
    async fn chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, orkestr_providers::ProviderError> {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        tokio::time::sleep(self.hold).await;
        self.now.fetch_sub(1, Ordering::SeqCst);
        self.inner.chat(request, cancel).await
    }
}

fn request(text: &str) -> ChatRequest {
    ChatRequest::new("mock-smart", vec![ChatMessage::user(text)])
}

#[tokio::test]
async fn api_concurrency_gate_limits_in_flight_requests_per_provider() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let pid = ProviderId::new();
    let counting = Arc::new(Counting {
        inner: MockAdapter::new(pid, "Mock", MockOptions { chunk_delay: Duration::ZERO }),
        now: AtomicUsize::new(0),
        max: AtomicUsize::new(0),
        hold: Duration::from_millis(120),
    });
    h.source.adapters.lock().insert(pid, Arc::clone(&counting) as Arc<dyn ProviderAdapter>);
    set(&h, |s| s.provider_caps = vec![ProviderCap { provider_id: pid.to_string(), max: 2 }]).await;
    let gated = Arc::new(GatedGateway::new(Arc::clone(&h.gateway) as Arc<dyn ModelGateway>, Arc::clone(h.mgr.parallel())));
    let model = ModelRef { provider_id: pid, model: "mock-smart".into() };
    let mut handles = Vec::new();
    for i in 0..6 {
        let gated = Arc::clone(&gated);
        let model = model.clone();
        handles.push(tokio::spawn(async move {
            let events: Vec<ChatEvent> =
                gated.chat(vec![model], request(&format!("hello {i}")), None, CancellationToken::new()).collect().await;
            events.iter().any(|e| matches!(e, ChatEvent::Done { .. }))
        }));
    }
    // While requests are in flight the status shows the gate.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let st = h.mgr.parallel().status(None).await.unwrap();
    let load = st.providers.iter().find(|p| p.provider_id == pid.to_string()).unwrap();
    assert_eq!(load.in_flight, 2, "{load:?}");
    assert_eq!(load.queued_requests, 4, "{load:?}");
    for hd in handles {
        assert!(hd.await.unwrap(), "every request completes");
    }
    assert_eq!(counting.max.load(Ordering::SeqCst), 2, "never more than 2 requests at once, and 2 in parallel");
    let st = h.mgr.parallel().status(None).await.unwrap();
    let load = st.providers.iter().find(|p| p.provider_id == pid.to_string()).unwrap();
    assert_eq!((load.in_flight, load.queued_requests), (0, 0), "permits released");

    // A cancelled waiter gives up its place without leaking a permit.
    let slow = gated.chat(vec![model.clone()], request("a"), None, CancellationToken::new());
    let slow2 = gated.chat(vec![model.clone()], request("b"), None, CancellationToken::new());
    let cancel = CancellationToken::new();
    let waiter = gated.chat(vec![model.clone()], request("c"), None, cancel.clone());
    let t = tokio::spawn(async move { slow.collect::<Vec<_>>().await });
    let t2 = tokio::spawn(async move { slow2.collect::<Vec<_>>().await });
    cancel.cancel();
    let out: Vec<ChatEvent> = waiter.collect().await;
    assert!(matches!(out.as_slice(), [ChatEvent::Error(orkestr_providers::ProviderError::Cancelled)] | [..]));
    t.await.unwrap();
    t2.await.unwrap();

    // Rate-limit errors seen through the gate start a back-off.
    let rl = ModelRef { provider_id: pid, model: "mock-rate-limit".into() };
    let _: Vec<ChatEvent> = gated.chat(vec![rl], request("x"), None, CancellationToken::new()).collect().await;
    assert!(h.mgr.parallel().rate_limited().contains_key(&pid.to_string()), "rate limit recorded");
}

// ---------------------------------------------------------------------------
// background agents + restart recovery
// ---------------------------------------------------------------------------

/// Agent model that does not answer while `hold` is set, until cancelled (an
/// agent "in flight" when the core stops); answers like the Mock afterwards.
struct Hanging {
    inner: MockAdapter,
    hold: Arc<AtomicBool>,
}

#[async_trait]
impl ProviderAdapter for Hanging {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn descriptor(&self) -> orkestr_providers::ProviderDescriptor {
        self.inner.descriptor()
    }
    async fn auth_state(&self) -> orkestr_providers::AuthStatus {
        self.inner.auth_state().await
    }
    async fn list_models(&self) -> Result<Vec<orkestr_domain::Model>, orkestr_providers::ProviderError> {
        self.inner.list_models().await
    }
    async fn health(&self) -> orkestr_providers::HealthReport {
        self.inner.health().await
    }
    async fn chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, orkestr_providers::ProviderError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        while self.hold.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::select! {
                () = cancel.cancelled() => return Err(orkestr_providers::ProviderError::Cancelled),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        self.inner.chat(request, cancel).await
    }
}

/// Routes every model call to a [`Hanging`] provider; clear the returned
/// flag to let it answer.
fn hang(h: &H) -> Arc<AtomicBool> {
    let pid = ProviderId::new();
    let hold = Arc::new(AtomicBool::new(true));
    h.source.adapters.lock().insert(
        pid,
        Arc::new(Hanging {
            inner: MockAdapter::new(pid, "Mock", MockOptions { chunk_delay: Duration::ZERO }),
            hold: Arc::clone(&hold),
        }),
    );
    let m = ModelRef { provider_id: pid, model: "mock-smart".into() };
    *h.gateway.chain.lock() = vec![m.clone()];
    h.hub.cfg.lock().model = Some(m);
    hold
}

/// Simulates a core crash/restart: stops runs and agents, marks in-flight
/// tasks `Interrupted` (crash-recovery hook) and builds a fresh Manager on the
/// same database.
async fn restart(h: &H) -> Manager {
    h.mgr.shutdown().await;
    h.rt.shutdown().await;
    h.tasks.recover_interrupted("Orkestr restarted").await.unwrap();
    let m = Manager::new(manager_deps(h, Arc::clone(&h.gateway) as Arc<dyn ModelGateway>));
    m.init().await.unwrap();
    m
}

async fn interrupted_run(h: &H) -> ManagerRunId {
    let _hold = hang(h);
    let plan = h
        .custom_plan(vec![
            ptask("a", "Implement the alpha module", AgentRole::Backend, &[]),
            ptask("b", "Implement the beta module", AgentRole::Backend, &[]),
        ])
        .await;
    let r = h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    h.mgr
        .wait_run(run.id, Duration::from_secs(30), |r| r.tasks.iter().all(|t| t.agent_id.is_some()))
        .await
        .unwrap();
    run.id
}

#[tokio::test]
async fn interrupted_runs_resume_automatically_after_a_restart() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    let run_id = interrupted_run(&h).await;
    let m = restart(&h).await;
    let before = m.get_run(run_id).await.unwrap();
    assert_eq!(before.status, ManagerRunStatus::Interrupted);
    for t in &before.tasks {
        assert_eq!(h.tasks.get(t.task_id).await.unwrap().status, TaskStatus::Interrupted, "{}", t.title);
    }
    h.mock("mock-smart");
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.resumed_runs, vec![run_id], "{report:?}");
    assert!(report.waiting_runs.is_empty());
    let done = m
        .wait_run(run_id, Duration::from_secs(120), |r| {
            final_status(r.status) || r.tasks.iter().all(|t| t.status == TaskStatus::Completed || t.awaiting_review)
        })
        .await
        .unwrap();
    // Each task was finished by its resumed agent (accepted, or waiting for
    // review when the mock's checks are partial on this machine).
    assert!(
        done.tasks.iter().all(|t| t.status == TaskStatus::Completed || t.awaiting_review),
        "{:?} {:#?}",
        done.status_reason,
        done.tasks
    );
    // The tasks went Interrupted → Recovering → … (resume from checkpoint).
    for t in &done.tasks {
        let (hist, _) = h.tasks.history(t.task_id).await.unwrap();
        let path: Vec<TaskStatus> = hist.iter().map(|e| e.to).collect();
        let i = path.iter().position(|s| *s == TaskStatus::Interrupted).expect("was interrupted");
        assert_eq!(path.get(i + 1), Some(&TaskStatus::Recovering), "{path:?}");
    }
    if h.git {
        // With git the new agent continued on the interrupted agent's branch.
        let first = &done.tasks[0];
        let msgs = m.history(orkestr_protocol::methods::manager::ManagerHistoryParams { project_id: h.project, limit: None }).await.unwrap();
        assert!(
            msgs.messages.iter().any(|x| x.text.contains("resumed automatically")),
            "a note in the Manager conversation"
        );
        assert!(first.attempts >= 2, "{first:?}");
    }
}

#[tokio::test]
async fn interrupted_runs_wait_for_the_user_when_the_policy_says_so() {
    // Policy Off.
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let run_id = interrupted_run(&h).await;
    set(&h, |s| s.auto_resume = AutoResumePolicy::Off).await;
    let m = restart(&h).await;
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.waiting_runs, vec![run_id]);
    assert!(report.resumed_runs.is_empty());
    assert_eq!(m.get_run(run_id).await.unwrap().status, ManagerRunStatus::Interrupted);
    let hist = m
        .history(orkestr_protocol::methods::manager::ManagerHistoryParams { project_id: h.project, limit: None })
        .await
        .unwrap();
    assert!(hist.messages.iter().any(|x| x.text.contains("waits for you")), "the user is told");

    // Ask approval policy with FollowApproval → waits too.
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let run_id = interrupted_run(&h).await;
    h.hub.cfg.lock().approval_policy = ApprovalMode::Ask;
    let m = restart(&h).await;
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.waiting_runs, vec![run_id]);

    // A run the user had paused when Orkestr crashed is never resumed
    // automatically (the crash left it `Paused` in the store).
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let run_id = interrupted_run(&h).await;
    set(&h, |s| s.auto_resume = AutoResumePolicy::Always).await;
    h.mgr.shutdown().await;
    h.rt.shutdown().await;
    h.tasks.recover_interrupted("Orkestr restarted").await.unwrap();
    let mut paused = h.mgr.get_run(run_id).await.unwrap();
    paused.status = ManagerRunStatus::Paused;
    h.mgr.store().save_run(&paused).await.unwrap();
    let m = Manager::new(manager_deps(&h, Arc::clone(&h.gateway) as Arc<dyn ModelGateway>));
    m.init().await.unwrap();
    let run = m.get_run(run_id).await.unwrap();
    assert_eq!(run.status, ManagerRunStatus::Interrupted);
    assert_eq!(run.status_reason.as_deref(), Some(crate::parallel::PAUSED_INTERRUPTED_REASON));
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.waiting_runs, vec![run_id], "paused stays paused");

    // Bounded: a task interrupted more often than allowed is left for the user.
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let run_id = interrupted_run(&h).await;
    set(&h, |s| {
        s.auto_resume = AutoResumePolicy::Always;
        s.max_auto_resume_attempts = 0;
    })
    .await;
    let m = restart(&h).await;
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.waiting_runs, vec![run_id], "attempt bound");
}

#[tokio::test]
async fn interrupted_stand_alone_agents_resume_with_a_new_agent() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    let hold = hang(&h);
    let template = h.rt.templates().list(Some(AgentRole::Backend), None).await.unwrap().remove(0);
    let (agent, task) = h
        .rt
        .spawn(SpawnAgentParams {
            project_id: h.project,
            template_id: template.id,
            task_id: None,
            new_task: Some(NewTaskSpec { title: "Implement the alpha module".into(), ..NewTaskSpec::default() }),
            model: None,
            worktree_policy: Some(if h.git { WorktreePolicy::DedicatedWorktree } else { WorktreePolicy::SharedWorktree }),
            name: None,
            max_turns: None,
            base_branch: None,
            approval_mode: None,
            budget: None,
        })
        .await
        .unwrap();
    assert!(h.rt.is_live(agent.id));
    let m = restart(&h).await;
    assert_eq!(h.tasks.get(task.id).await.unwrap().status, TaskStatus::Interrupted);
    // The resumed agent keeps the interrupted agent's model, which answers now.
    hold.store(false, Ordering::SeqCst);
    let report = m.auto_resume_interrupted().await;
    assert_eq!(report.resumed_tasks, vec![task.id], "{report:?}");
    let t = h.tasks.get(task.id).await.unwrap();
    assert_ne!(t.agent_id, Some(agent.id), "a new agent took over");
    let new_agent = t.agent_id.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !h.rt.is_finished(new_agent).await.unwrap() {
        assert!(tokio::time::Instant::now() < deadline, "resumed agent did not finish");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(matches!(
        h.tasks.get(task.id).await.unwrap().status,
        TaskStatus::Completed | TaskStatus::Review | TaskStatus::Testing
    ));
}

// ---------------------------------------------------------------------------
// worktree concurrency safety (§62.5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn many_agents_create_worktrees_concurrently() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    if !h.git {
        eprintln!("git not installed — skipped");
        return;
    }
    h.mock("mock-slow");
    let template = h.rt.templates().list(Some(AgentRole::Backend), None).await.unwrap().remove(0);
    const N: usize = 10;
    let mut handles = Vec::new();
    for i in 0..N {
        let rt = h.rt.clone();
        let project = h.project;
        let template = template.id;
        handles.push(tokio::spawn(async move {
            rt.spawn(SpawnAgentParams {
                project_id: project,
                template_id: template,
                task_id: None,
                new_task: Some(NewTaskSpec { title: format!("Implement module {i}"), ..NewTaskSpec::default() }),
                model: None,
                worktree_policy: Some(WorktreePolicy::DedicatedWorktree),
                name: None,
                max_turns: Some(2),
                base_branch: None,
                approval_mode: None,
                budget: None,
            })
            .await
        }));
    }
    let mut agents = Vec::new();
    for hd in handles {
        let (a, _) = hd.await.unwrap().expect("every spawn succeeds (no git lock races)");
        agents.push(a);
    }
    let paths: std::collections::BTreeSet<String> =
        agents.iter().map(|a| a.worktree.as_ref().expect("dedicated worktree").path.clone()).collect();
    let branches: std::collections::BTreeSet<String> =
        agents.iter().map(|a| a.worktree.as_ref().unwrap().branch.clone()).collect();
    assert_eq!(paths.len(), N, "distinct worktrees");
    assert_eq!(branches.len(), N, "distinct branches");
    let vcs = orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), h.db.clone(), h.dir.path().join("worktrees"));
    let list = vcs.worktrees(h.project, &h.root).await.unwrap();
    assert_eq!(list.len(), N + 1, "main + one per agent");

    // Mixed concurrent mutations on the same repository: stop agents while
    // removing half the worktrees and adding new ones.
    for a in &agents {
        let _ = h.rt.stop(a.id, orkestr_protocol::methods::agents::AgentStopMode::Cancel).await;
    }
    let vcs = Arc::new(vcs);
    let mut ops = Vec::new();
    for a in agents.iter().take(N / 2) {
        let vcs = Arc::clone(&vcs);
        let root = h.root.clone();
        let project = h.project;
        let path = a.worktree.as_ref().unwrap().path.clone();
        ops.push(tokio::spawn(async move { vcs.remove_worktree(project, &root, &path, true).await.map(|_| ()) }));
    }
    for i in 0..N / 2 {
        let vcs = Arc::clone(&vcs);
        let root = h.root.clone();
        let project = h.project;
        ops.push(tokio::spawn(async move {
            vcs.add_worktree(
                project,
                &root,
                orkestr_vcs::AddWorktreeRequest {
                    agent_id: Some(orkestr_domain::AgentId::new()),
                    role: Some(format!("extra{i}")),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
        }));
    }
    for op in ops {
        op.await.unwrap().expect("concurrent worktree mutation");
    }
    assert_eq!(vcs.worktrees(h.project, &h.root).await.unwrap().len(), N + 1);
}
