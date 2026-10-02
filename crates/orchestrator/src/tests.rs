//! Manager tests over real engines (SQLite, security, task engine, agent
//! runtime, terminal, snapshots, memory, cost, git when installed) with the
//! shipped offline Mock provider driven through the real fallback executor.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_agent_runtime::{AgentRuntime, ModelGateway, ProviderInfo, RuntimeConfig, RuntimeDeps};
use orkestr_domain::ApprovalStatus;
use orkestr_domain::{
    now, AgentRole, ApprovalMode, Capability, ManagerConfig, ManagerPlanId, ModelRef, PermissionEffect, PermissionRule,
    PermissionScope, Project, ProjectId, ProviderId, ProviderKind, TaskPriority, TaskStatus,
};
use orkestr_events::EventBus;
use orkestr_protocol::methods::manager::{
    ApprovePlanParams, ManagerChatParams, ManagerHistoryParams, ManagerPlan, ManagerPlanParams, ManagerRequestOptions,
    ManagerRunParams, ManagerRunStatus, PlanEdits, PlanSource, PlanStatus, PlanTask, PlanTaskTitleEdit,
    RejectPlanParams, RunLimitsPatch,
};
use orkestr_providers::adapters::mock::{MockAdapter, MockOptions};
use orkestr_providers::{
    chat_with_fallback, AdapterSource, AttemptOutcome, ChatRequest, ChatStream, FallbackOptions, HandoffHook,
    ProviderAdapter, ProviderEvent, RouteRequest,
};
use orkestr_security::{CheckContext, Decision, PermissionEngine, Resource, Security};
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use orkestr_terminal::{CommandGate, GateDecision, GateRequest};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::deps::{ManagerHub, ManagerTunables, OrchestratorDeps, ProviderBrief, Unconstrained};
use crate::Manager;

// ---------------------------------------------------------------------------
// provider plumbing
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

/// Wraps the Mock so that agents whose task title contains one of `group`
/// hold their first model call until `need` group members are in flight at
/// once (or 20 s pass). Makes "these tasks ran in parallel" deterministic:
/// an agent cannot finish before its sibling was admitted.
struct GatedMock {
    inner: MockAdapter,
    group: Vec<String>,
    need: usize,
    arrived: Mutex<std::collections::BTreeSet<String>>,
    notify: tokio::sync::Notify,
}

impl GatedMock {
    fn title(req: &ChatRequest) -> Option<String> {
        req.messages
            .iter()
            .flat_map(|m| m.content.lines())
            .find_map(|l| l.strip_prefix("# Task:"))
            .map(|t| t.trim().to_owned())
    }
}

#[async_trait]
impl ProviderAdapter for GatedMock {
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
        let agent = request.tools.iter().any(|t| t.name == "submit_result");
        if let Some(title) =
            Self::title(&request).filter(|t| agent && self.group.iter().any(|g| t.contains(g.as_str())))
        {
            self.arrived.lock().insert(title);
            self.notify.notify_waiters();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let notified = self.notify.notified();
                if self.arrived.lock().len() >= self.need || tokio::time::Instant::now() >= deadline {
                    break;
                }
                let _ = tokio::time::timeout_at(deadline, notified).await;
            }
        }
        self.inner.chat(request, cancel).await
    }
}

struct Gateway {
    source: Arc<Source>,
    chain: Mutex<Vec<ModelRef>>,
}

#[async_trait]
impl ModelGateway for Gateway {
    async fn route(&self, _req: &RouteRequest) -> Result<Vec<ModelRef>, String> {
        Ok(self.chain.lock().clone())
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

struct TestHub {
    cfg: Mutex<ManagerConfig>,
    structured: bool,
}

#[async_trait]
impl ManagerHub for TestHub {
    async fn manager_config(&self) -> Result<ManagerConfig, String> {
        Ok(self.cfg.lock().clone())
    }
    fn providers(&self) -> Vec<ProviderBrief> {
        Vec::new()
    }
    fn supports_structured_output(&self, _model: &ModelRef) -> bool {
        self.structured
    }
    fn provider_slots(&self) -> BTreeMap<String, usize> {
        BTreeMap::new()
    }
}

// ---------------------------------------------------------------------------
// terminal gate (same policy as the core's SecurityCommandGate, SemiAuto)
// ---------------------------------------------------------------------------

struct TestGate {
    sec: Arc<Security>,
}

#[async_trait]
impl CommandGate for TestGate {
    async fn check(&self, r: &GateRequest) -> GateDecision {
        let agent = r.agent_id();
        let actor = agent.map_or_else(|| self.sec.user(), orkestr_domain::Actor::Agent);
        let shell = match r.shell {
            orkestr_terminal::ShellKind::PowerShell7 | orkestr_terminal::ShellKind::WindowsPowerShell => {
                orkestr_security::ShellKind::PowerShell
            }
            orkestr_terminal::ShellKind::Cmd => orkestr_security::ShellKind::Cmd,
            _ => orkestr_security::ShellKind::Bash,
        };
        let ctx = CheckContext {
            project_id: r.project_id,
            agent_id: agent,
            task_id: r.task_id,
            approval_mode: ApprovalMode::SemiAuto,
            worktree_root: Some(r.cwd.clone()),
            protected_environment: false,
        };
        let res = Resource::Command { command: r.command.clone(), shell, cwd: Some(".".into()) };
        match self.sec.engine.check(&actor, Capability::Terminal, &res, &ctx) {
            Decision::Allow => GateDecision::Allow,
            Decision::Deny(d) => GateDecision::Deny { reason: d.message },
            Decision::Ask(a) => GateDecision::Deny { reason: format!("test gate: approval needed ({})", a.message) },
        }
    }
}

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct H {
    dir: tempfile::TempDir,
    root: PathBuf,
    db: Db,
    events: EventBus,
    sec: Arc<Security>,
    tasks: Arc<orkestr_task_engine::TaskService>,
    meta: Arc<SqliteMetadataStore>,
    rt: AgentRuntime,
    mgr: Manager,
    project: ProjectId,
    source: Arc<Source>,
    gateway: Arc<Gateway>,
    hub: Arc<TestHub>,
    git: bool,
}

fn git_available() -> bool {
    std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn node_available() -> bool {
    std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(args: &[&str], dir: &Path) {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn manager_deps(h: &H, models: Arc<dyn ModelGateway>) -> OrchestratorDeps {
    build_deps(h.dir.path(), &h.db, &h.events, &h.sec, &h.tasks, &h.meta, &h.rt, &h.hub, models)
}

#[allow(clippy::too_many_arguments)]
fn build_deps(
    dir: &Path,
    db: &Db,
    events: &EventBus,
    sec: &Arc<Security>,
    tasks: &Arc<orkestr_task_engine::TaskService>,
    meta: &Arc<SqliteMetadataStore>,
    rt: &AgentRuntime,
    hub: &Arc<TestHub>,
    models: Arc<dyn ModelGateway>,
) -> OrchestratorDeps {
    OrchestratorDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(sec),
        tasks: Arc::clone(tasks),
        projects: Arc::clone(meta) as Arc<dyn ProjectRepository>,
        vcs: orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.join("worktrees")),
        snapshots: snapshots(db, dir),
        memory: orkestr_memory::MemoryEngine::new(db.clone()).with_events(events.clone()),
        cost: orkestr_cost::CostEngine::new(db.clone()).with_events(events.clone()),
        agents: rt.clone(),
        models,
        hub: Arc::clone(hub) as Arc<dyn ManagerHub>,
        resources: Arc::new(Unconstrained),
        config: ManagerTunables {
            state_root: dir.join("data"),
            tick: Duration::from_millis(40),
            ..ManagerTunables::default()
        },
    }
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

async fn harness(with_git: bool, policy: ApprovalMode) -> H {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# Demo project\n").unwrap();
    let git_ok = with_git && git_available();
    if git_ok {
        git(&["init", "-q", "-b", "main"], &root);
        git(&["-c", "user.name=t", "-c", "user.email=t@e", "add", "-A"], &root);
        git(&["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-q", "-m", "init"], &root);
        git(&["config", "user.name", "Test"], &root);
        git(&["config", "user.email", "test@example.com"], &root);
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
    p.approval_mode = ApprovalMode::SemiAuto;
    meta.insert(&p).await.unwrap();
    let tasks = Arc::new(orkestr_task_engine::TaskService::new(db.clone(), events.clone()));
    let terminals =
        orkestr_terminal::TerminalService::builder().gate(Arc::new(TestGate { sec: Arc::clone(&sec) })).build();
    let source = Arc::new(Source::default());
    let gateway = Arc::new(Gateway { source: Arc::clone(&source), chain: Mutex::new(Vec::new()) });
    let rt = AgentRuntime::new(RuntimeDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&sec),
        tasks: Arc::clone(&tasks),
        projects: Arc::clone(&meta) as Arc<dyn ProjectRepository>,
        vcs: orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.path().join("worktrees")),
        snapshots: snapshots(&db, dir.path()),
        terminals,
        memory: orkestr_memory::MemoryEngine::new(db.clone()).with_events(events.clone()),
        cost: orkestr_cost::CostEngine::new(db.clone()).with_events(events.clone()),
        models: Arc::clone(&gateway) as Arc<dyn ModelGateway>,
        config: RuntimeConfig {
            approval_timeout: Duration::from_secs(30),
            command_timeout: Duration::from_secs(60),
            ..RuntimeConfig::default()
        },
    });
    rt.init().await.unwrap();
    let hub = Arc::new(TestHub {
        cfg: Mutex::new(ManagerConfig { approval_policy: policy, ..ManagerConfig::default() }),
        structured: true,
    });
    let mgr = Manager::new(build_deps(
        dir.path(),
        &db,
        &events,
        &sec,
        &tasks,
        &meta,
        &rt,
        &hub,
        Arc::clone(&gateway) as Arc<dyn ModelGateway>,
    ));
    let h = H { dir, root, db, events, sec, tasks, meta, rt, mgr, project: p.id, source, gateway, hub, git: git_ok };
    h.mgr.init().await.unwrap();
    h
}

impl H {
    /// Registers the shipped Mock adapter and routes everything to `model`.
    fn mock(&self, model: &str) -> ModelRef {
        let pid = ProviderId::new();
        self.source.adapters.lock().insert(
            pid,
            Arc::new(MockAdapter::new(pid, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
        );
        let m = ModelRef { provider_id: pid, model: model.into() };
        *self.gateway.chain.lock() = vec![m.clone()];
        self.hub.cfg.lock().model = Some(m.clone());
        m
    }

    /// Like [`H::mock`] with agents of the `group` tasks gated on each other.
    fn mock_gated(&self, model: &str, group: &[&str], need: usize) -> ModelRef {
        let pid = ProviderId::new();
        let inner = MockAdapter::new(pid, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO });
        let gated = GatedMock {
            inner,
            group: group.iter().map(|g| (*g).to_owned()).collect(),
            need,
            arrived: Mutex::new(std::collections::BTreeSet::new()),
            notify: tokio::sync::Notify::new(),
        };
        self.source.adapters.lock().insert(pid, Arc::new(gated));
        let m = ModelRef { provider_id: pid, model: model.into() };
        *self.gateway.chain.lock() = vec![m.clone()];
        self.hub.cfg.lock().model = Some(m.clone());
        m
    }

    async fn plan(&self, goal: &str) -> ManagerPlan {
        self.mgr.plan(ManagerPlanParams { project_id: self.project, goal: goal.into(), options: None }).await.unwrap()
    }

    /// Stores a hand-written proposed plan (no model needed).
    async fn custom_plan(&self, tasks: Vec<PlanTask>) -> ManagerPlan {
        let at = now();
        let plan = ManagerPlan {
            id: ManagerPlanId::new(),
            project_id: self.project,
            conversation_id: None,
            goal: "custom".into(),
            summary: "custom plan".into(),
            assumptions: vec![],
            risks: vec![],
            max_parallelism: crate::plan::max_parallelism(&tasks),
            parallelism: 1,
            tasks,
            estimated_cost_usd: None,
            status: PlanStatus::Proposed,
            source: PlanSource::Structured,
            model: None,
            warnings: vec![],
            options: ManagerRequestOptions::default(),
            run_id: None,
            rejection_reason: None,
            created_at: at,
            updated_at: at,
        };
        self.mgr.store().save_plan(&plan).await.unwrap();
        plan
    }
}

fn ptask(key: &str, title: &str, role: AgentRole, deps: &[&str]) -> PlanTask {
    PlanTask {
        key: key.into(),
        title: title.into(),
        description: String::new(),
        acceptance_criteria: vec!["done".into()],
        role,
        depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
        priority: TaskPriority::Normal,
        epic: None,
        feature: None,
        files: vec![],
        estimated_tokens: None,
        estimated_cost_usd: None,
        task_id: None,
    }
}

fn final_status(s: ManagerRunStatus) -> bool {
    s.is_final()
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_from_mock_creates_tasks_with_dependencies_on_approval() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let plan = h.plan("Create a hello module with tests").await;
    assert_eq!(plan.status, PlanStatus::Proposed);
    assert_eq!(plan.source, PlanSource::Structured);
    assert_eq!(plan.tasks.len(), 4);
    assert_eq!(plan.max_parallelism, 2, "implement ∥ document");
    let keys: Vec<(&str, Vec<String>)> = plan.tasks.iter().map(|t| (t.key.as_str(), t.depends_on.clone())).collect();
    assert_eq!(keys[1], ("t2", vec!["t1".to_owned()]));
    assert_eq!(plan.tasks[3].role, AgentRole::Qa);
    assert_eq!(plan.tasks[2].role, AgentRole::ProductManager);
    assert!(plan.tasks.iter().all(|t| !t.acceptance_criteria.is_empty()));
    assert!(plan.estimated_cost_usd.is_none(), "mock has no price");

    // Conversation records the goal and the Manager's plan message.
    let hist = h.mgr.history(ManagerHistoryParams { project_id: h.project, limit: None }).await.unwrap();
    assert_eq!(hist.messages.len(), 2);
    assert_eq!(hist.messages[1].plan_id, Some(plan.id));
    assert_eq!(hist.plans.len(), 1);

    // Approve with edits: retitle t3, keep the rest; no run.
    let r = h
        .mgr
        .approve_plan(ApprovePlanParams {
            plan_id: plan.id,
            edits: Some(PlanEdits {
                remove_tasks: vec![],
                titles: vec![PlanTaskTitleEdit { key: "t3".into(), title: "Write the usage docs".into() }],
            }),
            start_run: Some(false),
        })
        .await
        .unwrap();
    assert!(r.run.is_none());
    assert_eq!(r.plan.status, PlanStatus::Approved);
    let ids: Vec<_> = r.plan.tasks.iter().map(|t| t.task_id.unwrap()).collect();
    let t2 = h.tasks.get(ids[1]).await.unwrap();
    assert_eq!(t2.dependencies, vec![ids[0]]);
    assert_eq!(t2.status, TaskStatus::Planned);
    assert_eq!(h.tasks.get(ids[2]).await.unwrap().title, "Write the usage docs");
    assert!(t2.files_touched.contains(&"src/hello.mjs".to_owned()));
    let graph = h.tasks.graph(h.project, None).await.unwrap();
    assert_eq!(graph.edges.len(), 3);
    // Deciding twice is a conflict; rejecting an approved plan too.
    assert!(h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: None }).await.is_err());
    assert!(h.mgr.reject_plan(RejectPlanParams { plan_id: plan.id, reason: None }).await.is_err());
}

#[tokio::test]
async fn chat_plans_build_requests_and_answers_from_real_state() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let mut sub = h.events.subscribe([orkestr_protocol::topic::project(h.project)]).unwrap();
    let (res, reply) = h
        .mgr
        .chat_and_wait(ManagerChatParams {
            project_id: h.project,
            text: "Create a hello module with tests".into(),
            attachment: None,
            options: None,
        })
        .await
        .unwrap();
    assert_eq!(reply.id, res.reply_message_id);
    assert!(reply.tool_events.iter().any(|e| e.name == "propose_plan" && !e.is_error), "{:?}", reply.tool_events);
    let plan_id = reply.plan_id.expect("reply references the plan");
    assert_eq!(h.mgr.get_plan(plan_id).await.unwrap().status, PlanStatus::Proposed);
    assert!(reply.text.contains("plan card"), "{}", reply.text);
    assert!(!h.mgr.is_busy(h.project));
    // Streamed deltas + stored message events were published.
    let mut kinds = Vec::new();
    while let Ok(Some(orkestr_events::Received::Event(ev))) =
        tokio::time::timeout(Duration::from_millis(50), sub.recv()).await
    {
        kinds.push(ev.kind_name().to_owned());
    }
    assert!(kinds.iter().any(|k| k == "ManagerMessage"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "ManagerMessageAdded"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "ManagerPlanUpdated"), "{kinds:?}");

    // A status question reads state through a tool.
    let (_, reply) = h
        .mgr
        .chat_and_wait(ManagerChatParams {
            project_id: h.project,
            text: "What's the status?".into(),
            attachment: None,
            options: None,
        })
        .await
        .unwrap();
    assert_eq!(reply.tool_events[0].name, "project_overview");
    assert!(reply.text.contains("project_overview"), "{}", reply.text);

    // A newer proposal supersedes the older one.
    let newer = h.plan("Build the greeting service").await;
    assert_eq!(h.mgr.get_plan(plan_id).await.unwrap().status, PlanStatus::Superseded);
    assert_eq!(newer.status, PlanStatus::Proposed);

    // Clear archives the conversation with a summary.
    let cleared = h.mgr.clear(h.project).await.unwrap();
    let archived = cleared.archived.unwrap();
    assert!(archived.summary.contains("Create a hello module"), "{}", archived.summary);
    let hist = h.mgr.history(ManagerHistoryParams { project_id: h.project, limit: None }).await.unwrap();
    assert!(hist.messages.is_empty());
    assert_eq!(hist.summaries.len(), 1);
}

#[tokio::test]
async fn state_tools_are_permission_checked() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let project = h.meta.get(h.project).await.unwrap().unwrap();
    let ctx = crate::tools::TurnCtx {
        project: project.clone(),
        conversation: orkestr_domain::ConversationId::new(),
        options: ManagerRequestOptions::default(),
        policy: ApprovalMode::SemiAuto,
        cancel: CancellationToken::new(),
    };
    let ok = h.mgr.inner.execute_tool(&ctx, "list_tasks", &serde_json::json!({})).await;
    assert!(!ok.is_error, "{}", ok.content);
    let ok = h.mgr.inner.execute_tool(&ctx, "project_overview", &serde_json::json!({})).await;
    assert!(!ok.is_error && ok.content.contains("\"total\""), "{}", ok.content);
    // A project-scope Read deny rule blocks the Manager's state reads.
    h.sec
        .permissions
        .set(
            None,
            PermissionRule {
                capability: Capability::Read,
                effect: PermissionEffect::Deny,
                resource: None,
                scope: PermissionScope::Project { project_id: h.project },
            },
            Some("test".into()),
            None,
            h.sec.user(),
        )
        .await
        .unwrap();
    let denied = h.mgr.inner.execute_tool(&ctx, "list_tasks", &serde_json::json!({})).await;
    assert!(denied.is_error && denied.content.contains("denied"), "{}", denied.content);
    let unknown = h.mgr.inner.execute_tool(&ctx, "rm_rf", &serde_json::json!({})).await;
    assert!(unknown.is_error);
}

#[tokio::test]
async fn run_executes_the_mock_plan_in_parallel_respecting_dependencies() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    // Implement ∥ document: each holds its first model call until both run.
    h.mock_gated("mock-smart", &["Implement the hello", "Document the hello"], 2);
    let plan = h.plan("Create a hello module with tests").await;
    let r =
        h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    assert_eq!(run.status, ManagerRunStatus::Running);
    let done = h
        .mgr
        .wait_run(run.id, Duration::from_secs(120), |r| {
            final_status(r.status) || r.tasks.iter().any(|t| t.awaiting_review)
        })
        .await
        .unwrap();
    let node = node_available();
    if !h.git || !node {
        // Without git QA cannot see the implementation; without node its test
        // run fails → partial result → waits for review. Both are honest outcomes.
        eprintln!("git={} node={} — run ended {:?}: {:?}", h.git, node, done.status, done.status_reason);
        assert!(done.tasks.iter().take(3).all(|t| t.status == TaskStatus::Completed), "{:#?}", done.tasks);
        return;
    }
    if done.status != ManagerRunStatus::Completed {
        for t in &done.tasks {
            if let Some(a) = t.agent_id {
                let tr = h.rt.transcript(a, None).await.unwrap();
                for c in &tr.tool_calls {
                    eprintln!("{} {} {:?} {:?}", t.key, c.tool_call.tool, c.tool_call.status, c.result);
                }
            }
        }
    }
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?} {:#?}", done.status_reason, done.tasks);
    assert!(done.tasks.iter().all(|t| t.status == TaskStatus::Completed));
    assert_eq!(done.agents_spawned, 4);
    assert_eq!(done.peak_parallel, 2, "implement and document run in parallel");
    // Dependencies: every task started after its prerequisites finished.
    let ids: BTreeMap<String, _> = done.tasks.iter().map(|t| (t.key.clone(), t.task_id)).collect();
    for pt in &plan.tasks {
        let t = h.tasks.get(ids[&pt.key]).await.unwrap();
        for d in &pt.depends_on {
            let dep = h.tasks.get(ids[d]).await.unwrap();
            assert!(dep.finished_at.unwrap() <= t.started_at.unwrap(), "{} started before {d} finished", pt.key);
        }
    }
    // Each agent worked on its own branch; QA's worktree was based on the backend branch.
    let report = done.report.clone().unwrap();
    assert_eq!(report.completed, 4);
    assert_eq!(report.merge_proposals.len(), 4, "{report:#?}");
    let qa = done.tasks.iter().find(|t| t.key == "t4").unwrap();
    let qa_wt = PathBuf::from(qa.worktree_path.as_ref().unwrap());
    assert!(qa_wt.join("src/hello.mjs").is_file(), "QA sees the implementation");
    assert!(qa_wt.join("tests/hello.test.mjs").is_file());
    assert!(report.tests.iter().any(|t| t.contains("node --test") && t.contains("passed")), "{:?}", report.tests);
    assert!(report.summary.contains("Merge proposals"));
    assert!(h.root.join("README.md").is_file() && !h.root.join("src/hello.mjs").exists(), "main checkout untouched");
    let plan = h.mgr.get_plan(plan.id).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Completed);
    // Session resume summary.
    let s = h.mgr.resume_summary(h.project).await.unwrap();
    assert_eq!(s.tasks_completed, 4);
    assert!(s.headline.contains("4 of 4"), "{}", s.headline);
    assert_eq!(s.goals[0].percent, 100);
}

#[tokio::test]
async fn parallelism_is_capped_by_the_run_limit() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    // The first two agents wait for each other; the third starts later.
    h.mock_gated("mock-smart", &["alpha", "beta", "gamma"], 2);
    let plan = h
        .custom_plan(vec![
            ptask("a", "Implement the alpha module", AgentRole::Backend, &[]),
            ptask("b", "Implement the beta module", AgentRole::Backend, &[]),
            ptask("c", "Implement the gamma module", AgentRole::Backend, &[]),
        ])
        .await;
    h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(false) }).await.unwrap();
    let run = h
        .mgr
        .start_run(ManagerRunParams {
            plan_id: Some(plan.id),
            run_id: None,
            limits: Some(RunLimitsPatch { max_parallel_agents: Some(2), ..RunLimitsPatch::default() }),
        })
        .await
        .unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(120), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?} {:#?}", done.status_reason, done.tasks);
    assert_eq!(done.peak_parallel, 2, "three independent tasks, limit 2");
    assert_eq!(done.agents_spawned, 3);
}

#[tokio::test]
async fn failing_agents_are_retried_then_the_run_fails() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-server-error");
    let plan = h
        .custom_plan(vec![
            ptask("a", "Implement the alpha module", AgentRole::Backend, &[]),
            ptask("b", "Test the alpha module", AgentRole::Qa, &["a"]),
        ])
        .await;
    let r =
        h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let done = h.mgr.wait_run(r.run.unwrap().id, Duration::from_secs(60), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Failed);
    let a = &done.tasks[0];
    assert_eq!(a.status, TaskStatus::Failed);
    assert_eq!(a.attempts, 2, "one retry");
    assert_eq!(done.agents_spawned, 2);
    assert!(a.last_error.as_deref().unwrap_or("").contains("model call failed"), "{:?}", a.last_error);
    assert_eq!(done.tasks[1].status, TaskStatus::Blocked);
    let reason = done.status_reason.clone().unwrap();
    assert!(reason.contains("no task can proceed"), "{reason}");
    let report = done.report.unwrap();
    assert_eq!(report.failed, 1);
    assert!(!report.open_issues.is_empty());
    assert_eq!(h.mgr.get_plan(plan.id).await.unwrap().status, PlanStatus::Failed);

    // Consecutive-failure bound.
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-server-error");
    let plan = h.custom_plan(vec![ptask("a", "Implement the alpha module", AgentRole::Backend, &[])]).await;
    h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(false) }).await.unwrap();
    let run = h
        .mgr
        .start_run(ManagerRunParams {
            plan_id: Some(plan.id),
            run_id: None,
            limits: Some(RunLimitsPatch { max_consecutive_failures: Some(1), ..RunLimitsPatch::default() }),
        })
        .await
        .unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(60), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Failed);
    assert!(done.status_reason.unwrap().contains("consecutive"));
    assert_eq!(done.agents_spawned, 1, "no retry once the bound is hit");
}

#[tokio::test]
async fn cancel_stops_agents_and_cancels_unfinished_tasks() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-slow");
    let plan = h
        .custom_plan(vec![
            ptask("a", "Implement the alpha module", AgentRole::Backend, &[]),
            ptask("b", "Test the alpha module", AgentRole::Qa, &["a"]),
        ])
        .await;
    let r =
        h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    let live = h
        .mgr
        .wait_run(run.id, Duration::from_secs(20), |r| r.tasks.iter().any(|t| t.agent_id.is_some()))
        .await
        .unwrap();
    let agent = live.tasks[0].agent_id.unwrap();
    let cancelled = h.mgr.cancel(run.id).await.unwrap();
    assert_eq!(cancelled.status, ManagerRunStatus::Cancelled, "{:?}", cancelled.status_reason);
    assert!(!h.rt.is_live(agent));
    for t in &cancelled.tasks {
        assert_eq!(h.tasks.get(t.task_id).await.unwrap().status, TaskStatus::Cancelled, "{}", t.title);
    }
    assert_eq!(h.mgr.get_plan(plan.id).await.unwrap().status, PlanStatus::Cancelled);
    assert!(h.mgr.cancel(run.id).await.is_err(), "already final");
}

#[tokio::test]
async fn emergency_stop_cancels_the_run() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-slow");
    let plan = h.custom_plan(vec![ptask("a", "Implement the alpha module", AgentRole::Backend, &[])]).await;
    let r =
        h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    h.mgr.wait_run(run.id, Duration::from_secs(20), |r| r.tasks[0].agent_id.is_some()).await.unwrap();
    h.sec.stop_project(h.project, h.sec.user()).await.unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(20), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Cancelled);
    assert_eq!(done.status_reason.as_deref(), Some("emergency stop"));
}

#[tokio::test]
async fn budget_bound_stops_the_run() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    // The mock answers under a priced model id, so usage costs money.
    h.mock("gpt-4o");
    let plan = h
        .custom_plan(vec![
            ptask("a", "Implement the alpha module", AgentRole::Backend, &[]),
            ptask("b", "Implement the beta module", AgentRole::Backend, &["a"]),
        ])
        .await;
    h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(false) }).await.unwrap();
    let run = h
        .mgr
        .start_run(ManagerRunParams {
            plan_id: Some(plan.id),
            run_id: None,
            limits: Some(RunLimitsPatch { max_cost_usd: Some(0.01), ..RunLimitsPatch::default() }),
        })
        .await
        .unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(60), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Failed);
    assert!(done.status_reason.as_deref().unwrap_or("").contains("budget"), "{:?}", done.status_reason);
    assert_eq!(done.agents_spawned, 1, "no new agent after the budget is hit");
    assert!(done.cost_usd <= 0.01 + f64::EPSILON);
}

#[tokio::test]
async fn ask_policy_waits_for_review_and_interrupted_runs_resume() {
    let h = harness(false, ApprovalMode::Ask).await;
    h.mock("mock-smart");
    let plan = h
        .custom_plan(vec![
            ptask("a", "Write the alpha design", AgentRole::Architect, &[]),
            ptask("b", "Write the beta design", AgentRole::Architect, &["a"]),
        ])
        .await;
    let r =
        h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    assert_eq!(run.approval_policy, ApprovalMode::Ask);
    // Agents run with the Ask cap: the first write waits for approval.
    let approval = loop {
        let list = h
            .sec
            .approvals
            .list(&orkestr_security::ApprovalFilter {
                status: Some(ApprovalStatus::Pending),
                project_id: Some(h.project),
                limit: None,
            })
            .await
            .unwrap();
        if let Some(a) = list.into_iter().next() {
            break a;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    h.sec
        .approvals
        .resolve(approval.id, orkestr_security::ApprovalDecision::Approve, None, h.sec.user())
        .await
        .unwrap();
    // The result is not accepted automatically under Ask.
    let waiting = h.mgr.wait_run(run.id, Duration::from_secs(30), |r| r.tasks[0].awaiting_review).await.unwrap();
    assert_eq!(waiting.status, ManagerRunStatus::Running);
    assert!(waiting.status_reason.unwrap().contains("review"));

    // Core shutdown → Interrupted; a new Manager (restart) resumes it.
    h.mgr.shutdown().await;
    let interrupted = h.mgr.get_run(run.id).await.unwrap();
    assert_eq!(interrupted.status, ManagerRunStatus::Interrupted);
    let restarted = Manager::new(manager_deps(&h, Arc::clone(&h.gateway) as Arc<dyn ModelGateway>));
    restarted.init().await.unwrap();
    // The user accepts the reviewed task (Tasks panel), then resumes.
    let t0 = waiting.tasks[0].task_id;
    h.tasks
        .transition(t0, TaskStatus::Completed, orkestr_domain::TaskActor::User, Some("looks good".into()), None)
        .await
        .unwrap();
    h.hub.cfg.lock().approval_policy = ApprovalMode::SemiAuto;
    let resumed = restarted.resume(run.id).await.unwrap();
    assert_eq!(resumed.status, ManagerRunStatus::Running);
    let summary = restarted.resume_summary(h.project).await.unwrap();
    assert!(summary.headline.starts_with("Last session:"), "{}", summary.headline);
    // The run keeps its Ask policy for agents: approve the second write too.
    let approval = loop {
        let list = h
            .sec
            .approvals
            .list(&orkestr_security::ApprovalFilter {
                status: Some(ApprovalStatus::Pending),
                project_id: Some(h.project),
                limit: None,
            })
            .await
            .unwrap();
        if let Some(a) = list.into_iter().next() {
            break a;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    h.sec
        .approvals
        .resolve(approval.id, orkestr_security::ApprovalDecision::Approve, None, h.sec.user())
        .await
        .unwrap();
    let waiting = restarted.wait_run(run.id, Duration::from_secs(30), |r| r.tasks[1].awaiting_review).await.unwrap();
    h.tasks
        .transition(waiting.tasks[1].task_id, TaskStatus::Completed, orkestr_domain::TaskActor::User, None, None)
        .await
        .unwrap();
    let done = restarted.wait_run(run.id, Duration::from_secs(30), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?}", done.status_reason);
}

#[tokio::test]
async fn full_auto_policy_approves_and_runs_the_plan() {
    let h = harness(false, ApprovalMode::FullAuto).await;
    h.mock("mock-smart");
    let plan = h.plan("Create a hello module with tests").await;
    assert_eq!(plan.status, PlanStatus::Running, "auto-approved and started");
    let run_id = plan.run_id.unwrap();
    let done = h
        .mgr
        .wait_run(run_id, Duration::from_secs(120), |r| {
            final_status(r.status) || r.tasks.iter().any(|t| t.awaiting_review)
        })
        .await
        .unwrap();
    // Without git the agents share the checkout; design, implementation and
    // docs complete; QA's outcome depends on node being installed.
    assert!(done.tasks.iter().take(3).all(|t| t.status == TaskStatus::Completed), "{:#?}", done.tasks);
    assert!(h.root.join("src/hello.mjs").is_file(), "shared checkout has the implementation");
    assert!(done.status_reason.is_some() || done.status == ManagerRunStatus::Completed);
    // Starting a second run of a running plan is a conflict.
    assert!(h.mgr.start_run(ManagerRunParams { plan_id: Some(plan.id), run_id: None, limits: None }).await.is_err());
    if node_available() {
        assert_eq!(done.status, ManagerRunStatus::Completed, "{:?} {:#?}", done.status_reason, done.tasks);
    }
}

#[tokio::test]
async fn invalid_requests_are_rejected() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    // No model configured at all.
    let err =
        h.mgr.plan(ManagerPlanParams { project_id: h.project, goal: "x".into(), options: None }).await.unwrap_err();
    assert!(matches!(err, crate::OrchestratorError::NoModel(_)), "{err}");
    h.mock("mock-smart");
    assert!(h
        .mgr
        .chat(ManagerChatParams { project_id: h.project, text: "  ".into(), attachment: None, options: None })
        .await
        .is_err());
    assert!(h
        .mgr
        .chat(ManagerChatParams { project_id: ProjectId::new(), text: "hi".into(), attachment: None, options: None })
        .await
        .is_err());
    let plan = h.custom_plan(vec![ptask("a", "A", AgentRole::Backend, &[])]).await;
    let err = h
        .mgr
        .approve_plan(ApprovePlanParams {
            plan_id: plan.id,
            edits: Some(PlanEdits { remove_tasks: vec!["a".into()], titles: vec![] }),
            start_run: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no tasks"), "{err}");
    assert!(
        h.mgr.start_run(ManagerRunParams { plan_id: Some(plan.id), run_id: None, limits: None }).await.is_err(),
        "not approved"
    );
    let rejected =
        h.mgr.reject_plan(RejectPlanParams { plan_id: plan.id, reason: Some("too vague".into()) }).await.unwrap();
    assert_eq!(rejected.status, PlanStatus::Rejected);
    let hist = h.mgr.history(ManagerHistoryParams { project_id: h.project, limit: None }).await.unwrap();
    assert!(hist.messages.iter().any(|m| m.text.contains("too vague")));
    // The rejected plan is no longer open but its message refers to it, so
    // history still carries it for the plan card.
    let card = hist.plans.iter().find(|p| p.id == plan.id).expect("referenced plan in history");
    assert_eq!(card.status, PlanStatus::Rejected);
    // Bad limits.
    let plan = h.custom_plan(vec![ptask("a", "A", AgentRole::Backend, &[])]).await;
    h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(false) }).await.unwrap();
    let err = h
        .mgr
        .start_run(ManagerRunParams {
            plan_id: Some(plan.id),
            run_id: None,
            limits: Some(RunLimitsPatch { max_parallel_agents: Some(0), ..RunLimitsPatch::default() }),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("maxParallelAgents"), "{err}");
}
