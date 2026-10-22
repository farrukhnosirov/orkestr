//! End-to-end Bug Detective workflow over a real git repository with fake
//! provider / agent / tests / review / monitor / deploy seams.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_detective::ports::{
    DeploymentsReader, FixAgent, FixOutcome, FixRequest, HypothesisModel, ModelReply, ModelRequest, MonitorReader,
    ProjectRoots, QualityFailure, QualityReader, ReviewOutcome, ReviewRequest, Reviewer, TestRequest, TestRunner,
};
use orkestr_detective::{
    Detective, DetectiveDeps, DetectiveError, DetectiveSettings, GitVcs, Incident, IncidentStage, Ports,
};
use orkestr_domain::{
    now, Actor, AgentId, AlertId, ApprovalStatus, Deployment, DeploymentId, DeploymentStatus, DeploymentTarget,
    EnvironmentId, IncidentId, NodeId, ProjectId, Timestamp,
};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::event::{EventKind, NewEvent};
use orkestr_protocol::methods::detective::{EvidenceKind, IncidentRun, IncidentTrigger, OpenIncidentParams};
use orkestr_protocol::methods::monitor::{
    Alert, AlertPayload, AlertSeverity, AlertSource, AlertState, LogLine, LogQuery, MetricPoint, MetricSeries,
};
use orkestr_security::Security;
use orkestr_storage::Db;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use IncidentStage as S;

const BUGGY: &str = "pub fn div(a: i32, b: i32) -> i32 {\n    a / b\n}\n";
const GOOD_V1: &str = "pub fn div(a: i32, b: i32) -> i32 {\n    if b == 0 { return 0; }\n    a / b\n}\n";

fn git(args: &[&str], dir: &Path) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Where a fake should block (for cancel tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    Collect,
    Model,
    Repro,
    Agent,
    FixTests,
    Review,
}

struct Active(Arc<AtomicUsize>);
impl Active {
    fn enter(c: &Arc<AtomicUsize>) -> Self {
        c.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(c))
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct Fakes {
    block: Arc<Mutex<Block>>,
    active: Arc<AtomicUsize>,
    root: PathBuf,
    alert: Alert,
    deployments: Vec<Deployment>,
    replies: Arc<Mutex<Vec<String>>>,
    model_calls: Arc<AtomicUsize>,
    model_cost: f64,
    agent_cost: f64,
    test_calls: Arc<AtomicUsize>,
    stopped: Arc<Mutex<Vec<AgentId>>>,
}

impl Fakes {
    fn blocked(&self, b: Block) -> bool {
        *self.block.lock() == b
    }
}

#[async_trait]
impl MonitorReader for Fakes {
    async fn alert(&self, id: AlertId) -> Result<Option<Alert>, DetectiveError> {
        Ok((id == self.alert.id).then(|| self.alert.clone()))
    }
    async fn active_alerts(&self, _p: ProjectId) -> Result<Vec<Alert>, DetectiveError> {
        Ok(vec![self.alert.clone()])
    }
    async fn logs(&self, q: &LogQuery) -> Result<Vec<LogLine>, DetectiveError> {
        if self.blocked(Block::Collect) {
            let _a = Active::enter(&self.active);
            std::future::pending::<()>().await;
        }
        let line = |m: &str| LogLine { ts: now(), source_id: "api".into(), level: None, message: m.into() };
        Ok(match q.source_id.as_deref() {
            Some("api") => vec![
                line("GET /div?a=1&b=0"),
                line("thread 'main' panicked at src/calc.rs:2:5:"),
                line("attempt to divide by zero"),
            ],
            _ => vec![],
        })
    }
    async fn metrics(&self, since: Timestamp) -> Result<Vec<MetricSeries>, DetectiveError> {
        Ok(vec![MetricSeries {
            node_id: NodeId::new(),
            metric: "cpu".into(),
            unit: "percent".into(),
            points: vec![MetricPoint { ts: since, value: 10.0 }, MetricPoint { ts: now(), value: 95.0 }],
        }])
    }
}

#[async_trait]
impl DeploymentsReader for Fakes {
    async fn recent(&self, _p: ProjectId, _limit: u32) -> Result<Vec<Deployment>, DetectiveError> {
        Ok(self.deployments.clone())
    }
    async fn logs(&self, _d: DeploymentId, _max: u32) -> Result<Vec<String>, DetectiveError> {
        Ok(vec!["build ok".into(), "started on :8080".into()])
    }
}

#[async_trait]
impl QualityReader for Fakes {
    async fn failure(&self, _p: ProjectId, _r: Option<&str>) -> Result<Option<QualityFailure>, DetectiveError> {
        Ok(None)
    }
}

#[async_trait]
impl ProjectRoots for Fakes {
    async fn root(&self, _p: ProjectId) -> Result<PathBuf, DetectiveError> {
        Ok(self.root.clone())
    }
}

#[async_trait]
impl HypothesisModel for Fakes {
    async fn complete(&self, req: ModelRequest, cancel: CancellationToken) -> Result<ModelReply, DetectiveError> {
        assert!(req.messages[0].1.contains("ev1"), "evidence is in the prompt");
        if self.blocked(Block::Model) {
            let _a = Active::enter(&self.active);
            cancel.cancelled().await;
            return Err(DetectiveError::Cancelled);
        }
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        let mut r = self.replies.lock();
        let text = if r.len() > 1 { r.remove(0) } else { r[0].clone() };
        Ok(ModelReply { text, cost_usd: self.model_cost })
    }
}

#[async_trait]
impl FixAgent for Fakes {
    async fn run(
        &self,
        req: FixRequest,
        on_agent: Arc<dyn Fn(AgentId) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<FixOutcome, DetectiveError> {
        let agent = AgentId::new();
        on_agent(agent);
        assert!(req.prompt.contains("divide by zero") || req.prompt.contains("Root cause"));
        assert_eq!(git(&["rev-parse", "--abbrev-ref", "HEAD"], &req.worktree), req.branch);
        if self.blocked(Block::Agent) {
            let _a = Active::enter(&self.active);
            cancel.cancelled().await;
            return Err(DetectiveError::Cancelled);
        }
        // The "mock provider" agent writes the fix (uncommitted).
        std::fs::write(req.worktree.join("src/calc.rs"), GOOD_V1.replace("return 0;", "return 0; // FIXED")).unwrap();
        Ok(FixOutcome {
            agent_id: Some(agent),
            task_id: None,
            cost_usd: self.agent_cost,
            summary: "guarded b == 0".into(),
        })
    }
    async fn stop(&self, agent_id: AgentId) {
        self.stopped.lock().push(agent_id);
    }
}

#[async_trait]
impl TestRunner for Fakes {
    async fn run(&self, req: TestRequest, cancel: CancellationToken) -> Result<IncidentRun, DetectiveError> {
        let n = self.test_calls.fetch_add(1, Ordering::SeqCst);
        if (n == 0 && self.blocked(Block::Repro)) || (n > 0 && self.blocked(Block::FixTests)) {
            let _a = Active::enter(&self.active);
            cancel.cancelled().await;
            return Err(DetectiveError::Cancelled);
        }
        let code = std::fs::read_to_string(req.dir.join("src/calc.rs")).unwrap();
        let passed = code.contains("FIXED");
        Ok(IncidentRun {
            command: req.command.unwrap_or_else(|| "cargo test".into()),
            exit_code: Some(i32::from(!passed)),
            passed,
            output_tail: if passed { "test result: ok".into() } else { "panicked: attempt to divide by zero".into() },
            finished_at: now(),
        })
    }
}

#[async_trait]
impl Reviewer for Fakes {
    async fn review(&self, req: ReviewRequest, cancel: CancellationToken) -> Result<ReviewOutcome, DetectiveError> {
        assert!(req.diff.contains("FIXED"), "review sees the fix diff: {}", req.diff);
        if self.blocked(Block::Review) {
            let _a = Active::enter(&self.active);
            cancel.cancelled().await;
            return Err(DetectiveError::Cancelled);
        }
        Ok(ReviewOutcome { review_id: Some("rev-1".into()), approved: true, summary: "LGTM".into(), cost_usd: 0.01 })
    }
}

fn hypothesis_json() -> String {
    serde_json::json!({
        "summary": "div() divides by zero",
        "rootCause": "commit 'remove guard' dropped the b == 0 check in src/calc.rs",
        "confidence": 0.9,
        "evidenceRefs": ["ev1"],
        "suspectedFiles": ["src/calc.rs:2"],
        "proposedFix": "restore the guard",
        "reproduction": "cargo test div_by_zero"
    })
    .to_string()
}

struct H {
    _dir: tempfile::TempDir,
    root: PathBuf,
    det: Detective,
    fakes: Fakes,
    security: Arc<Security>,
    events: EventBus,
    project: ProjectId,
    main_head: String,
}

async fn harness(block: Block, replies: Vec<String>, model_cost: f64, agent_cost: f64) -> Option<H> {
    if !git_available() {
        eprintln!("git not installed вЂ” skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&["init", "-q", "-b", "main"], &root);
    git(&["config", "user.name", "Test"], &root);
    git(&["config", "user.email", "test@example.com"], &root);
    git(&["config", "core.autocrlf", "false"], &root);
    std::fs::write(root.join("src/calc.rs"), GOOD_V1).unwrap();
    git(&["add", "-A"], &root);
    git(&["commit", "-q", "-m", "v1"], &root);
    let v1 = git(&["rev-parse", "HEAD"], &root);
    std::fs::write(root.join("src/calc.rs"), BUGGY).unwrap();
    git(&["commit", "-q", "-am", "remove guard"], &root);
    let v2 = git(&["rev-parse", "HEAD"], &root);

    let db = Db::open(dir.path().join("d.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_vcs::vcs_migrations());
    migs.extend(orkestr_detective::detective_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(4096);
    let security = Arc::new(
        Security::open(orkestr_security::SecurityConfig {
            db: db.clone(),
            events: events.clone(),
            data_dir: dir.path().join("data"),
            credentials: Some(Arc::new(orkestr_platform::MemoryCredentialStore::default())),
            secret_backend: orkestr_security::SecretBackendChoice::Memory,
            guard: orkestr_security::GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let project = ProjectId::new();
    let env = EnvironmentId::new();
    let dep = |v: &str, ago: i64| Deployment {
        id: DeploymentId::new(),
        project_id: project,
        environment_id: env,
        target: DeploymentTarget::LocalProcess,
        status: DeploymentStatus::Succeeded,
        version: Some(v.to_owned()),
        task_id: None,
        triggered_by: Actor::System,
        url: Some("http://127.0.0.1:8080".into()),
        log_object_id: None,
        started_at: None,
        finished_at: Some(now() - chrono::Duration::minutes(ago)),
        created_at: now() - chrono::Duration::minutes(ago),
    };
    let alert = Alert {
        id: AlertId::new(),
        project_id: Some(project),
        rule: "log-errors".into(),
        severity: AlertSeverity::Critical,
        title: "Errors in api".into(),
        message: "3 error lines in api".into(),
        source: AlertSource::Log { source_id: "api".into() },
        state: AlertState::Open,
        count: 1,
        opened_at: now(),
        acked_at: None,
        resolved_at: None,
    };
    let fakes = Fakes {
        block: Arc::new(Mutex::new(block)),
        active: Arc::new(AtomicUsize::new(0)),
        root: root.clone(),
        alert,
        deployments: vec![dep(&v2, 5), dep(&v1, 60)],
        replies: Arc::new(Mutex::new(replies)),
        model_calls: Arc::new(AtomicUsize::new(0)),
        model_cost,
        agent_cost,
        test_calls: Arc::new(AtomicUsize::new(0)),
        stopped: Arc::new(Mutex::new(Vec::new())),
    };
    let vcs =
        orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.path().join("worktrees"));
    let det = Detective::new(DetectiveDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: dir.path().join("detective"),
    });
    let f = Arc::new(fakes.clone());
    det.install(Ports {
        projects: f.clone(),
        vcs: Arc::new(GitVcs::new(vcs)),
        model: f.clone(),
        agent: f.clone(),
        tests: f.clone(),
        reviewer: f.clone(),
        monitor: Some(f.clone()),
        deployments: Some(f.clone()),
        quality: Some(f),
    })
    .await;
    Some(H { _dir: dir, root, det, fakes, security, events, project, main_head: v2 })
}

async fn wait_stage(det: &Detective, id: IncidentId, want: IncidentStage) -> Incident {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let inc = det.get(id).await.unwrap();
        if inc.stage == want {
            return inc;
        }
        assert!(
            !(inc.stage.is_terminal() && !want.is_terminal()),
            "incident ended in {:?} ({:?}) while waiting for {want:?}",
            inc.stage,
            inc.error
        );
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {want:?}; at {:?}", inc.stage);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_active(f: &Fakes, n: usize) {
    for _ in 0..1500 {
        if f.active.load(Ordering::SeqCst) == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("active children never reached {n}");
}

fn open_params(h: &H) -> OpenIncidentParams {
    OpenIncidentParams {
        project_id: h.project,
        trigger: IncidentTrigger::Alert { alert_id: h.fakes.alert.id },
        title: None,
        budget_usd: Some(5.0),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alert_to_merge_gate_keeps_main_untouched_then_merges() {
    let Some(h) = harness(Block::None, vec![hypothesis_json()], 0.02, 0.5).await else { return };
    let mut sub = h.events.subscribe_all();
    let inc = h.det.open(open_params(&h), h.security.user()).await.unwrap();
    assert_eq!(inc.title, "Errors in api");

    let inc = wait_stage(&h.det, inc.id, S::AwaitingFixApproval).await;
    let kinds: Vec<EvidenceKind> = inc.evidence.iter().map(|e| e.kind).collect();
    for k in [
        EvidenceKind::Alert,
        EvidenceKind::Logs,
        EvidenceKind::Metrics,
        EvidenceKind::Deployments,
        EvidenceKind::Commits,
        EvidenceKind::Diff,
        EvidenceKind::StackTrace,
        EvidenceKind::CodeSnippet,
    ] {
        assert!(kinds.contains(&k), "missing {k:?} in {kinds:?}");
    }
    let snippet = inc.evidence.iter().find(|e| e.kind == EvidenceKind::CodeSnippet).unwrap();
    assert_eq!(snippet.reference.as_deref(), Some("src/calc.rs:2"));
    assert!(snippet.content.contains(">    2 |     a / b"), "{}", snippet.content);
    let diff = inc.evidence.iter().find(|e| e.kind == EvidenceKind::Diff).unwrap();
    assert!(diff.title.contains("last good deploy") && diff.content.contains("if b == 0"), "{diff:?}");
    assert!(inc.evidence.iter().enumerate().all(|(i, e)| e.id == format!("ev{}", i + 1)));
    let hyp = inc.hypothesis.clone().unwrap();
    assert!((hyp.confidence - 0.9).abs() < 1e-9);
    assert!(!inc.reproduction.as_ref().unwrap().passed, "reproduction fails before the fix");
    let approval = inc.approval_id.expect("gate 1 approval");

    // Gate 1 decided in the approvals inbox.
    h.security
        .approvals
        .resolve(approval, orkestr_security::ApprovalDecision::Approve, None, h.security.user())
        .await
        .unwrap();
    let inc = wait_stage(&h.det, inc.id, S::AwaitingMergeApproval).await;
    let branch = format!("detective/{}", inc.id);
    assert_eq!(inc.fix_branch.as_deref(), Some(branch.as_str()));
    assert!(inc.fix_tests.as_ref().unwrap().passed);
    assert_eq!(inc.review_id.as_deref(), Some("rev-1"));
    assert!(inc.agent_id.is_some());
    assert!((inc.spent_usd - 0.53).abs() < 1e-9, "spent {}", inc.spent_usd);
    // main is untouched; the fix lives on detective/<id>.
    assert_eq!(git(&["rev-parse", "main"], &h.root), h.main_head);
    assert_eq!(std::fs::read_to_string(h.root.join("src/calc.rs")).unwrap(), BUGGY);
    assert!(git(&["show", &format!("{branch}:src/calc.rs")], &h.root).contains("FIXED"));
    let merge_approval = inc.approval_id.expect("gate 2 approval");
    assert_ne!(merge_approval, approval);

    // Merge needs gate 2: a second approveFix is refused.
    assert!(matches!(h.det.approve_fix(inc.id, None, h.security.user()).await, Err(DetectiveError::Conflict(_))));
    let inc = h.det.approve_merge(inc.id, Some("ship it".into()), h.security.user()).await.unwrap();
    assert_eq!(inc.stage, S::Resolved);
    assert!(std::fs::read_to_string(h.root.join("src/calc.rs")).unwrap().contains("FIXED"));
    assert_ne!(git(&["rev-parse", "main"], &h.root), h.main_head);
    let a = h.security.approvals.get(merge_approval).await.unwrap().unwrap();
    assert_eq!(a.status, ApprovalStatus::Approved);

    // Events: opened + the full stage sequence.
    let mut stages = Vec::new();
    let mut opened = false;
    while let Ok(Some(Received::Event(ev))) = tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
        match &ev.kind {
            EventKind::IncidentOpened(_) => opened = true,
            EventKind::IncidentStageChanged(p) => stages.push(p.to),
            _ => {}
        }
    }
    assert!(opened);
    assert_eq!(
        stages,
        vec![
            S::Collecting,
            S::Hypothesizing,
            S::Reproducing,
            S::AwaitingFixApproval,
            S::Fixing,
            S::Testing,
            S::Reviewing,
            S::AwaitingMergeApproval,
            S::Resolved
        ]
    );
    let audit = h.security.audit.query(orkestr_security::AuditQuery::default()).await.unwrap();
    assert!(audit.iter().any(|r| r.event.action == "detective.merge"));
    assert!(audit.iter().any(|r| r.event.action == "detective.open"));
    assert_eq!(h.det.live_runs(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_works_at_every_stage_and_stops_children() {
    let cases = [
        (Block::Collect, S::Collecting),
        (Block::Model, S::Hypothesizing),
        (Block::Repro, S::Reproducing),
        (Block::None, S::AwaitingFixApproval),
        (Block::Agent, S::Fixing),
        (Block::FixTests, S::Testing),
        (Block::Review, S::Reviewing),
        (Block::None, S::AwaitingMergeApproval),
    ];
    for (block, at) in cases {
        let Some(h) = harness(block, vec![hypothesis_json()], 0.0, 0.0).await else { return };
        let inc = h.det.open(open_params(&h), h.security.user()).await.unwrap();
        if ordinal_after_gate1(at) {
            wait_stage(&h.det, inc.id, S::AwaitingFixApproval).await;
            h.det.approve_fix(inc.id, None, h.security.user()).await.unwrap();
        }
        let before = wait_stage(&h.det, inc.id, at).await;
        if block != Block::None {
            wait_active(&h.fakes, 1).await;
        }
        let gate_approval = before.approval_id;
        let done = h.det.cancel(inc.id, Some("stop".into()), h.security.user()).await.unwrap();
        assert_eq!(done.stage, S::Cancelled, "{at:?}");
        assert_eq!(done.error.as_deref(), Some("stop"));
        assert_eq!(h.fakes.active.load(Ordering::SeqCst), 0, "children still running after cancel at {at:?}");
        assert_eq!(h.det.live_runs(), 0, "{at:?}");
        if at == S::Fixing {
            assert_eq!(h.fakes.stopped.lock().len(), 1, "agent stopped");
        }
        if let Some(a) = gate_approval {
            let a = h.security.approvals.get(a).await.unwrap().unwrap();
            assert_eq!(a.status, ApprovalStatus::Cancelled, "{at:?}");
        }
        assert_eq!(git(&["rev-parse", "main"], &h.root), h.main_head);
        assert!(matches!(h.det.cancel(inc.id, None, h.security.user()).await, Err(DetectiveError::Conflict(_))));
        assert_eq!(h.det.get(inc.id).await.unwrap().stage, S::Cancelled);
    }
}

fn ordinal_after_gate1(s: IncidentStage) -> bool {
    matches!(s, S::Fixing | S::Testing | S::Reviewing | S::AwaitingMergeApproval)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emergency_cancel_all_stops_every_incident() {
    let Some(h) = harness(Block::Model, vec![hypothesis_json()], 0.0, 0.0).await else { return };
    let a = h.det.open(open_params(&h), h.security.user()).await.unwrap();
    let b = h.det.open(open_params(&h), h.security.user()).await.unwrap();
    wait_active(&h.fakes, 2).await;
    assert_eq!(h.det.cancel_all().await, 2);
    assert_eq!(h.fakes.active.load(Ordering::SeqCst), 0);
    for id in [a.id, b.id] {
        let inc = h.det.get(id).await.unwrap();
        assert_eq!((inc.stage, inc.error.as_deref()), (S::Cancelled, Some("emergency stop")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_hypothesis_is_retried_then_accepted() {
    let bad = hypothesis_json().replace("0.9", "7");
    let Some(h) = harness(Block::None, vec!["no json at all".into(), bad, hypothesis_json()], 0.01, 0.0).await else {
        return;
    };
    let inc = h.det.open(open_params(&h), h.security.user()).await.unwrap();
    let inc = wait_stage(&h.det, inc.id, S::AwaitingFixApproval).await;
    assert_eq!(h.fakes.model_calls.load(Ordering::SeqCst), 3);
    assert!((inc.spent_usd - 0.03).abs() < 1e-9);
    assert_eq!(inc.hypothesis.unwrap().summary, "div() divides by zero");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_invalid_hypothesis_fails_the_incident() {
    let Some(h) = harness(Block::None, vec!["{\"summary\": 1}".into()], 0.0, 0.0).await else { return };
    let inc = h.det.open(open_params(&h), h.security.user()).await.unwrap();
    let inc = wait_stage(&h.det, inc.id, S::Failed).await;
    assert_eq!(h.fakes.model_calls.load(Ordering::SeqCst), 3);
    assert!(inc.error.unwrap().contains("valid hypothesis"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budget_cap_aborts_with_reason() {
    // Model calls cost 0.6 each against a 1.0 cap: the first answer is
    // invalid, the second call crosses the cap.
    let Some(h) = harness(Block::None, vec!["nope".into(), hypothesis_json()], 0.6, 0.0).await else { return };
    let mut p = open_params(&h);
    p.budget_usd = Some(1.0);
    let inc = h.det.open(p, h.security.user()).await.unwrap();
    let inc = wait_stage(&h.det, inc.id, S::Failed).await;
    assert!(inc.error.as_deref().unwrap().starts_with("budget cap reached"), "{:?}", inc.error);
    assert!(inc.spent_usd > 1.0);

    // An expensive agent run is stopped by the cap too (no tests, no review).
    let Some(h) = harness(Block::None, vec![hypothesis_json()], 0.1, 5.0).await else { return };
    let mut p = open_params(&h);
    p.budget_usd = Some(1.0);
    let inc = h.det.open(p, h.security.user()).await.unwrap();
    wait_stage(&h.det, inc.id, S::AwaitingFixApproval).await;
    h.det.approve_fix(inc.id, None, h.security.user()).await.unwrap();
    let inc = wait_stage(&h.det, inc.id, S::Failed).await;
    assert!(inc.error.as_deref().unwrap().contains("budget"), "{:?}", inc.error);
    assert!(inc.fix_tests.is_none());
    assert_eq!(git(&["rev-parse", "main"], &h.root), h.main_head);

    // Invalid budgets are refused.
    let mut p = open_params(&h);
    p.budget_usd = Some(0.0);
    assert!(matches!(h.det.open(p, h.security.user()).await, Err(DetectiveError::Invalid(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_open_on_alert_is_opt_in() {
    let Some(h) = harness(Block::Model, vec![hypothesis_json()], 0.0, 0.0).await else { return };
    let raise = || async {
        h.events
            .publish(NewEvent::new(EventKind::AlertRaised(AlertPayload { alert: h.fakes.alert.clone() })))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    raise().await;
    assert!(h.det.list(&Default::default()).await.unwrap().is_empty(), "off by default");
    h.det.set_settings(DetectiveSettings { auto_open_on_alert: true, ..DetectiveSettings::default() }).await.unwrap();
    raise().await;
    raise().await;
    let list = h.det.list(&Default::default()).await.unwrap();
    assert_eq!(list.len(), 1, "deduplicated per alert");
    assert!(matches!(list[0].trigger, IncidentTrigger::Alert { alert_id } if alert_id == h.fakes.alert.id));
    h.det.cancel_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_and_log_triggers_and_validation() {
    let Some(h) = harness(Block::None, vec![hypothesis_json()], 0.0, 0.0).await else { return };
    let p =
        |t: IncidentTrigger| OpenIncidentParams { project_id: h.project, trigger: t, title: None, budget_usd: None };
    let inc = h
        .det
        .open(
            p(IncidentTrigger::LogError {
                source_id: "api".into(),
                line: "thread 'main' panicked at src/calc.rs:2:5: attempt to divide by zero".into(),
            }),
            h.security.user(),
        )
        .await
        .unwrap();
    let inc = wait_stage(&h.det, inc.id, S::AwaitingFixApproval).await;
    assert!(inc.evidence.iter().any(|e| e.kind == EvidenceKind::CodeSnippet));
    assert!((inc.budget_usd - DetectiveSettings::default().default_budget_usd).abs() < 1e-9);
    assert!(matches!(
        h.det.open(p(IncidentTrigger::Manual { description: "  ".into() }), h.security.user()).await,
        Err(DetectiveError::Invalid(_))
    ));
    assert!(matches!(
        h.det.open(p(IncidentTrigger::Alert { alert_id: AlertId::new() }), h.security.user()).await,
        Err(DetectiveError::NotFound(_))
    ));
    let m = h
        .det
        .open(p(IncidentTrigger::Manual { description: "Checkout is slow\nsince Monday".into() }), h.security.user())
        .await
        .unwrap();
    assert_eq!(m.title, "Checkout is slow");
    assert!(matches!(h.det.approve_merge(inc.id, None, h.security.user()).await, Err(DetectiveError::Conflict(_))));
    h.det.cancel_all().await;
}
