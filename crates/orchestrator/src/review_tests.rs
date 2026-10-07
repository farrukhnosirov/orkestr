//! Review / adversarial review / consensus tests over real engines (SQLite,
//! task engine, memory, git when installed) with the shipped offline Mock
//! provider (scripted review behaviours, `mock_review.rs`) driven through the
//! real fallback executor.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_agent_runtime::{AgentRuntime, ModelGateway, ProviderInfo, RuntimeConfig, RuntimeDeps};
use orkestr_domain::{
    now, ManagerConfig, ModelRef, Project, ProjectId, ProviderId, ProviderKind, ReviewState, Task, TaskActor,
    TaskStatus, WorktreeRef,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::consensus::{ConsensusStatus, ListConsensusParams, ProposalStatus, RunConsensusParams};
use orkestr_protocol::methods::reviews::{
    AdversarialReviewParams, FindingSeverity, FindingStatus, ListReviewsParams, RequestReviewParams,
    ResolveFindingParams, ReviewKind, ReviewStatus, ReviewVerdict,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_providers::adapters::mock::{MockAdapter, MockOptions};
use orkestr_providers::{
    chat_with_fallback, AdapterSource, AttemptOutcome, ChatRequest, ChatStream, FallbackOptions, HandoffHook,
    ProviderAdapter, ProviderEvent, RouteRequest,
};
use orkestr_security::Security;
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::deps::{ManagerHub, ManagerTunables, OrchestratorDeps, ProviderBrief, Unconstrained};
use crate::{Manager, OrchestratorError};

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

/// Routes to a fixed chain minus the excluded models (like the real router).
struct Gateway {
    source: Arc<Source>,
    chain: Mutex<Vec<ModelRef>>,
}

#[async_trait]
impl ModelGateway for Gateway {
    async fn route(&self, req: &RouteRequest) -> Result<Vec<ModelRef>, String> {
        let chain: Vec<ModelRef> = self.chain.lock().iter().filter(|m| !req.exclude.contains(m)).cloned().collect();
        if chain.is_empty() {
            Err("no usable model".into())
        } else {
            Ok(chain)
        }
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

struct Hub {
    cfg: Mutex<ManagerConfig>,
}

#[async_trait]
impl ManagerHub for Hub {
    async fn manager_config(&self) -> Result<ManagerConfig, String> {
        Ok(self.cfg.lock().clone())
    }
    fn providers(&self) -> Vec<ProviderBrief> {
        Vec::new()
    }
    fn supports_structured_output(&self, _model: &ModelRef) -> bool {
        true
    }
    fn provider_slots(&self) -> BTreeMap<String, usize> {
        BTreeMap::new()
    }
}

struct H {
    _dir: tempfile::TempDir,
    root: PathBuf,
    events: EventBus,
    tasks: Arc<orkestr_task_engine::TaskService>,
    memory: orkestr_memory::MemoryEngine,
    mgr: Manager,
    project: ProjectId,
    source: Arc<Source>,
    gateway: Arc<Gateway>,
    hub: Arc<Hub>,
    git: bool,
}

fn git_available() -> bool {
    std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(args: &[&str], dir: &Path) {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn snapshots(db: &Db, dir: &Path) -> Arc<orkestr_snapshots::SnapshotService> {
    let objects = Arc::new(
        orkestr_storage::FsObjectStore::open(dir.join("objects"), orkestr_storage::Compression::None).unwrap(),
    );
    Arc::new(orkestr_snapshots::SnapshotService::new(
        db.clone(),
        objects as Arc<dyn orkestr_storage::ObjectStore>,
        None,
        orkestr_snapshots::ScanOptions::default(),
    ))
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# Demo project\n").unwrap();
    let git_ok = git_available();
    if git_ok {
        git(&["init", "-q", "-b", "main"], &root);
        git(&["config", "user.name", "Test"], &root);
        git(&["config", "user.email", "test@example.com"], &root);
        git(&["add", "-A"], &root);
        git(&["commit", "-q", "-m", "init"], &root);
    }
    let db = Db::open(dir.path().join("o.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_task_engine::task_migrations());
    migs.extend(orkestr_vcs::vcs_migrations());
    migs.extend(orkestr_snapshots::snapshot_migrations());
    migs.extend(orkestr_terminal::terminal_migrations());
    migs.extend(orkestr_memory::memory_migrations());
    migs.extend(orkestr_cost::cost_migrations());
    migs.extend(orkestr_agent_runtime::agent_migrations());
    migs.extend(crate::orchestrator_migrations());
    migs.extend(crate::review_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(16_384);
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
    meta.insert(&p).await.unwrap();
    let tasks = Arc::new(orkestr_task_engine::TaskService::new(db.clone(), events.clone()));
    let source = Arc::new(Source::default());
    let gateway = Arc::new(Gateway { source: Arc::clone(&source), chain: Mutex::new(Vec::new()) });
    let vcs =
        || orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.path().join("worktrees"));
    let memory = orkestr_memory::MemoryEngine::new(db.clone()).with_events(events.clone());
    let cost = orkestr_cost::CostEngine::new(db.clone()).with_events(events.clone());
    let rt = AgentRuntime::new(RuntimeDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&sec),
        tasks: Arc::clone(&tasks),
        projects: Arc::clone(&meta) as Arc<dyn ProjectRepository>,
        vcs: vcs(),
        snapshots: snapshots(&db, dir.path()),
        terminals: orkestr_terminal::TerminalService::builder().build(),
        memory: memory.clone(),
        cost: cost.clone(),
        models: Arc::clone(&gateway) as Arc<dyn ModelGateway>,
        config: RuntimeConfig::default(),
    });
    rt.init().await.unwrap();
    let hub = Arc::new(Hub { cfg: Mutex::new(ManagerConfig::default()) });
    let mgr = Manager::new(OrchestratorDeps {
        db: db.clone(),
        events: events.clone(),
        security: sec,
        tasks: Arc::clone(&tasks),
        projects: meta as Arc<dyn ProjectRepository>,
        vcs: vcs(),
        snapshots: snapshots(&db, dir.path()),
        memory: memory.clone(),
        cost,
        agents: rt,
        models: Arc::clone(&gateway) as Arc<dyn ModelGateway>,
        hub: Arc::clone(&hub) as Arc<dyn ManagerHub>,
        resources: Arc::new(Unconstrained),
        config: ManagerTunables { state_root: dir.path().join("data"), ..ManagerTunables::default() },
    });
    mgr.init().await.unwrap();
    H { _dir: dir, root, events, tasks, memory, mgr, project: p.id, source, gateway, hub, git: git_ok }
}

impl H {
    /// Registers a Mock provider and returns its model.
    fn provider(&self) -> ModelRef {
        let pid = ProviderId::new();
        self.source.adapters.lock().insert(
            pid,
            Arc::new(MockAdapter::new(pid, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
        );
        ModelRef { provider_id: pid, model: "mock-smart".into() }
    }

    /// Routing chain + Manager model.
    fn route(&self, chain: &[ModelRef]) {
        *self.gateway.chain.lock() = chain.to_vec();
        self.hub.cfg.lock().model = chain.first().cloned();
    }

    /// A task in `Review` whose agent (`author`) committed `file` = `content`
    /// on its own branch in a worktree. Without git: no worktree.
    async fn task_in_review(&self, key: &str, file: &str, content: &str, author: &ModelRef) -> Task {
        let task = self
            .tasks
            .create(
                CreateTaskParams {
                    project_id: self.project,
                    title: format!("Implement {key}"),
                    description: Some("Add the module.".into()),
                    acceptance_criteria: Some(vec!["The module exists".into(), "It is documented".into()]),
                    ..CreateTaskParams::default()
                },
                TaskActor::User,
            )
            .await
            .unwrap();
        let mut worktree = None;
        if self.git {
            let branch = format!("agent/{key}");
            let wt = self.root.parent().unwrap().join(format!("wt-{key}"));
            git(&["worktree", "add", "-q", "-b", &branch, wt.to_str().unwrap()], &self.root);
            let path = wt.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            git(&["add", "-A"], &wt);
            git(&["commit", "-q", "-m", &format!("feat: {key}")], &wt);
            worktree = Some(WorktreeRef { branch, path: wt.to_string_lossy().into_owned() });
        } else {
            let path = self.root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        self.tasks
            .record_agent_run(
                task.id,
                orkestr_task_engine::AgentRunUpdate {
                    model: Some(author.clone()),
                    worktree,
                    ..orkestr_task_engine::AgentRunUpdate::default()
                },
            )
            .await
            .unwrap();
        for to in [TaskStatus::Ready, TaskStatus::Running, TaskStatus::Review] {
            self.tasks.transition(task.id, to, TaskActor::Manager, None, None).await.unwrap();
        }
        self.tasks.get(task.id).await.unwrap()
    }
}

async fn drain(sub: &mut orkestr_events::Subscription) -> Vec<EventKind> {
    let mut out = Vec::new();
    while let Ok(Some(orkestr_events::Received::Event(ev))) =
        tokio::time::timeout(Duration::from_millis(50), sub.recv()).await
    {
        out.push(ev.kind.clone());
    }
    out
}

const CLEAN: &str = "export function hello(name) {\n  return `Hello, ${name}!`;\n}\n";
const LEAKY: &str = "const password = \"hunter2\";\nexport function login() {\n  return eval(\"1+1\");\n}\n";

#[tokio::test]
async fn approved_review_moves_the_task_to_testing_and_ticks_criteria() {
    let h = harness().await;
    let m = h.provider();
    h.route(std::slice::from_ref(&m));
    let task = h.task_in_review("hello", "src/hello.js", CLEAN, &m).await;
    let mut sub = h.events.subscribe([orkestr_protocol::topic::project(h.project)]).unwrap();
    let res = h
        .mgr
        .request_review(RequestReviewParams { task_id: task.id, wait: Some(true), ..RequestReviewParams::default() })
        .await
        .unwrap();
    let r = res.review;
    assert_eq!(r.status, ReviewStatus::Finished, "{:?}", r.error);
    assert_eq!(r.kind, ReviewKind::Standard);
    assert_eq!(r.verdict, Some(ReviewVerdict::Approve), "{}", r.summary);
    assert_eq!(r.ac_check.len(), 2);
    assert!(r.ac_check.iter().all(|c| c.met), "{:?}", r.ac_check);
    assert_eq!(r.transitioned_to, Some(TaskStatus::Testing));
    assert_eq!(r.diff.files, 1, "{:?}", r.diff);
    let t = h.tasks.get(task.id).await.unwrap();
    assert_eq!(t.status, TaskStatus::Testing);
    assert_eq!(t.review_state, ReviewState::Approved);
    assert!(t.acceptance_criteria.iter().all(|c| c.met));
    // The reviewer's spend is the task's spend (run / healing budgets read it).
    assert!(r.usage.input_tokens > 0, "{:?}", r.usage);
    assert_eq!(t.usage.input_tokens - task.usage.input_tokens, r.usage.input_tokens, "{:?}", t.usage);
    assert_eq!(t.usage.output_tokens - task.usage.output_tokens, r.usage.output_tokens);
    let kinds = drain(&mut sub).await;
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ReviewUpdated(_))));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ReviewFinished(p) if p.verdict == ReviewState::Approved)));
    let gate = h.mgr.review_gate(task.id).await.unwrap();
    assert!(gate.passed && gate.reviewer_approved && gate.no_critical_security, "{gate:?}");
    let list =
        h.mgr.list_reviews(ListReviewsParams { task_id: Some(task.id), ..ListReviewsParams::default() }).await.unwrap();
    assert_eq!(list.reviews.len(), 1);
    assert!(list.gate.unwrap().passed);
    assert!(h.mgr.review_feedback(task.id).await.unwrap().is_none());
}

#[tokio::test]
async fn critical_findings_request_changes_and_block_the_gate_until_resolved() {
    let h = harness().await;
    let m = h.provider();
    h.route(std::slice::from_ref(&m));
    let task = h.task_in_review("login", "src/login.js", LEAKY, &m).await;
    let r = h
        .mgr
        .request_review(RequestReviewParams { task_id: task.id, wait: Some(true), ..RequestReviewParams::default() })
        .await
        .unwrap()
        .review;
    assert_eq!(r.verdict, Some(ReviewVerdict::RequestChanges));
    let critical: Vec<_> = r.findings.iter().filter(|f| f.severity == FindingSeverity::Critical).collect();
    assert_eq!(critical.len(), 2, "{:?}", r.findings);
    assert!(critical.iter().all(|f| f.category == "security" && f.file.as_deref() == Some("src/login.js")));
    if h.git {
        assert_eq!(critical[0].line, Some(1));
    }
    assert_eq!(h.tasks.get(task.id).await.unwrap().status, TaskStatus::Running, "changes requested → Running");
    let gate = h.mgr.review_gate(task.id).await.unwrap();
    assert!(!gate.passed && !gate.no_critical_security && gate.blocking_findings == 2);
    let feedback = h.mgr.review_feedback(task.id).await.unwrap().unwrap();
    assert!(feedback.contains("src/login.js"), "{feedback}");
    // Dismissing a critical finding needs a note; fixing does not.
    let id0 = critical[0].id.clone();
    let err = h
        .mgr
        .resolve_finding(ResolveFindingParams {
            review_id: r.id,
            finding_id: id0.clone(),
            status: FindingStatus::Dismissed,
            note: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, OrchestratorError::Invalid(_)));
    let after = h
        .mgr
        .resolve_finding(ResolveFindingParams {
            review_id: r.id,
            finding_id: id0,
            status: FindingStatus::Dismissed,
            note: Some("test fixture".into()),
        })
        .await
        .unwrap()
        .review;
    let id1 = critical[1].id.clone();
    h.mgr
        .resolve_finding(ResolveFindingParams {
            review_id: after.id,
            finding_id: id1,
            status: FindingStatus::Fixed,
            note: None,
        })
        .await
        .unwrap();
    let gate = h.mgr.review_gate(task.id).await.unwrap();
    assert_eq!(gate.blocking_findings, 0);
    assert!(gate.no_critical_security && !gate.reviewer_approved && !gate.passed);
}

#[tokio::test]
async fn adversarial_review_uses_another_provider_and_verifies_with_the_author() {
    let h = harness().await;
    let author = h.provider();
    let adversary = h.provider();
    h.route(&[author.clone(), adversary.clone()]);
    let task = h.task_in_review("auth", "src/auth.js", LEAKY, &author).await;
    let r = h
        .mgr
        .adversarial_review(AdversarialReviewParams { task_id: task.id, model: None, wait: Some(true) })
        .await
        .unwrap()
        .review;
    assert_eq!(r.status, ReviewStatus::Finished, "{:?}", r.error);
    assert_eq!(r.kind, ReviewKind::Adversarial);
    assert_eq!(r.model.as_ref(), Some(&adversary), "a different provider attacks");
    assert!(!r.same_model_fallback);
    assert_eq!(r.verifier_model.as_ref(), Some(&author), "the author verifies");
    // Critical findings with evidence are conceded; the generic edge case is rebutted.
    assert!(r.findings.iter().any(|f| f.severity == FindingSeverity::Critical && f.status == FindingStatus::Confirmed));
    assert!(r.findings.iter().any(|f| f.category == "edge-case" && f.status == FindingStatus::Rebutted));
    assert!(r.findings.iter().all(|f| f.response.is_some()));
    assert_eq!(r.verdict, Some(ReviewVerdict::RequestChanges));
    // A standalone adversarial review does not move the task, but blocks the gate.
    assert_eq!(h.tasks.get(task.id).await.unwrap().status, TaskStatus::Review);
    let gate = h.mgr.review_gate(task.id).await.unwrap();
    assert!(!gate.passed && !gate.no_critical_security);
    assert_eq!(gate.latest_adversarial_id, Some(r.id));
}

#[tokio::test]
async fn adversarial_review_falls_back_to_the_author_model_and_combines_with_the_review() {
    let h = harness().await;
    let only = h.provider();
    h.route(std::slice::from_ref(&only));
    let task = h.task_in_review("clean", "src/clean.js", CLEAN, &only).await;
    let res = h
        .mgr
        .request_review(RequestReviewParams {
            task_id: task.id,
            adversarial: Some(true),
            wait: Some(true),
            ..RequestReviewParams::default()
        })
        .await
        .unwrap();
    let adv = res.adversarial.expect("adversarial review ran too");
    assert!(adv.same_model_fallback, "only the author's model was available");
    assert_eq!(adv.linked_review_id, Some(res.review.id));
    assert_eq!(res.review.linked_review_id, Some(adv.id));
    // The adversary always challenges something, but nothing critical here.
    assert!(!adv.findings.is_empty());
    assert!(adv.blocking().is_empty());
    assert_eq!(res.review.verdict, Some(ReviewVerdict::Approve));
    assert_eq!(h.tasks.get(task.id).await.unwrap().status, TaskStatus::Testing);
    // get() returns the linked adversarial review.
    let got = h.mgr.get_review(res.review.id).await.unwrap();
    assert_eq!(got.adversarial.map(|a| a.id), Some(adv.id));
}

#[tokio::test]
async fn combined_review_with_blocking_adversarial_findings_requests_changes() {
    let h = harness().await;
    let author = h.provider();
    let adversary = h.provider();
    h.route(&[author.clone(), adversary.clone()]);
    // Leaky code: the reviewer requests changes and the adversary's conceded critical findings block.
    let task = h.task_in_review("combo", "src/combo.js", LEAKY, &author).await;
    let res = h
        .mgr
        .request_review(RequestReviewParams {
            task_id: task.id,
            adversarial: Some(true),
            wait: Some(true),
            ..RequestReviewParams::default()
        })
        .await
        .unwrap();
    assert_eq!(res.review.verdict, Some(ReviewVerdict::RequestChanges));
    assert_eq!(h.tasks.get(task.id).await.unwrap().status, TaskStatus::Running);
    assert_eq!(h.tasks.get(task.id).await.unwrap().review_state, ReviewState::ChangesRequested);
}

#[tokio::test]
async fn second_review_of_the_same_task_is_a_conflict_while_one_runs() {
    let h = harness().await;
    let m = h.provider();
    h.route(std::slice::from_ref(&m));
    let task = h.task_in_review("busy", "src/busy.js", CLEAN, &m).await;
    let _hold = crate::review::InFlight::acquire(task.id, false).unwrap();
    let err = h
        .mgr
        .request_review(RequestReviewParams { task_id: task.id, ..RequestReviewParams::default() })
        .await
        .unwrap_err();
    assert!(matches!(err, OrchestratorError::Conflict(_)), "{err}");
}

#[tokio::test]
async fn consensus_runs_participants_in_parallel_and_records_the_decision() {
    let h = harness().await;
    let a = h.provider();
    let b = h.provider();
    h.route(&[a.clone(), b.clone()]);
    let mut sub = h.events.subscribe([orkestr_protocol::topic::project(h.project)]).unwrap();
    let run = h
        .mgr
        .run_consensus(RunConsensusParams {
            project_id: h.project,
            question: "How should we cache API responses?".into(),
            participants: Some(3),
            purpose: Some("Security".into()),
            wait: Some(true),
            ..RunConsensusParams::default()
        })
        .await
        .unwrap()
        .run;
    assert_eq!(run.status, ConsensusStatus::Finished, "{:?}", run.error);
    assert_eq!(run.proposals.len(), 3);
    // Distinct providers first, then reuse with a warning.
    assert_eq!(run.proposals[0].model, a);
    assert_eq!(run.proposals[1].model, b);
    assert!(!run.warnings.is_empty());
    assert!(run.proposals.iter().all(|p| p.status == ProposalStatus::Done && p.score.is_some()));
    let titles: std::collections::BTreeSet<&str> = run.proposals.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles.len(), 3, "independent perspectives give different proposals");
    let decision = run.decision.clone().unwrap();
    // Mock participant 2 has the highest confidence.
    assert_eq!(decision.chosen_index, Some(1));
    let id = run.decision_id.expect("recorded in the Decision Log");
    let d = h.memory.get_decision(id).await.unwrap();
    assert_eq!(d.alternatives.len(), 3);
    assert_eq!(d.alternatives.iter().filter(|a| a.rejected_reason.is_none()).count(), 1);
    assert_eq!(d.decided_by, orkestr_domain::Actor::Manager);
    let kinds = drain(&mut sub).await;
    assert!(kinds.iter().any(|k| matches!(k, EventKind::DecisionRecorded(_))));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, EventKind::ConsensusUpdated(c) if c.run.status == ConsensusStatus::Finished)));
    let list = h.mgr.list_consensus(ListConsensusParams { project_id: h.project, limit: None }).await.unwrap();
    assert_eq!(list.runs.len(), 1);
    assert_eq!(h.mgr.get_consensus(run.id).await.unwrap().run.id, run.id);
}

#[tokio::test]
async fn consensus_validates_participants_and_fails_with_too_few_proposals() {
    let h = harness().await;
    let a = h.provider();
    h.route(std::slice::from_ref(&a));
    let one = h
        .mgr
        .run_consensus(RunConsensusParams {
            project_id: h.project,
            question: "q?".into(),
            models: Some(vec![a.clone()]),
            ..RunConsensusParams::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(one, OrchestratorError::Invalid(_)));
    let empty = h
        .mgr
        .run_consensus(RunConsensusParams {
            project_id: h.project,
            question: "  ".into(),
            ..RunConsensusParams::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(empty, OrchestratorError::Invalid(_)));
    // A failing participant: only one proposal succeeds → Failed, nothing recorded.
    let pid = ProviderId::new();
    h.source.adapters.lock().insert(
        pid,
        Arc::new(MockAdapter::new(pid, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
    );
    let broken = ModelRef { provider_id: pid, model: "mock-server-error".into() };
    let run = h
        .mgr
        .run_consensus(RunConsensusParams {
            project_id: h.project,
            question: "Pick a database".into(),
            models: Some(vec![a, broken]),
            record_decision: Some(true),
            wait: Some(true),
            ..RunConsensusParams::default()
        })
        .await
        .unwrap()
        .run;
    assert_eq!(run.status, ConsensusStatus::Failed);
    assert_eq!(run.proposals[1].status, ProposalStatus::Failed);
    assert!(run.decision_id.is_none());
    assert!(h.memory.list_decisions(h.project, None, None).await.unwrap().is_empty());
}

#[tokio::test]
async fn unfinished_reviews_are_failed_on_recovery() {
    let h = harness().await;
    let m = h.provider();
    h.route(std::slice::from_ref(&m));
    let task = h.task_in_review("rec", "src/rec.js", CLEAN, &m).await;
    let doc = h.mgr.inner.new_review(&task, ReviewKind::Standard, Some(m));
    h.mgr.inner.save_review(&doc).await;
    h.mgr.recover_reviews().await.unwrap();
    let r = h.mgr.get_review(doc.id).await.unwrap().review;
    assert_eq!(r.status, ReviewStatus::Failed);
    assert!(r.error.is_some());
}
