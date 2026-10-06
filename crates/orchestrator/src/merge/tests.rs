//! Merge Manager tests over real engines (SQLite, security approvals, task
//! engine, snapshots, git in temp repositories) with the shipped offline Mock
//! provider for AI conflict suggestions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_agent_runtime::{ModelGateway, ProviderInfo};
use orkestr_domain::{
    now, AgentRole, ApprovalMode, ManagerPlanId, ManagerRunId, ModelRef, Project, ProjectId, ProviderId, ProviderKind,
    TaskActor, TaskId, TaskStatus,
};
use orkestr_events::EventBus;
use orkestr_protocol::methods::manager::{ManagerRun, ManagerRunStatus, RunLimits, RunTask};
use orkestr_protocol::methods::merge::{
    MergeAbortParams, MergeBlockerKind, MergeBranchInput, MergeBranchState, MergeConflictScope, MergeExecuteParams,
    MergeGate, MergeGateStatus, MergeListParams, MergeManagerProposal, MergePolicy, MergePolicyMode,
    MergeProposalStatus, MergeProposeParams, MergeQualityCheck, MergeQualityReport, MergeRecommendation,
    MergeResolution, MergeResolveConflictParams,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_providers::adapters::mock::{MockAdapter, MockOptions};
use orkestr_providers::{
    chat_with_fallback, AdapterSource, AttemptOutcome, ChatRequest, ChatStream, FallbackOptions, HandoffHook,
    ProviderAdapter, ProviderEvent, RouteRequest,
};
use orkestr_security::{ApprovalDecision, Security};
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    MergeCompletedHook, MergeConfig, MergeDeps, MergeGateSource, MergeManager, MergeQualityRunner, NoGateSource,
};

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Source {
    adapters: Mutex<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
}

impl AdapterSource for Source {
    fn adapter(&self, provider: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.adapters.lock().get(provider).cloned()
    }
    fn observe(&self, _model: &ModelRef, _outcome: &AttemptOutcome) {}
    fn emit(&self, _event: ProviderEvent) {}
}

struct Gateway {
    source: Arc<Source>,
    model: ModelRef,
}

#[async_trait]
impl ModelGateway for Gateway {
    async fn route(&self, _req: &RouteRequest) -> Result<Vec<ModelRef>, String> {
        Ok(vec![self.model.clone()])
    }
    fn chat(
        &self,
        chain: Vec<ModelRef>,
        request: ChatRequest,
        hook: Option<Arc<dyn HandoffHook>>,
        cancel: CancellationToken,
    ) -> ChatStream {
        let opts = FallbackOptions { max_rate_limit_wait: Duration::ZERO, same_model_retries: 0 };
        chat_with_fallback(Arc::clone(&self.source) as Arc<dyn AdapterSource>, chain, request, hook, opts, cancel)
    }
    fn provider(&self, id: ProviderId) -> Option<ProviderInfo> {
        self.source.adapters.lock().contains_key(&id).then_some(ProviderInfo {
            kind: ProviderKind::Mock,
            is_local: true,
            enabled: true,
        })
    }
    fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.source.adapters.lock().get(&id).cloned()
    }
}

/// Gate source with fixed answers.
struct FixedGates {
    review: MergeGateStatus,
    tests: MergeGateStatus,
}

#[async_trait]
impl MergeGateSource for FixedGates {
    async fn review(&self, _p: ProjectId, _t: Option<TaskId>, _b: &str) -> MergeGate {
        MergeGate::of(self.review)
    }
    async fn tests(&self, _p: ProjectId, _t: Option<TaskId>, _b: &str) -> MergeGate {
        MergeGate::with(self.tests, "npm test")
    }
}

/// Quality runner with a fixed verdict that records the checked tree.
struct FixedQuality {
    passed: bool,
    seen: Mutex<Vec<PathBuf>>,
}

#[async_trait]
impl MergeQualityRunner for FixedQuality {
    async fn check(
        &self,
        _project: &Project,
        dir: &Path,
        _cancel: &CancellationToken,
    ) -> Result<MergeQualityReport, String> {
        self.seen.lock().push(dir.to_path_buf());
        Ok(MergeQualityReport {
            passed: self.passed,
            checks: vec![MergeQualityCheck {
                name: "test".into(),
                passed: self.passed,
                detail: (!self.passed).then(|| "1 failing test".into()),
            }],
        })
    }
}

fn git_available() -> bool {
    std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(args: &[&str], dir: &Path) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct H {
    _dir: tempfile::TempDir,
    root: PathBuf,
    sec: Arc<Security>,
    tasks: Arc<orkestr_task_engine::TaskService>,
    db: Db,
    mm: MergeManager,
    project: ProjectId,
    quality: Arc<FixedQuality>,
}

async fn harness(gates: Arc<dyn MergeGateSource>, quality_passes: Option<bool>) -> Option<H> {
    if !git_available() {
        eprintln!("git not installed — skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    git(&["init", "-q", "-b", "main"], &root);
    git(&["config", "user.name", "Test"], &root);
    git(&["config", "user.email", "test@example.com"], &root);
    git(&["config", "core.autocrlf", "false"], &root);
    std::fs::write(root.join("README.md"), "# Demo\n").unwrap();
    std::fs::write(root.join("shared.txt"), "line 1\nline 2\nline 3\n").unwrap();
    git(&["add", "-A"], &root);
    git(&["commit", "-q", "-m", "init"], &root);

    let db = Db::open(dir.path().join("m.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_task_engine::task_migrations());
    migs.extend(orkestr_vcs::vcs_migrations());
    migs.extend(orkestr_snapshots::snapshot_migrations());
    migs.extend(orkestr_agent_runtime::agent_migrations());
    migs.extend(crate::orchestrator_migrations());
    migs.extend(crate::merge_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(4096);
    let sec = Arc::new(
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
    let meta = Arc::new(SqliteMetadataStore::new(db.clone()));
    let mut p = Project::new("Demo", now());
    p.root_path = Some(root.to_string_lossy().into_owned());
    p.approval_mode = ApprovalMode::SemiAuto;
    meta.insert(&p).await.unwrap();
    let tasks = Arc::new(orkestr_task_engine::TaskService::new(db.clone(), events.clone()));
    let objects = Arc::new(
        orkestr_storage::FsObjectStore::open(dir.path().join("objects"), orkestr_storage::Compression::None).unwrap(),
    );
    let snapshots = Arc::new(orkestr_snapshots::SnapshotService::new(
        db.clone(),
        objects as Arc<dyn orkestr_storage::ObjectStore>,
        None,
        orkestr_snapshots::ScanOptions::default(),
    ));
    let source = Arc::new(Source::default());
    let pid = ProviderId::new();
    source.adapters.lock().insert(
        pid,
        Arc::new(MockAdapter::new(pid, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
    );
    let gateway = Arc::new(Gateway { source, model: ModelRef { provider_id: pid, model: "mock-smart".into() } });
    let quality = Arc::new(FixedQuality { passed: quality_passes.unwrap_or(true), seen: Mutex::new(Vec::new()) });
    let mut deps = MergeDeps {
        db: db.clone(),
        events,
        security: Arc::clone(&sec),
        tasks: Arc::clone(&tasks),
        projects: Arc::clone(&meta) as Arc<dyn ProjectRepository>,
        vcs: orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.path().join("worktrees")),
        snapshots,
        agents: None,
        models: Some(gateway as Arc<dyn ModelGateway>),
        hub: None,
        cost: Some(orkestr_cost::CostEngine::new(db.clone())),
        gates,
        quality: None,
        config: MergeConfig {
            approval_timeout: Duration::from_secs(60),
            abort_wait: Duration::from_secs(20),
            ..MergeConfig::default()
        },
    };
    if quality_passes.is_some() {
        deps = deps.with_quality(Arc::clone(&quality) as Arc<dyn MergeQualityRunner>);
    }
    let mm = MergeManager::new(deps);
    mm.init().await.unwrap();
    Some(H { _dir: dir, root, sec, tasks, db, mm, project: p.id, quality })
}

impl H {
    /// Creates `name` from `base` with one commit writing `files`.
    fn branch(&self, name: &str, base: &str, files: &[(&str, &str)]) {
        git(&["checkout", "-q", "-b", name, base], &self.root);
        for (f, c) in files {
            let p = self.root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        git(&["add", "-A"], &self.root);
        git(&["commit", "-q", "-m", &format!("work on {name}")], &self.root);
        git(&["checkout", "-q", "main"], &self.root);
    }

    async fn task(&self, title: &str, deps: Vec<TaskId>) -> TaskId {
        let mut params: CreateTaskParams =
            serde_json::from_value(serde_json::json!({"projectId": self.project, "title": title})).unwrap();
        params.depends_on = (!deps.is_empty()).then_some(deps);
        let t = self.tasks.create(params, TaskActor::User).await.unwrap();
        t.id
    }

    async fn complete(&self, id: TaskId) {
        for _ in 0..6 {
            let t = self.tasks.get(id).await.unwrap();
            let next = match t.status {
                TaskStatus::Completed => return,
                TaskStatus::Planned | TaskStatus::Blocked => TaskStatus::Ready,
                TaskStatus::Ready => TaskStatus::Running,
                _ => TaskStatus::Completed,
            };
            self.tasks.transition(id, next, TaskActor::User, None, None).await.unwrap();
        }
        panic!("task {id} could not be completed");
    }

    async fn propose(&self, branches: &[(&str, Option<TaskId>)]) -> MergeManagerProposal {
        self.mm
            .propose(MergeProposeParams {
                project_id: self.project,
                run_id: None,
                branches: Some(
                    branches.iter().map(|(b, t)| MergeBranchInput { branch: (*b).into(), task_id: *t }).collect(),
                ),
                target: None,
            })
            .await
            .unwrap()
    }

    async fn policy(&self, f: impl FnOnce(&mut MergePolicy)) {
        let mut p = self.mm.policy(self.project).await.unwrap();
        f(&mut p);
        self.mm.set_policy(self.project, p).await.unwrap();
    }

    async fn approve(&self, p: &MergeManagerProposal) {
        let aid = p.execution.as_ref().and_then(|e| e.approval_id).expect("approval requested");
        self.sec.approvals.resolve(aid, ApprovalDecision::Approve, None, self.sec.user()).await.unwrap();
    }

    async fn wait_status(
        &self,
        id: orkestr_domain::MergeProposalId,
        want: &[MergeProposalStatus],
    ) -> MergeManagerProposal {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let p = self.mm.get(id).await.unwrap();
            if want.contains(&p.status) {
                return p;
            }
            assert!(tokio::time::Instant::now() < deadline, "proposal stuck in {:?} ({:?})", p.status, p.status_detail);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    fn rev(&self, r: &str) -> String {
        git(&["rev-parse", r], &self.root)
    }

    fn branches(&self) -> String {
        git(&["branch", "--list"], &self.root)
    }
}

fn exec(id: orkestr_domain::MergeProposalId) -> MergeExecuteParams {
    MergeExecuteParams { proposal_id: id, order: None }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_conflicting_branches_merge_in_dependency_order_after_approval() {
    let Some(h) = harness(Arc::new(NoGateSource), None).await else { return };
    // Task graph: api ← ui (ui depends on api); docs independent.
    let api = h.task("Build the API", vec![]).await;
    let ui = h.task("Build the UI", vec![api]).await;
    let docs = h.task("Write docs", vec![]).await;
    for t in [api, ui, docs] {
        h.complete(t).await;
    }
    h.branch("agent/backend-1", "main", &[("src/api.rs", "pub fn api() -> u32 {\n    1\n}\n")]);
    // The UI agent worked on top of the API branch (as the Manager's run does).
    h.branch("agent/frontend-1", "agent/backend-1", &[("web/app.ts", "export function app() { return 1; }\n")]);
    h.branch("agent/docs-1", "main", &[("docs/guide.md", "# Guide\n")]);

    // The docs task's own pipeline proposed its branch alone first.
    let single = h.propose(&[("agent/docs-1", Some(docs))]).await;
    let hook = Arc::new(CountHook::default());
    h.mm.on_completed(Arc::clone(&hook) as Arc<dyn MergeCompletedHook>);

    // Given in the "wrong" order: the DAG decides.
    let p = h
        .propose(&[("agent/frontend-1", Some(ui)), ("agent/docs-1", Some(docs)), ("agent/backend-1", Some(api))])
        .await;
    let pos = |b: &str| p.order.iter().position(|o| o == b).unwrap();
    assert!(pos("agent/backend-1") < pos("agent/frontend-1"), "{:?}", p.order);
    let fe = p.branches.iter().find(|b| b.branch == "agent/frontend-1").unwrap();
    assert_eq!(fe.depends_on, vec!["agent/backend-1".to_owned()]);
    assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    // Default policy requires review + tests, which are unknown → review first.
    assert_eq!(p.recommendation, MergeRecommendation::ReviewFirst);
    assert!(!p.auto_mergeable);
    let be = p.branches.iter().find(|b| b.branch == "agent/backend-1").unwrap();
    assert!(be.diff.symbols.iter().any(|s| s.name == "api" && s.public), "{:?}", be.diff.symbols);
    assert_eq!(be.gates.task.status, MergeGateStatus::Passed);
    assert_eq!(be.commits_ahead, 1);

    // Reordering against the DAG is refused; a valid reorder is accepted.
    let bad =
        h.mm.reorder(p.id, vec!["agent/frontend-1".into(), "agent/backend-1".into(), "agent/docs-1".into()])
            .await
            .unwrap_err();
    assert!(bad.to_string().contains("must merge first"), "{bad}");
    let p =
        h.mm.reorder(p.id, vec!["agent/docs-1".into(), "agent/backend-1".into(), "agent/frontend-1".into()])
            .await
            .unwrap();
    assert_eq!(p.order[0], "agent/docs-1");

    let main_before = h.rev("main");
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(p.status, MergeProposalStatus::AwaitingApproval);
    // Nothing happens before the approval.
    assert_eq!(h.rev("main"), main_before);
    h.approve(&p).await;
    let done = h.wait_status(p.id, &[MergeProposalStatus::Completed, MergeProposalStatus::Failed]).await;
    assert_eq!(done.status, MergeProposalStatus::Completed, "{:?}", done.status_detail);
    let ex = done.execution.as_ref().unwrap();
    assert!(ex.snapshot_id.is_some(), "pre-merge snapshot");
    assert_eq!(ex.pre_merge_commit.as_deref(), Some(main_before.as_str()));
    assert_eq!(ex.final_commit.as_deref(), Some(h.rev("main").as_str()));
    assert!(done.branches.iter().all(|b| b.state == MergeBranchState::Merged));
    // The target got the files of every branch, through --no-ff merge commits in order.
    for f in ["src/api.rs", "web/app.ts", "docs/guide.md"] {
        assert!(h.root.join(f).exists(), "{f} missing after merge");
    }
    let log = git(&["log", "--merges", "--format=%s", "main"], &h.root);
    let subjects: Vec<&str> = log.lines().rev().collect();
    assert_eq!(subjects.len(), 3, "{log}");
    assert!(
        subjects[0].contains("agent/docs-1")
            && subjects[1].contains("agent/backend-1")
            && subjects[2].contains("agent/frontend-1")
    );
    // The integration branch is gone.
    assert!(!h.branches().contains("orkestr/integration-"), "{}", h.branches());
    // The single-branch proposal is superseded (its branch is merged), and
    // the completion hook ran once.
    let single = h.mm.get(single.id).await.unwrap();
    assert_eq!(single.status, MergeProposalStatus::Superseded, "{:?}", single.status_detail);
    assert!(single.status_detail.as_deref().unwrap_or_default().contains(&done.id.to_string()));
    assert_eq!(hook.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Listed.
    let list = h.mm.list(MergeListParams { project_id: h.project, run_id: None, limit: None }).await.unwrap();
    assert_eq!(list.len(), 2);
}

#[derive(Default)]
struct CountHook(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl MergeCompletedHook for CountHook {
    async fn merged(&self, p: &MergeManagerProposal) {
        assert_eq!(p.status, MergeProposalStatus::Completed);
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn conflicts_are_detected_resolved_with_an_ai_suggestion_and_merged() {
    let Some(h) = harness(Arc::new(NoGateSource), None).await else { return };
    h.policy(|p| {
        p.require_review = false;
        p.require_tests = false;
    })
    .await;
    h.branch("agent/a-1", "main", &[("shared.txt", "line 1\nline 2 from A\nline 3\n")]);
    h.branch("agent/b-1", "main", &[("shared.txt", "line 1\nline 2 from B\nline 3\n"), ("b.txt", "b\n")]);
    let p = h.propose(&[("agent/a-1", None), ("agent/b-1", None)]).await;
    assert_eq!(p.recommendation, MergeRecommendation::ResolveConflicts);
    assert!(p
        .conflicts
        .iter()
        .any(|c| c.scope == MergeConflictScope::Pair && c.paths == vec!["shared.txt".to_owned()]));
    let ordered = p.conflicts.iter().find(|c| c.scope == MergeConflictScope::Order).unwrap();
    assert_eq!(ordered.branch, "agent/b-1");
    assert_eq!(p.branches[1].predicted_conflicts, vec!["shared.txt".to_owned()]);
    // Conflicts always need a human, even under the Auto policy.
    assert!(!p.auto_mergeable);

    let main_before = h.rev("main");
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    h.approve(&p).await;
    let c = h.wait_status(p.id, &[MergeProposalStatus::Conflicted, MergeProposalStatus::Failed]).await;
    assert_eq!(c.status, MergeProposalStatus::Conflicted, "{:?}", c.status_detail);
    assert_eq!(h.rev("main"), main_before, "the target is untouched while conflicted");
    let ex = c.execution.as_ref().unwrap();
    assert_eq!(ex.conflict_branch.as_deref(), Some("agent/b-1"));
    let f = &ex.conflicts[0];
    assert_eq!(f.path, "shared.txt");
    assert_eq!(f.ours.as_deref(), Some("line 1\nline 2 from A\nline 3\n"));
    assert_eq!(f.theirs.as_deref(), Some("line 1\nline 2 from B\nline 3\n"));
    assert_eq!(f.base.as_deref(), Some("line 1\nline 2\nline 3\n"));
    assert!(f.result.as_deref().unwrap().contains("<<<<<<<"));

    // Continuing with an open conflict is refused.
    assert!(h.mm.execute(exec(p.id)).await.unwrap_err().to_string().contains("unresolved"));
    // Content with markers is refused.
    let err =
        h.mm.resolve_conflict(MergeResolveConflictParams {
            proposal_id: p.id,
            path: "shared.txt".into(),
            resolution: MergeResolution::Content { content: f.result.clone().unwrap() },
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("conflict markers"));

    // AI suggestion (Mock: union of both sides), stored but not applied.
    let s =
        h.mm.resolve_conflict(MergeResolveConflictParams {
            proposal_id: p.id,
            path: "shared.txt".into(),
            resolution: MergeResolution::Ai,
        })
        .await
        .unwrap();
    let sf = &s.execution.as_ref().unwrap().conflicts[0];
    let suggestion = sf.suggestion.clone().unwrap();
    assert_eq!(suggestion, "line 1\nline 2 from A\nline 2 from B\nline 3\n");
    assert!(!sf.resolved);
    assert!(sf.suggestion_note.as_deref().unwrap().contains("Mock"));

    // The user accepts it.
    let r =
        h.mm.resolve_conflict(MergeResolveConflictParams {
            proposal_id: p.id,
            path: "shared.txt".into(),
            resolution: MergeResolution::Content { content: suggestion.clone() },
        })
        .await
        .unwrap();
    assert!(r.execution.as_ref().unwrap().conflicts[0].resolved);
    let cont = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(cont.status, MergeProposalStatus::Merging);
    let done = h.wait_status(p.id, &[MergeProposalStatus::Completed, MergeProposalStatus::Failed]).await;
    assert_eq!(done.status, MergeProposalStatus::Completed, "{:?}", done.status_detail);
    assert_eq!(std::fs::read_to_string(h.root.join("shared.txt")).unwrap(), suggestion);
    assert!(h.root.join("b.txt").exists());
}

#[tokio::test]
async fn auto_policy_merges_only_when_every_gate_is_green() {
    // Tests gate passed, review gate unknown.
    let Some(h) =
        harness(Arc::new(FixedGates { review: MergeGateStatus::Unknown, tests: MergeGateStatus::Passed }), None).await
    else {
        return;
    };
    h.policy(|p| p.mode = MergePolicyMode::Auto).await;
    h.branch("agent/x-1", "main", &[("x.txt", "x\n")]);
    let p = h.propose(&[("agent/x-1", None)]).await;
    assert_eq!(p.branches[0].gates.tests.status, MergeGateStatus::Passed);
    assert_eq!(p.branches[0].gates.review.status, MergeGateStatus::Unknown);
    assert_eq!(p.recommendation, MergeRecommendation::ReviewFirst);
    assert!(!p.auto_mergeable);
    // Auto policy but a non-green gate → still asks.
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(p.status, MergeProposalStatus::AwaitingApproval);
    let p = h.mm.abort(MergeAbortParams { proposal_id: p.id, reason: Some("not now".into()) }).await.unwrap();
    assert_eq!(p.status, MergeProposalStatus::Aborted);
    let aid = p.execution.as_ref().unwrap().approval_id.unwrap();
    let a = h.sec.approvals.get(aid).await.unwrap().unwrap();
    assert_ne!(a.status, orkestr_domain::ApprovalStatus::Pending, "the approval is withdrawn");

    // Review not required by the policy → every gate green → merges without approval.
    h.policy(|p| p.require_review = false).await;
    let p = h.propose(&[("agent/x-1", None)]).await;
    assert_eq!(p.recommendation, MergeRecommendation::MergeNow);
    assert!(p.auto_mergeable);
    let started = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(started.status, MergeProposalStatus::Merging);
    let done = h.wait_status(p.id, &[MergeProposalStatus::Completed, MergeProposalStatus::Failed]).await;
    assert_eq!(done.status, MergeProposalStatus::Completed, "{:?}", done.status_detail);
    let ex = done.execution.unwrap();
    assert!(ex.auto_approved && ex.approval_id.is_none());
    assert!(h.root.join("x.txt").exists());

    // Manual policy with green gates still asks.
    h.policy(|p| p.mode = MergePolicyMode::Manual).await;
    h.branch("agent/y-1", "main", &[("y.txt", "y\n")]);
    let p = h.propose(&[("agent/y-1", None)]).await;
    assert_eq!(p.recommendation, MergeRecommendation::MergeNow);
    assert!(!p.auto_mergeable);
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(p.status, MergeProposalStatus::AwaitingApproval);
    // Rejection finishes the proposal without merging.
    let aid = p.execution.as_ref().unwrap().approval_id.unwrap();
    h.sec.approvals.resolve(aid, ApprovalDecision::Reject, Some("later".into()), h.sec.user()).await.unwrap();
    let r = h.wait_status(p.id, &[MergeProposalStatus::Rejected]).await;
    assert!(r.status_detail.unwrap().contains("later"));
    assert!(!h.root.join("y.txt").exists());
}

#[tokio::test]
async fn failed_quality_checks_roll_back_and_leave_the_target_untouched() {
    let Some(h) = harness(Arc::new(NoGateSource), Some(false)).await else { return };
    h.policy(|p| {
        p.mode = MergePolicyMode::Auto;
        p.require_review = false;
        p.require_tests = false;
    })
    .await;
    h.branch("agent/q-1", "main", &[("q.txt", "q\n")]);
    let main_before = h.rev("main");
    let p = h.propose(&[("agent/q-1", None)]).await;
    assert!(p.auto_mergeable);
    h.mm.execute(exec(p.id)).await.unwrap();
    let f = h.wait_status(p.id, &[MergeProposalStatus::Failed, MergeProposalStatus::Completed]).await;
    assert_eq!(f.status, MergeProposalStatus::Failed);
    assert!(f.status_detail.as_deref().unwrap().contains("quality"));
    let ex = f.execution.unwrap();
    assert!(!ex.quality.unwrap().passed);
    assert_eq!(h.quality.seen.lock().len(), 1, "checks ran on the integration worktree");
    assert!(h.quality.seen.lock()[0].to_string_lossy().contains("integration"));
    assert_eq!(h.rev("main"), main_before, "target unchanged");
    assert!(!h.root.join("q.txt").exists());
    assert!(!h.branches().contains("orkestr/integration-"), "integration branch removed");
    assert!(h.branches().contains("agent/q-1"), "the agent branch is kept");
    // A finished proposal cannot be executed again.
    assert!(h.mm.execute(exec(p.id)).await.is_err());
}

#[tokio::test]
async fn aborting_a_conflicted_merge_restores_everything() {
    let Some(h) = harness(Arc::new(NoGateSource), None).await else { return };
    h.branch("agent/a-1", "main", &[("shared.txt", "A\n")]);
    h.branch("agent/b-1", "main", &[("shared.txt", "B\n")]);
    let p = h.propose(&[("agent/a-1", None), ("agent/b-1", None)]).await;
    let main_before = h.rev("main");
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    h.approve(&p).await;
    let c = h.wait_status(p.id, &[MergeProposalStatus::Conflicted]).await;
    let wt = c.execution.as_ref().unwrap().integration_worktree.clone().unwrap();
    assert!(Path::new(&wt).is_dir());
    let a = h.mm.abort(MergeAbortParams { proposal_id: p.id, reason: None }).await.unwrap();
    assert_eq!(a.status, MergeProposalStatus::Aborted);
    assert_eq!(h.rev("main"), main_before);
    assert!(!Path::new(&wt).exists(), "integration worktree removed");
    assert!(!h.branches().contains("orkestr/integration-"));
    assert!(h.mm.abort(MergeAbortParams { proposal_id: p.id, reason: None }).await.is_err());
}

#[tokio::test]
async fn blockers_dirty_worktrees_unfinished_tasks_and_run_collection() {
    let Some(h) = harness(Arc::new(NoGateSource), None).await else { return };
    let done = h.task("Finished", vec![]).await;
    h.complete(done).await;
    let open = h.task("Still running", vec![]).await;
    h.branch("agent/done-1", "main", &[("d.txt", "d\n")]);
    h.branch("agent/open-1", "main", &[("o.txt", "o\n")]);
    // A worktree with uncommitted work on the finished branch.
    let wt = h._dir.path().join("wt-done");
    git(&["worktree", "add", "-q", wt.to_str().unwrap(), "agent/done-1"], &h.root);
    std::fs::write(wt.join("d.txt"), "changed but not committed\n").unwrap();

    // Collect branches from a Manager run.
    let run = ManagerRun {
        id: ManagerRunId::new(),
        project_id: h.project,
        plan_id: ManagerPlanId::new(),
        goal: "demo".into(),
        status: ManagerRunStatus::Completed,
        status_reason: None,
        limits: RunLimits {
            max_parallel_agents: 2,
            max_agents: 4,
            max_cost_usd: None,
            time_limit_mins: None,
            max_consecutive_failures: 3,
            max_attempts_per_task: 2,
        },
        approval_policy: ApprovalMode::SemiAuto,
        tasks: [(done, "agent/done-1"), (open, "agent/open-1")]
            .into_iter()
            .map(|(id, b)| RunTask {
                task_id: id,
                key: b.into(),
                title: b.into(),
                role: AgentRole::Backend,
                status: TaskStatus::Completed,
                agent_id: None,
                attempts: 1,
                branch: Some(b.into()),
                worktree_path: None,
                cost_usd: 0.0,
                files_changed: Vec::new(),
                last_error: None,
                awaiting_review: false,
                summary: None,
                pipeline: None,
            })
            .collect(),
        agents_spawned: 2,
        peak_parallel: 2,
        consecutive_failures: 0,
        cost_usd: 0.0,
        active_secs: 1,
        started_at: now(),
        updated_at: now(),
        finished_at: Some(now()),
        report: None,
    };
    crate::store::ManagerStore::new(h.db.clone()).save_run(&run).await.unwrap();
    let p =
        h.mm.propose(MergeProposeParams {
            project_id: h.project,
            run_id: Some(run.id),
            branches: Some(vec![MergeBranchInput { branch: "agent/ghost".into(), task_id: None }]),
            target: None,
        })
        .await
        .unwrap();
    assert_eq!(p.run_id, Some(run.id));
    assert_eq!(p.branches.len(), 2);
    let kinds: Vec<(MergeBlockerKind, Option<String>)> =
        p.blockers.iter().map(|b| (b.kind, b.branch.clone())).collect();
    assert!(kinds.contains(&(MergeBlockerKind::UncommittedChanges, Some("agent/done-1".into()))), "{kinds:?}");
    assert!(kinds.contains(&(MergeBlockerKind::TaskNotCompleted, Some("agent/open-1".into()))), "{kinds:?}");
    assert!(kinds.contains(&(MergeBlockerKind::BranchMissing, Some("agent/ghost".into()))), "{kinds:?}");
    assert_eq!(p.recommendation, MergeRecommendation::Blocked);
    let err = h.mm.execute(exec(p.id)).await.unwrap_err();
    assert!(err.to_string().contains("blocked"), "{err}");
    let listed = h.mm.list(MergeListParams { project_id: h.project, run_id: Some(run.id), limit: None }).await.unwrap();
    assert_eq!(listed.len(), 1);

    // A dirty target checkout blocks too (the final fast-forward would be refused).
    std::fs::write(h.root.join("README.md"), "dirty\n").unwrap();
    let p2 = h.propose(&[("agent/done-1", Some(done))]).await;
    assert!(p2.blockers.iter().any(|b| b.kind == MergeBlockerKind::TargetDirty));
}

#[tokio::test]
async fn cleanup_keeps_a_branch_that_moved_after_the_analysis() {
    let Some(h) = harness(Arc::new(NoGateSource), None).await else { return };
    h.policy(|p| {
        p.require_review = false;
        p.require_tests = false;
        p.cleanup = orkestr_protocol::methods::merge::MergeCleanup::RemoveWorktreesAndBranches;
    })
    .await;
    h.branch("feat/a", "main", &[("a.txt", "a\n")]);
    h.branch("feat/b", "main", &[("b.txt", "b\n")]);
    let p = h.propose(&[("feat/a", None), ("feat/b", None)]).await;
    let p = h.mm.execute(exec(p.id)).await.unwrap();
    assert_eq!(p.status, MergeProposalStatus::AwaitingApproval);
    // While the approval is pending the user commits more work on feat/b.
    git(&["checkout", "-q", "feat/b"], &h.root);
    std::fs::write(h.root.join("b2.txt"), "later fix\n").unwrap();
    git(&["add", "-A"], &h.root);
    git(&["commit", "-q", "-m", "later fix"], &h.root);
    git(&["checkout", "-q", "main"], &h.root);
    let later = h.rev("feat/b");
    h.approve(&p).await;
    let done = h.wait_status(p.id, &[MergeProposalStatus::Completed, MergeProposalStatus::Failed]).await;
    assert_eq!(done.status, MergeProposalStatus::Completed, "{:?}", done.status_detail);
    let branches = h.branches();
    assert!(!branches.contains("feat/a"), "the merged, unchanged branch is cleaned up: {branches}");
    assert!(branches.contains("feat/b"), "the moved branch is kept: {branches}");
    assert_eq!(h.rev("feat/b"), later, "its unmerged commit survives");
}
