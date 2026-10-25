//! Runtime tests over real engines (SQLite, security, task engine, terminal,
//! snapshots, memory, cost) with scripted providers driven through the real
//! fallback executor.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_domain::{
    AgentId, AgentInstance, AgentState, AgentTemplateId, ApprovalMode, ApprovalStatus, Budget, BudgetPeriod, Model,
    ModelRef, Project, ProjectId, ProviderHealth, ProviderId, ProviderKind, TaskStatus, ToolCallStatus, WorktreePolicy,
};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::methods::agents::{AgentStopMode, NewTaskSpec, SpawnAgentParams};
use orkestr_protocol::methods::templates::TemplateDraft;
use orkestr_providers::adapters::mock::{MockAdapter, MockOptions};
use orkestr_providers::{
    chat_with_fallback, AdapterSource, AttemptOutcome, AuthStatus, ChatEvent, ChatRequest, ChatStream, FallbackOptions,
    HandoffHook, HealthReport, ProviderAdapter, ProviderDescriptor, ProviderError, ProviderEvent, RouteRequest,
    StopReason, Usage,
};
use orkestr_security::{ApprovalDecision, CheckContext, Decision, PermissionEngine, Resource, Security};
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use orkestr_terminal::{CommandGate, GateDecision, GateRequest};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::gateway::{ModelGateway, ProviderInfo};
use crate::run::submit_json;
use crate::templates::draft_of;
use crate::{AgentRuntime, RuntimeConfig, RuntimeDeps};

// ---------------------------------------------------------------------------
// scripted provider
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Step {
    Events(Vec<ChatEvent>),
    Hang,
    /// Runs a side effect (e.g. another writer changing the workspace) when
    /// the model is asked for this turn, then plays the inner step.
    Act(Arc<dyn Fn() + Send + Sync>, Box<Step>),
}

fn tool(name: &str, args: Value) -> Step {
    Step::Events(vec![
        ChatEvent::TextDelta(format!("Calling {name}.")),
        ChatEvent::ToolCall { id: format!("c_{name}_{}", uuid::Uuid::now_v7()), name: name.into(), arguments: args },
        ChatEvent::Usage(Usage { input_tokens: 100, output_tokens: 20, ..Usage::default() }),
        ChatEvent::Done { stop_reason: StopReason::ToolUse },
    ])
}

fn submit(status: &str, summary: &str) -> Step {
    tool("submit_result", submit_json(status, summary))
}

struct Scripted {
    id: ProviderId,
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<ChatRequest>>,
    /// Billing override (default: the mock's `Local`).
    cost_model: Mutex<Option<orkestr_providers::CostModelKind>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::new(),
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
            cost_model: Mutex::new(None),
        })
    }
}

#[async_trait]
impl ProviderAdapter for Scripted {
    fn id(&self) -> ProviderId {
        self.id
    }
    fn descriptor(&self) -> ProviderDescriptor {
        let mut d = MockAdapter::new(self.id, "Mock (offline demo)", MockOptions::default()).descriptor();
        if let Some(c) = self.cost_model.lock().clone() {
            d.cost_model = c;
        }
        d
    }
    async fn auth_state(&self) -> AuthStatus {
        AuthStatus::Ready { label: "scripted".into() }
    }
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(Vec::new())
    }
    async fn health(&self) -> HealthReport {
        HealthReport { health: ProviderHealth::Healthy, latency_ms: None, detail: None, version: None }
    }
    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        self.requests.lock().push(request);
        let mut step = self.steps.lock().pop_front().unwrap_or_else(|| {
            Step::Events(vec![
                ChatEvent::TextDelta("script exhausted".into()),
                ChatEvent::Done { stop_reason: StopReason::EndTurn },
            ])
        });
        while let Step::Act(act, inner) = step {
            act();
            step = *inner;
        }
        Ok(match step {
            Step::Events(evs) => futures::stream::iter(evs).boxed(),
            Step::Hang => futures::stream::once(async move {
                cancel.cancelled().await;
                ChatEvent::Error(ProviderError::Cancelled)
            })
            .boxed(),
            Step::Act(..) => unreachable!("unwrapped above"),
        })
    }
}

#[derive(Default)]
struct Source {
    adapters: Mutex<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    events: Mutex<Vec<ProviderEvent>>,
}

impl AdapterSource for Source {
    fn adapter(&self, provider: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.adapters.lock().get(provider).cloned()
    }
    fn observe(&self, _model: &ModelRef, _outcome: &AttemptOutcome) {}
    fn emit(&self, event: ProviderEvent) {
        self.events.lock().push(event);
    }
}

struct Gateway {
    source: Arc<Source>,
    chain: Mutex<Vec<ModelRef>>,
    /// Provider kinds other than `Mock` (CLI coding agents).
    kinds: Mutex<HashMap<ProviderId, ProviderKind>>,
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
            kind: self.kinds.lock().get(&id).copied().unwrap_or(ProviderKind::Mock),
            is_local: true,
            enabled: true,
        })
    }
    fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.source.adapters.lock().get(&id).cloned()
    }
}

// ---------------------------------------------------------------------------
// terminal gate (same policy as the core's SecurityCommandGate)
// ---------------------------------------------------------------------------

struct TestGate {
    sec: Arc<Security>,
    pending: Mutex<HashMap<String, orkestr_domain::ApprovalId>>,
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
        match self.sec.engine.check(&actor, orkestr_domain::Capability::Terminal, &res, &ctx) {
            Decision::Allow => GateDecision::Allow,
            Decision::Deny(d) => GateDecision::Deny { reason: d.message },
            Decision::Ask(a) => {
                let prev = self.pending.lock().get(&r.command).copied();
                if let Some(id) = prev {
                    let st = self.sec.approvals.get(id).await.ok().flatten().map(|a| a.status);
                    match st {
                        Some(ApprovalStatus::Approved) => {
                            self.pending.lock().remove(&r.command);
                            return GateDecision::Allow;
                        }
                        Some(ApprovalStatus::Rejected) => return GateDecision::Deny { reason: "rejected".into() },
                        Some(ApprovalStatus::Pending) => {
                            return GateDecision::Ask { reason: a.message, approval_id: Some(id.to_string()) }
                        }
                        _ => {}
                    }
                }
                let mut req = orkestr_security::ApprovalRequest::new(
                    orkestr_domain::Capability::Terminal,
                    "Run command",
                    a.message.clone(),
                    a.risk,
                );
                req.project_id = r.project_id;
                req.agent_id = agent;
                req.command_preview = Some(r.command.clone());
                let ap = self.sec.approvals.request(req).await.unwrap();
                self.pending.lock().insert(r.command.clone(), ap.id);
                GateDecision::Ask { reason: a.message, approval_id: Some(ap.id.to_string()) }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct H {
    ui_language: crate::UiLanguage,
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    rt: AgentRuntime,
    sec: Arc<Security>,
    events: EventBus,
    tasks: Arc<orkestr_task_engine::TaskService>,
    db: Db,
    project: ProjectId,
    source: Arc<Source>,
    gateway: Arc<Gateway>,
    snapshots: Arc<orkestr_snapshots::SnapshotService>,
}

fn model(p: &Scripted, m: &str) -> ModelRef {
    ModelRef { provider_id: p.id, model: m.into() }
}

async fn harness(mode: ApprovalMode) -> H {
    harness_with(mode, orkestr_snapshots::ScanOptions::default()).await
}

async fn harness_with(mode: ApprovalMode, scan: orkestr_snapshots::ScanOptions) -> H {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("README.md"), "# Demo project\n").unwrap();
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
    migs.extend(crate::agent_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(8192);
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
    let mut p = Project::new("Demo", orkestr_domain::now());
    p.root_path = Some(root.to_string_lossy().into_owned());
    p.approval_mode = mode;
    meta.insert(&p).await.unwrap();
    let tasks = Arc::new(orkestr_task_engine::TaskService::new(db.clone(), events.clone()));
    let objects = Arc::new(
        orkestr_storage::FsObjectStore::open(dir.path().join("objects"), orkestr_storage::Compression::None).unwrap(),
    );
    let snapshots = Arc::new(orkestr_snapshots::SnapshotService::new(
        db.clone(),
        objects as Arc<dyn orkestr_storage::ObjectStore>,
        None,
        scan,
    ));
    let terminals = orkestr_terminal::TerminalService::builder()
        .gate(Arc::new(TestGate { sec: Arc::clone(&sec), pending: Mutex::new(HashMap::new()) }))
        .build();
    let source = Arc::new(Source::default());
    let gateway =
        Arc::new(Gateway { source: Arc::clone(&source), chain: Mutex::new(Vec::new()), kinds: Mutex::default() });
    let ui_language = crate::UiLanguage::default();
    let deps = RuntimeDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&sec),
        tasks: Arc::clone(&tasks),
        projects: meta as Arc<dyn ProjectRepository>,
        vcs: orkestr_vcs::Vcs::new(Arc::new(orkestr_vcs::GitCli::discover()), db.clone(), dir.path().join("worktrees")),
        snapshots: Arc::clone(&snapshots),
        terminals,
        memory: orkestr_memory::MemoryEngine::new(db.clone()).with_events(events.clone()),
        cost: orkestr_cost::CostEngine::new(db.clone()).with_events(events.clone()),
        models: Arc::clone(&gateway) as Arc<dyn ModelGateway>,
        config: RuntimeConfig {
            approval_timeout: Duration::from_secs(30),
            ui_language: ui_language.clone(),
            ..RuntimeConfig::default()
        },
    };
    let rt = AgentRuntime::new(deps);
    rt.init().await.unwrap();
    H { ui_language, root, outside, rt, sec, events, tasks, db, project: p.id, source, gateway, snapshots, _dir: dir }
}

impl H {
    fn provider(&self, steps: Vec<Step>) -> Arc<Scripted> {
        let s = Scripted::new(steps);
        self.source.adapters.lock().insert(s.id, Arc::clone(&s) as Arc<dyn ProviderAdapter>);
        *self.gateway.chain.lock() = vec![model(&s, "scripted-1")];
        s
    }

    async fn template(&self, tools: &[&str], policy: WorktreePolicy, budget: Option<Budget>) -> AgentTemplateId {
        let backend = self.rt.templates().list(Some(orkestr_domain::AgentRole::Backend), None).await.unwrap().remove(0);
        let mut d: TemplateDraft = draft_of(&backend);
        d.name = "Test agent".into();
        d.tools = tools.iter().map(|t| (*t).to_owned()).collect();
        d.worktree_policy = policy;
        d.quality_gates = Vec::new();
        d.budget = budget;
        self.rt.templates().create(d).await.unwrap().id
    }

    async fn spawn(&self, template: AgentTemplateId, max_turns: Option<u32>) -> AgentInstance {
        let (a, task) = self
            .rt
            .spawn(SpawnAgentParams {
                project_id: self.project,
                template_id: template,
                task_id: None,
                new_task: Some(NewTaskSpec {
                    title: "Add a greeting file".into(),
                    description: Some("Create src/hello.txt".into()),
                    acceptance_criteria: Some(vec!["file exists".into()]),
                    priority: None,
                }),
                model: None,
                worktree_policy: None,
                name: None,
                max_turns,
                base_branch: None,
                approval_mode: None,
                budget: None,
            })
            .await
            .unwrap();
        assert_eq!(task.agent_id, Some(a.id));
        a
    }

    async fn wait_done(&self, id: AgentId) -> AgentInstance {
        for _ in 0..1500 {
            if !self.rt.is_live(id) {
                return self.rt.get(id).await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("agent did not finish");
    }

    async fn wait_state(&self, id: AgentId, state: AgentState) {
        for _ in 0..1500 {
            if self.rt.get(id).await.unwrap().state == state {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("agent never reached {state:?}");
    }

    async fn pending_approval(&self, agent: AgentId) -> orkestr_domain::Approval {
        for _ in 0..1500 {
            let list = self
                .sec
                .approvals
                .list(&orkestr_security::ApprovalFilter { status: Some(ApprovalStatus::Pending), ..Default::default() })
                .await
                .unwrap();
            if let Some(a) = list.into_iter().find(|a| a.agent_id == Some(agent)) {
                return a;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no approval requested");
    }

    async fn task_status(&self, a: &AgentInstance) -> TaskStatus {
        self.tasks.get(a.task_id.unwrap()).await.unwrap().status
    }
}

fn drain_kinds(sub: &mut orkestr_events::Subscription) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(Some(Received::Event(e))) = sub.recv().now_or_never() {
        out.push(e.kind_name().to_owned());
    }
    out
}

use futures::FutureExt;

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writes_a_file_in_its_worktree_and_completes() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let mut sub = h.events.subscribe_all();
    let tpl = h.template(&["write_file", "read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let p = h.provider(vec![
        tool("write_file", json!({"path": "src/hello.txt", "content": "hello api_key=sk-abcdefghijklmnopqrstuv"})),
        submit("done", "Created src/hello.txt"),
    ]);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert!(h.root.join("src/hello.txt").is_file());
    assert_eq!(h.task_status(&a).await, TaskStatus::Completed);
    assert_eq!(a.turns_used, 2);
    assert_eq!(a.usage.input_tokens, 200);
    // The second model call saw the tool result.
    let reqs = p.requests.lock().clone();
    assert!(reqs[1]
        .messages
        .iter()
        .any(|m| m.role == orkestr_providers::Role::Tool && m.content.contains("created src/hello.txt")));

    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let write = tr.tool_calls.iter().find(|c| c.tool_call.tool == "write_file").unwrap();
    assert_eq!(write.tool_call.status, ToolCallStatus::Succeeded);
    assert!(!write.tool_call.input.to_string().contains("sk-abcdefghijklmnop"), "args are redacted");
    assert!(tr.messages.iter().any(|m| m.text.contains("Calling write_file")));
    assert!(tr.messages.iter().any(|m| m.text.contains("Checkpoint")), "automatic checkpoint before first write");
    // Handoff on completion, usage in the cost engine, task result recorded.
    let hs = h.rt.handoffs(a.id).await.unwrap();
    assert_eq!(hs[0].reason, orkestr_protocol::methods::agents::HandoffReason::Completed);
    assert!(hs[0].workspace.files_changed.contains(&"src/hello.txt".to_owned()));
    let task = h.tasks.get(a.task_id.unwrap()).await.unwrap();
    assert_eq!(task.result.as_ref().unwrap().summary, "Created src/hello.txt");
    assert!(task.files_touched.contains(&"src/hello.txt".to_owned()));
    let n: i64 = h.db.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM usage_records", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(n, 2);
    // Template grants were installed as Agent rules and removed at the end.
    let rules = h
        .sec
        .permissions
        .list(&orkestr_security::permissions::RuleFilter { project_id: None, agent_id: Some(a.id), task_id: None })
        .await
        .unwrap();
    assert!(rules.iter().all(|r| !matches!(r.rule.scope, orkestr_domain::PermissionScope::Agent { .. })));
    let kinds = drain_kinds(&mut sub);
    for k in [
        "AgentSpawned",
        "AgentStarted",
        "AgentStateChanged",
        "ToolCallStarted",
        "ToolCallFinished",
        "AgentStopped",
        "SnapshotCreated",
        "FileChanged",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing {k} in {kinds:?}");
    }
    // Streaming deltas are emitted (ephemeral events).
    assert!(kinds.iter().any(|x| x == "AgentMessage"));
}

#[tokio::test]
async fn completed_task_has_a_checkpoint_pair_and_rolls_back_alone() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["write_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![
        tool("write_file", json!({"path": "src/hello.txt", "content": "hello"})),
        submit("done", "Created src/hello.txt"),
    ]);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let task_id = a.task_id.unwrap();
    let snaps = h.snapshots.list(h.project, Some(task_id), None, 10).await.unwrap();
    let phases: Vec<_> = snaps.iter().filter_map(|s| s.task_phase).collect();
    let (before, after) = (orkestr_domain::TaskSnapshotPhase::Before, orkestr_domain::TaskSnapshotPhase::After);
    assert!(phases.contains(&before) && phases.contains(&after), "{phases:?}");
    // The user edits an unrelated file afterwards; rolling the task back
    // removes only what the task did (§63 J).
    std::fs::write(h.root.join("README.md"), "# Demo project\nuser edit\n").unwrap();
    let rb = h.snapshots.rollback_task(h.project, task_id, None, None, false).await.unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(!h.root.join("src/hello.txt").exists());
    assert!(std::fs::read_to_string(h.root.join("README.md")).unwrap().contains("user edit"));
}

/// Another agent and the user change files in the shared checkout while the
/// task runs (between its before- and after-checkpoints): rolling the task
/// back reverts only the task's own files.
#[tokio::test]
async fn task_rollback_in_shared_root_spares_concurrent_writers() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["write_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let root = h.root.clone();
    let others: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/b.ts"), "agent B's work\n").unwrap();
        std::fs::write(root.join("README.md"), "# Demo project\nuser edit during the task\n").unwrap();
    });
    h.provider(vec![
        tool("write_file", json!({"path": "src/hello.txt", "content": "hello"})),
        Step::Act(others, Box::new(submit("done", "Created src/hello.txt"))),
    ]);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let rb = h.snapshots.rollback_task(h.project, a.task_id.unwrap(), None, None, false).await.unwrap();
    assert_eq!(rb.deleted, vec!["src/hello.txt"]);
    let conflicts: Vec<_> = rb.conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(conflicts, vec!["README.md", "src/b.ts"], "{rb:?}");
    assert!(!h.root.join("src/hello.txt").exists());
    assert_eq!(std::fs::read_to_string(h.root.join("src/b.ts")).unwrap(), "agent B's work\n");
    assert!(std::fs::read_to_string(h.root.join("README.md")).unwrap().contains("user edit during the task"));
}

/// Same, for a task whose only change came from a command (no reported
/// files): the command's own change is attributed to the task, other
/// writers' work in the shared checkout is never reverted.
#[tokio::test]
async fn command_only_task_rollback_in_shared_root_spares_concurrent_writers() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let root = h.root.clone();
    let others: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/api.rs"), "agent B's work\n").unwrap();
    });
    h.provider(vec![
        tool("run_command", json!({"command": "echo changed > notes.txt"})),
        Step::Act(others, Box::new(submit("done", "formatted"))),
    ]);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let rb = h.snapshots.rollback_task(h.project, a.task_id.unwrap(), None, None, false).await.unwrap();
    assert_eq!(rb.deleted, vec!["notes.txt"], "{rb:?}");
    let conflicts: Vec<_> = rb.conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(conflicts, vec!["src/api.rs"], "{rb:?}");
    assert_eq!(std::fs::read_to_string(h.root.join("src/api.rs")).unwrap(), "agent B's work\n");
}

/// Indexed Knowledge Base docs that match the task reach the agent in its
/// initial context.
#[tokio::test]
async fn relevant_knowledge_base_chunks_reach_the_agent() {
    let h = harness(ApprovalMode::FullAuto).await;
    let docs = tempfile::tempdir().unwrap();
    std::fs::write(
        docs.path().join("conventions.md"),
        "# Greeting conventions\nEvery greeting file ends with an exclamation mark and names the team.\n",
    )
    .unwrap();
    h.rt.deps()
        .memory
        .add_knowledge(orkestr_protocol::methods::memory::AddKnowledgeParams {
            project_id: Some(h.project),
            location: docs.path().to_string_lossy().into_owned(),
            name: Some("Team docs".into()),
            kind: None,
        })
        .await
        .unwrap();
    let tpl = h.template(&["submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let p = h.provider(vec![submit("done", "ok")]);
    h.wait_done(h.spawn(tpl, None).await.id).await;
    let first = p.requests.lock()[0].messages[0].content.clone();
    assert!(first.contains("## Project knowledge"), "{first}");
    assert!(first.contains("**Team docs**") && first.contains("exclamation mark"), "{first}");
}

/// No checkpoint, no change: when the task's `Before` checkpoint cannot be
/// taken, a changing tool is refused (the task stays undoable) and the
/// checkpoint is tried again before the next change.
#[tokio::test]
async fn a_change_without_a_checkpoint_is_refused() {
    // Any workspace is "too large" for this snapshot engine.
    let scan = orkestr_snapshots::ScanOptions { max_files: 0, ..orkestr_snapshots::ScanOptions::default() };
    let h = harness_with(ApprovalMode::FullAuto, scan).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![
        tool("write_file", json!({"path": "src/hello.txt", "content": "hello"})),
        tool("run_command", json!({"command": "echo changed > notes.txt"})),
        submit("blocked", "no checkpoint"),
    ]);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert!(!h.root.join("src/hello.txt").exists(), "the write was not made");
    assert!(!h.root.join("notes.txt").exists(), "the command was not run");
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let failed: Vec<_> = tr
        .tool_calls
        .iter()
        .filter(|c| c.result.as_deref().unwrap_or_default().contains("could not take the task's checkpoint"))
        .collect();
    assert_eq!(failed.len(), 2, "both changes refused, the checkpoint retried: {:?}", tr.tool_calls);
    let snaps = h.snapshots.list(h.project, a.task_id, None, 10).await.unwrap();
    assert!(snaps.is_empty(), "no lone After checkpoint: {snaps:?}");
}

#[tokio::test]
async fn path_escapes_and_git_dir_are_denied() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let abs = h.outside.join("pwned.txt").to_string_lossy().into_owned();
    h.provider(vec![
        tool("read_file", json!({"path": "../outside/secret.txt"})),
        tool("write_file", json!({"path": abs, "content": "x"})),
        tool("write_file", json!({"path": ".git/config", "content": "x"})),
        submit("done", "tried"),
    ]);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed);
    assert!(!h.outside.join("pwned.txt").exists());
    assert!(!h.root.join(".git/config").exists());
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let denied: Vec<_> = tr.tool_calls.iter().filter(|c| c.tool_call.status == ToolCallStatus::Denied).collect();
    assert_eq!(denied.len(), 3, "{:?}", tr.tool_calls);
    assert!(denied[0].result.as_deref().unwrap().contains("escapes"));
}

/// Audit r2: an in-root link (junction / symlink) to `.git` — like an 8.3
/// alias such as `GIT~1` — must not let `write_file` into git internals: the
/// engine asks (canonical key `.git/...`), and even after approval the tool
/// refuses because it checks the resolved path.
#[tokio::test]
async fn write_through_link_to_git_dir_is_refused() {
    let h = harness(ApprovalMode::FullAuto).await;
    std::fs::create_dir_all(h.root.join(".git/hooks")).unwrap();
    let linked = {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(h.root.join("gl"))
                .arg(h.root.join(".git"))
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(h.root.join(".git"), h.root.join("gl")).is_ok()
        }
    };
    if !linked {
        return;
    }
    let tpl = h.template(&["write_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![
        tool("write_file", json!({"path": "gl/hooks/pre-commit", "content": "#!/bin/sh\necho pwned\n"})),
        submit("done", "tried"),
    ]);
    let a = h.spawn(tpl, None).await;
    let ap = h.pending_approval(a.id).await;
    assert_eq!(ap.danger, Some(orkestr_domain::DangerCategory::GitInternals), "{ap:?}");
    h.sec.approvals.resolve(ap.id, ApprovalDecision::Approve, None, h.sec.user()).await.unwrap();
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert!(!h.root.join(".git/hooks/pre-commit").exists());
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let call = tr.tool_calls.iter().find(|c| c.tool_call.tool.contains("write")).unwrap();
    assert!(call.result.as_deref().unwrap_or_default().contains("inside `.git`"), "{call:?}");
}

#[tokio::test]
async fn dangerous_command_waits_for_approval_then_runs() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    // A local dangerous command: since audit r1 a `git push` also needs the
    // Network capability, which the Backend template denies.
    h.provider(vec![
        tool("run_command", json!({"command": "git branch -D no-such-branch"})),
        submit("done", "deleted"),
    ]);
    let a = h.spawn(tpl, None).await;
    let ap = h.pending_approval(a.id).await;
    h.wait_state(a.id, AgentState::Waiting).await;
    assert_eq!(h.task_status(&a).await, TaskStatus::Waiting);
    assert!(ap.command_preview.as_deref().unwrap().contains("branch -D"));
    h.sec.approvals.resolve(ap.id, ApprovalDecision::Approve, None, h.sec.user()).await.unwrap();
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(h.task_status(&a).await, TaskStatus::Completed);
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let cmd = tr.tool_calls.iter().find(|c| c.tool_call.tool == "run_command").unwrap();
    assert_ne!(cmd.tool_call.status, ToolCallStatus::Denied);
    assert_eq!(cmd.tool_call.approval_id, Some(ap.id));
    assert!(cmd.result.as_deref().unwrap().contains("exit code"), "the command actually ran: {:?}", cmd.result);
}

#[tokio::test]
async fn rejected_command_is_denied_and_the_task_fails() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let marker = h.root.join("marker.txt");
    let p = h.provider(vec![
        tool("run_command", json!({"command": "git reset --hard HEAD~3"})),
        submit("failed", "the user rejected the reset"),
    ]);
    let a = h.spawn(tpl, None).await;
    let ap = h.pending_approval(a.id).await;
    h.sec.approvals.resolve(ap.id, ApprovalDecision::Reject, Some("no".into()), h.sec.user()).await.unwrap();
    let a = h.wait_done(a.id).await;
    assert!(!marker.exists());
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let cmd = tr.tool_calls.iter().find(|c| c.tool_call.tool == "run_command").unwrap();
    assert_eq!(cmd.tool_call.status, ToolCallStatus::Denied);
    assert!(cmd.result.as_deref().unwrap().contains("rejected"));
    assert!(p.requests.lock()[1].messages.iter().any(|m| m.is_error && m.content.contains("rejected")));
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
}

#[tokio::test]
async fn emergency_stop_cancels_the_agent() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let mut sub = h.events.subscribe_all();
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![Step::Hang]);
    let a = h.spawn(tpl, None).await;
    h.wait_state(a.id, AgentState::Thinking).await;
    let out = h.sec.stop_all(h.sec.user()).await.unwrap();
    assert_eq!(out.report.agents_stopped, 1);
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Stopped);
    assert_eq!(h.task_status(&a).await, TaskStatus::Cancelled);
    let mut saw = false;
    while let Some(Some(Received::Event(e))) = sub.recv().now_or_never() {
        if let orkestr_protocol::event::EventKind::AgentStopped(p) = &e.kind {
            saw = p.reason == orkestr_protocol::event::StopReason::EmergencyStop;
        }
    }
    assert!(saw, "AgentStopped(EmergencyStop) published");
    // New spawns are refused until re-armed.
    let err =
        h.rt.spawn(SpawnAgentParams {
            project_id: h.project,
            template_id: tpl,
            task_id: None,
            new_task: Some(NewTaskSpec { title: "x".into(), ..NewTaskSpec::default() }),
            model: None,
            worktree_policy: None,
            name: None,
            max_turns: None,
            base_branch: None,
            approval_mode: None,
            budget: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, crate::AgentError::Forbidden(_)));
}

#[tokio::test]
async fn user_stop_interrupts_the_task() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![Step::Hang]);
    let a = h.spawn(tpl, None).await;
    h.wait_state(a.id, AgentState::Thinking).await;
    let a = h.rt.stop(a.id, AgentStopMode::Interrupt).await.unwrap();
    assert_eq!(a.state, AgentState::Stopped);
    assert_eq!(h.task_status(&a).await, TaskStatus::Interrupted);
    assert!(!h.rt.is_live(a.id));
    // Stopping again is idempotent.
    assert_eq!(h.rt.stop(a.id, AgentStopMode::Interrupt).await.unwrap().state, AgentState::Stopped);
}

#[tokio::test]
async fn max_turns_ends_the_run() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider((0..5).map(|_| tool("read_file", json!({"path": "README.md"}))).collect());
    let a = h.spawn(tpl, Some(2)).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Error);
    assert_eq!(a.turns_used, 2);
    assert!(a.last_error.as_deref().unwrap().contains("max_turns"));
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
}

#[tokio::test]
async fn budget_exceeded_stops_cleanly() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let budget = Budget {
        max_cost_usd: Some(0.4),
        max_tokens: None,
        max_duration_secs: None,
        period: BudgetPeriod::PerTask,
        warn_at: 0.8,
    };
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, Some(budget)).await;
    let p = h.provider(vec![
        Step::Events(vec![
            ChatEvent::ToolCall { id: "c1".into(), name: "read_file".into(), arguments: json!({"path": "README.md"}) },
            ChatEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 10,
                reported_cost_usd: Some(0.5),
                ..Usage::default()
            }),
            ChatEvent::Done { stop_reason: StopReason::ToolUse },
        ]),
        submit("done", "never reached"),
    ]);
    *p.cost_model.lock() = Some(orkestr_providers::CostModelKind::PerToken);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Error);
    assert!(a.last_error.as_deref().unwrap().contains("budget_exceeded"), "{:?}", a.last_error);
    assert!((a.usage.cost_usd - 0.5).abs() < 1e-9);
    assert_eq!(a.turns_used, 1, "no further model call after the budget was exhausted");
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
}

#[tokio::test]
async fn a_subscription_cli_cost_is_api_equivalent_not_spend() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let budget = Budget {
        max_cost_usd: Some(0.4),
        max_tokens: None,
        max_duration_secs: None,
        period: BudgetPeriod::PerTask,
        warn_at: 0.8,
    };
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, Some(budget)).await;
    // A Pro/Max login reports `total_cost_usd` on every call (here above the cap).
    let reported = |c: f64| {
        ChatEvent::Usage(Usage { input_tokens: 10, output_tokens: 10, reported_cost_usd: Some(c), ..Usage::default() })
    };
    let p = h.provider(vec![
        Step::Events(vec![
            ChatEvent::ToolCall { id: "c1".into(), name: "read_file".into(), arguments: json!({"path": "README.md"}) },
            reported(0.5),
            ChatEvent::Done { stop_reason: StopReason::ToolUse },
        ]),
        submit("done", "read it"),
    ]);
    *p.cost_model.lock() = Some(orkestr_providers::CostModelKind::Subscription);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(a.usage.cost_usd, 0.0, "no money was spent");
    assert!(a.usage.input_tokens >= 10, "usage is still tracked: {:?}", a.usage);
    let task = h.tasks.get(a.task_id.unwrap()).await.unwrap();
    assert_eq!(task.usage.cost_usd, 0.0);
    let records =
        h.rt.deps()
            .cost
            .records(orkestr_protocol::methods::cost::CostRecordsParams { agent_id: Some(a.id), ..Default::default() })
            .await
            .unwrap();
    assert!(records.iter().all(|r| r.cost_usd == 0.0), "{records:?}");
    assert!(records.iter().any(|r| r.api_equivalent_usd == Some(0.5)), "{records:?}");
}

fn price_warnings(sub: &mut orkestr_events::Subscription) -> usize {
    let mut n = 0;
    while let Some(Some(Received::Event(e))) = sub.recv().now_or_never() {
        if let orkestr_protocol::event::EventKind::Log(l) = &e.kind {
            if l.fields.get("code").and_then(Value::as_str) == Some("model_price_unknown") {
                n += 1;
            }
        }
    }
    n
}

#[tokio::test]
async fn unknown_price_of_a_billed_model_is_warned_once() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let mut sub = h.events.subscribe_all();
    let budget = Budget {
        max_cost_usd: Some(1.0),
        max_tokens: None,
        max_duration_secs: None,
        period: BudgetPeriod::PerTask,
        warn_at: 0.8,
    };
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, Some(budget)).await;
    let p = h.provider(vec![tool("read_file", json!({"path": "README.md"})), submit("done", "read it")]);
    *p.cost_model.lock() = Some(orkestr_providers::CostModelKind::PerToken);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(price_warnings(&mut sub), 1, "one warning per run, not per turn");

    // Local / subscription billing has no marginal price: nothing to warn about.
    let mut sub = h.events.subscribe_all();
    h.provider(vec![submit("done", "ok")]);
    let a = h.spawn(tpl, None).await;
    h.wait_done(a.id).await;
    assert_eq!(price_warnings(&mut sub), 0);
}

#[tokio::test]
async fn fallback_to_a_provider_over_its_budget_is_refused() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let first =
        h.provider(vec![Step::Events(vec![ChatEvent::Error(ProviderError::QuotaExceeded("out of credits".into()))])]);
    let second = Scripted::new(vec![submit("done", "finished on the fallback model")]);
    *second.cost_model.lock() = Some(orkestr_providers::CostModelKind::PerToken);
    h.source.adapters.lock().insert(second.id, Arc::clone(&second) as Arc<dyn ProviderAdapter>);
    // A priced model, so the pre-flight estimate is above the tiny limit.
    *h.gateway.chain.lock() = vec![model(&first, "primary"), model(&second, "gpt-4o")];
    h.rt.deps()
        .cost
        .set_budget(orkestr_protocol::methods::cost::SetBudgetParams {
            id: None,
            name: Some("backup provider".into()),
            scope: orkestr_protocol::methods::cost::BudgetScope::Provider { provider_id: second.id },
            period: BudgetPeriod::Monthly,
            limit_usd: Some(0.000_001),
            unrestricted: None,
            warn_at: None,
            enabled: None,
        })
        .await
        .unwrap();
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Error, "{:?}", a.last_error);
    assert!(second.requests.lock().is_empty(), "the over-budget fallback provider was never called");
    assert!(!h.source.events.lock().iter().any(|e| matches!(e, ProviderEvent::Fallback { .. })));
}

#[tokio::test]
async fn concurrent_spawns_on_one_task_start_only_one_agent() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![Step::Hang, Step::Hang]);
    let task = h
        .tasks
        .create(
            orkestr_protocol::methods::tasks::CreateTaskParams {
                project_id: h.project,
                title: "One task".into(),
                ..Default::default()
            },
            orkestr_domain::TaskActor::User,
        )
        .await
        .unwrap();
    let params = || SpawnAgentParams {
        project_id: h.project,
        template_id: tpl,
        task_id: Some(task.id),
        new_task: None,
        model: None,
        worktree_policy: None,
        name: None,
        max_turns: None,
        base_branch: None,
        approval_mode: None,
        budget: None,
    };
    let (a, b) = tokio::join!(h.rt.spawn(params()), h.rt.spawn(params()));
    let (ok, err) = match (a, b) {
        (Ok(x), Err(e)) | (Err(e), Ok(x)) => (x.0, e),
        (a, b) => panic!("exactly one spawn must win: {:?} / {:?}", a.map(|x| x.0.id), b.map(|x| x.0.id)),
    };
    assert!(matches!(err, crate::AgentError::Conflict(_)), "{err}");
    h.wait_state(ok.id, AgentState::Thinking).await;
    let t = h.tasks.get(task.id).await.unwrap();
    assert_eq!(t.status, TaskStatus::Running, "the winner's task is not flipped to Failed");
    assert_eq!(t.agent_id, Some(ok.id));
    h.rt.stop(ok.id, AgentStopMode::Cancel).await.unwrap();
}

#[tokio::test]
async fn provider_fallback_creates_a_handoff() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let first =
        h.provider(vec![Step::Events(vec![ChatEvent::Error(ProviderError::QuotaExceeded("out of credits".into()))])]);
    let second = Scripted::new(vec![submit("done", "finished on the fallback model")]);
    h.source.adapters.lock().insert(second.id, Arc::clone(&second) as Arc<dyn ProviderAdapter>);
    *h.gateway.chain.lock() = vec![model(&first, "primary"), model(&second, "backup")];
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(a.model.as_ref().unwrap().model, "backup");
    let hs = h.rt.handoffs(a.id).await.unwrap();
    assert!(hs.iter().any(|x| x.reason == orkestr_protocol::methods::agents::HandoffReason::ProviderFallback));
    let sys = second.requests.lock()[0].system.clone().unwrap();
    assert!(sys.contains("## Handoff"), "the fallback model receives the structured handoff");
    assert!(h.source.events.lock().iter().any(|e| matches!(e, ProviderEvent::Fallback { .. })));
    assert!(h.rt.transcript(a.id, None).await.unwrap().messages.iter().any(|m| m.text.contains("Provider fallback")));
}

#[tokio::test]
async fn provider_fallback_sticks_to_the_fallback_model_for_later_turns() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["list_dir", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let quota = || Step::Events(vec![ChatEvent::Error(ProviderError::QuotaExceeded("out of credits".into()))]);
    let first = h.provider(vec![quota(), quota(), quota()]);
    let second =
        Scripted::new(vec![tool("list_dir", json!({"path": "."})), submit("done", "finished on the fallback")]);
    h.source.adapters.lock().insert(second.id, Arc::clone(&second) as Arc<dyn ProviderAdapter>);
    *h.gateway.chain.lock() = vec![model(&first, "primary"), model(&second, "backup")];
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(first.requests.lock().len(), 1, "the exhausted primary is not called again on later turns");
    assert_eq!(second.requests.lock().len(), 2);
    let hs = h.rt.handoffs(a.id).await.unwrap();
    assert_eq!(
        hs.iter().filter(|x| x.reason == orkestr_protocol::methods::agents::HandoffReason::ProviderFallback).count(),
        1
    );
    let switches = h.source.events.lock().iter().filter(|e| matches!(e, ProviderEvent::Fallback { .. })).count();
    assert_eq!(switches, 1, "one ProviderFallback for the switch");
}

#[tokio::test]
async fn ask_user_waits_for_the_answer() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["ask_user", "submit_result"], WorktreePolicy::None, None).await;
    let p = h.provider(vec![tool("ask_user", json!({"question": "Use PostgreSQL?"})), submit("done", "ok")]);
    let a = h.spawn(tpl, None).await;
    let ap = h.pending_approval(a.id).await;
    h.sec.approvals.resolve(ap.id, ApprovalDecision::Approve, Some("yes, v16".into()), h.sec.user()).await.unwrap();
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed);
    assert!(a.worktree.is_none());
    let reqs = p.requests.lock().clone();
    assert!(reqs[1].messages.iter().any(|m| m.content.contains("YES") && m.content.contains("v16")));
    assert!(!reqs[0].tools.iter().any(|t| t.name == "read_file"), "no fs tools without a workspace");
}

#[tokio::test]
async fn unknown_or_disallowed_tools_and_invalid_args_are_reported() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::ReadOnlyMain, None).await;
    h.provider(vec![
        tool("write_file", json!({"path": "a.txt", "content": "x"})),
        tool("read_file", json!({"pathh": "README.md"})),
        Step::Events(vec![
            ChatEvent::ToolCall {
                id: "bad".into(),
                name: "read_file".into(),
                arguments: json!({"__invalid_json": "{oops"}),
            },
            ChatEvent::Done { stop_reason: StopReason::ToolUse },
        ]),
        tool("read_file", json!({"path": "README.md"})),
        submit("partial", "read only"),
    ]);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed);
    assert!(!h.root.join("a.txt").exists());
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let st: Vec<ToolCallStatus> = tr.tool_calls.iter().map(|c| c.tool_call.status).collect();
    assert_eq!(
        st,
        vec![
            ToolCallStatus::Denied,
            ToolCallStatus::Failed,
            ToolCallStatus::Failed,
            ToolCallStatus::Succeeded,
            ToolCallStatus::Succeeded
        ]
    );
    assert_eq!(h.task_status(&a).await, TaskStatus::Review, "partial results go to review");
}

#[tokio::test]
async fn shipped_mock_provider_runs_end_to_end() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["write_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let pid = ProviderId::new();
    h.source
        .adapters
        .lock()
        .insert(pid, Arc::new(MockAdapter::new(pid, "Mock", MockOptions { chunk_delay: Duration::ZERO })));
    *h.gateway.chain.lock() = vec![ModelRef { provider_id: pid, model: "mock-smart".into() }];
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    assert!(tr
        .tool_calls
        .iter()
        .any(|c| c.tool_call.tool == "write_file" && c.tool_call.status == ToolCallStatus::Succeeded));
    assert!(tr.messages.iter().any(|m| m.text.contains("Mock (offline demo)")));
}

#[tokio::test]
async fn agents_reply_in_the_ui_language() {
    let h = harness(ApprovalMode::SemiAuto).await;
    h.ui_language.set("ru");
    let tpl = h.template(&["write_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let pid = ProviderId::new();
    h.source
        .adapters
        .lock()
        .insert(pid, Arc::new(MockAdapter::new(pid, "Mock", MockOptions { chunk_delay: Duration::ZERO })));
    *h.gateway.chain.lock() = vec![ModelRef { provider_id: pid, model: "mock-smart".into() }];
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    assert!(tr.messages.iter().any(|m| m.text.contains("Mock (офлайн-демо)")), "{:?}", tr.messages);
    assert!(!tr.messages.iter().any(|m| m.text.contains("Mock (offline demo)")));
}

#[test]
fn system_prompt_states_the_reply_language() {
    let s = crate::reply_language_instruction("ru");
    assert!(s.contains(orkestr_providers::UI_LANGUAGE_MARKER));
    assert!(s.contains("Russian"));
}

#[tokio::test]
async fn no_model_and_bad_spawn_requests_fail_without_side_effects() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file"], WorktreePolicy::SharedWorktree, None).await;
    let base = SpawnAgentParams {
        project_id: h.project,
        template_id: tpl,
        task_id: None,
        new_task: Some(NewTaskSpec { title: "x".into(), ..NewTaskSpec::default() }),
        model: None,
        worktree_policy: None,
        name: None,
        max_turns: None,
        base_branch: None,
        approval_mode: None,
        budget: None,
    };
    assert!(matches!(h.rt.spawn(base.clone()).await, Err(crate::AgentError::NoModel(_))));
    h.provider(vec![]);
    let mut both = base.clone();
    both.task_id = Some(orkestr_domain::TaskId::new());
    assert!(matches!(h.rt.spawn(both).await, Err(crate::AgentError::Invalid(_))));
    let mut turns = base.clone();
    turns.max_turns = Some(0);
    assert!(h.rt.spawn(turns).await.is_err());
    assert_eq!(h.rt.list(Default::default()).await.unwrap().len(), 0);
}

#[tokio::test]
async fn dedicated_worktree_commits_on_the_agent_branch() {
    if orkestr_vcs::GitCli::discover().version().await.is_err() {
        eprintln!("git not available — skipping");
        return;
    }
    let h = harness(ApprovalMode::FullAuto).await;
    let git = |args: &[&str], dir: &Path| {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"], &h.root);
    git(&["-c", "user.name=t", "-c", "user.email=t@e", "add", "-A"], &h.root);
    git(&["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-q", "-m", "init"], &h.root);
    git(&["config", "user.name", "Test"], &h.root);
    git(&["config", "user.email", "test@example.com"], &h.root);
    let tpl = h
        .template(&["write_file", "git_commit", "git_status", "submit_result"], WorktreePolicy::DedicatedWorktree, None)
        .await;
    h.provider(vec![
        tool("write_file", json!({"path": "feature.txt", "content": "agent work\n"})),
        tool("git_commit", json!({"message": "Add feature"})),
        submit("done", "committed"),
    ]);
    let a = h.spawn(tpl, None).await;
    let wt = a.worktree.clone().unwrap();
    assert!(wt.branch.starts_with("agent/backend-"), "{}", wt.branch);
    assert_ne!(PathBuf::from(&wt.path), h.root);
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert!(PathBuf::from(&wt.path).join("feature.txt").is_file());
    assert!(!h.root.join("feature.txt").exists(), "the main checkout is untouched");
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let commit = tr.tool_calls.iter().find(|c| c.tool_call.tool == "git_commit").unwrap();
    assert_eq!(commit.tool_call.status, ToolCallStatus::Succeeded, "{:?}", commit.result);
    let log = std::process::Command::new("git")
        .args(["log", "--format=%s%n%b", "-1", &wt.branch])
        .current_dir(&h.root)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(log.contains("Add feature") && log.contains("Orkestr-Agent"), "{log}");
}

// ---------------------------------------------------------------------------
// CLI coding agents (Claude Code / Codex)
// ---------------------------------------------------------------------------

/// A scripted official CLI agent: writes `writes` into its cwd, then streams
/// `events`.
struct ScriptedCli {
    id: ProviderId,
    writes: Vec<(String, String)>,
    events: Vec<orkestr_providers::CodingAgentEvent>,
    requests: Mutex<Vec<orkestr_providers::CodingAgentRequest>>,
}

#[async_trait]
impl ProviderAdapter for ScriptedCli {
    fn id(&self) -> ProviderId {
        self.id
    }
    fn descriptor(&self) -> ProviderDescriptor {
        MockAdapter::new(self.id, "Scripted CLI", MockOptions::default()).descriptor()
    }
    async fn auth_state(&self) -> AuthStatus {
        AuthStatus::Ready { label: "scripted".into() }
    }
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(Vec::new())
    }
    async fn health(&self) -> HealthReport {
        HealthReport { health: ProviderHealth::Healthy, latency_ms: None, detail: None, version: None }
    }
    async fn chat(&self, _request: ChatRequest, _cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        Err(ProviderError::Unsupported("CLI only".into()))
    }
    async fn run_coding_agent(
        &self,
        request: orkestr_providers::CodingAgentRequest,
        _cancel: CancellationToken,
    ) -> Result<orkestr_providers::CodingAgentStream, ProviderError> {
        for (path, content) in &self.writes {
            let p = Path::new(&request.cwd).join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        self.requests.lock().push(request);
        Ok(futures::stream::iter(self.events.clone()).boxed())
    }
}

impl H {
    fn cli(
        &self,
        kind: ProviderKind,
        writes: Vec<(String, String)>,
        events: Vec<orkestr_providers::CodingAgentEvent>,
    ) -> Arc<ScriptedCli> {
        let s = Arc::new(ScriptedCli { id: ProviderId::new(), writes, events, requests: Mutex::new(Vec::new()) });
        self.source.adapters.lock().insert(s.id, Arc::clone(&s) as Arc<dyn ProviderAdapter>);
        self.gateway.kinds.lock().insert(s.id, kind);
        *self.gateway.chain.lock() = vec![ModelRef { provider_id: s.id, model: "default".into() }];
        s
    }
}

fn cli_text(t: &str) -> orkestr_providers::CodingAgentEvent {
    orkestr_providers::CodingAgentEvent::Chat(ChatEvent::TextDelta(t.into()))
}

fn cli_done() -> orkestr_providers::CodingAgentEvent {
    orkestr_providers::CodingAgentEvent::Chat(ChatEvent::Done { stop_reason: StopReason::EndTurn })
}

#[tokio::test]
async fn cli_agent_takes_a_checkpoint_and_its_task_rolls_back() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let cli = h.cli(
        ProviderKind::ClaudeCode,
        vec![("src/cli.txt".into(), "from the CLI\n".into())],
        vec![
            orkestr_providers::CodingAgentEvent::FileChanged {
                path: "src/cli.txt".into(),
                change: orkestr_providers::FileChangeKind::Added,
            },
            cli_text("Done.\n```json\n{\"status\": \"done\", \"summary\": \"wrote src/cli.txt\"}\n```\n"),
            cli_done(),
        ],
    );
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    // Full Auto + run_command never pre-approves the CLI's own shell: Claude
    // Code asks Orkestr for every shell command over the permission bridge.
    let req = cli.requests.lock()[0].clone();
    assert!(!req.tool_policy.allowed_tools.iter().any(|t| t == "Bash"), "{:?}", req.tool_policy);
    assert!(req.permission_bridge.is_some(), "Claude Code runs with the permission bridge");
    assert!(!req.tool_policy.allow_writes, "edits are asked over the bridge, never auto-accepted");
    let task_id = a.task_id.unwrap();
    let snaps = h.snapshots.list(h.project, Some(task_id), None, 10).await.unwrap();
    let phases: Vec<_> = snaps.iter().filter_map(|s| s.task_phase).collect();
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::Before), "{phases:?}");
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::After), "{phases:?}");
    let rb = h.snapshots.rollback_task(h.project, task_id, None, None, false).await.unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(!h.root.join("src/cli.txt").exists(), "the CLI agent's change was rolled back");
}

#[test]
fn classified_failures_move_a_cli_agent_to_the_next_model() {
    let (a, b, c) = (ProviderId::new(), ProviderId::new(), ProviderId::new());
    let m = |p: ProviderId, n: &str| ModelRef { provider_id: p, model: n.into() };
    let chain = vec![m(a, "claude"), m(b, "disabled"), m(c, "gpt")];
    let usable = |x: &ModelRef| x.provider_id != b;
    let quota = ProviderError::QuotaExceeded("Claude AI usage limit reached".into());
    let next = crate::run::next_after_cli_failure(&chain, &chain[0], &quota, usable);
    assert_eq!(next, Some(m(c, "gpt")), "skips the unusable provider");
    for e in [
        ProviderError::RateLimited { retry_after_secs: Some(60) },
        ProviderError::Server("claude exited with code 1".into()),
        ProviderError::Auth("Claude Code CLI (`claude`) is not installed".into()),
        ProviderError::Auth("Invalid API key · Please run /login".into()),
        ProviderError::Network("connection reset".into()),
    ] {
        assert!(crate::run::next_after_cli_failure(&chain, &chain[0], &e, usable).is_some(), "{e}");
    }
    for e in [ProviderError::BadRequest("bad flag".into()), ProviderError::Cancelled] {
        assert_eq!(crate::run::next_after_cli_failure(&chain, &chain[0], &e, usable), None, "{e}");
    }
    assert_eq!(crate::run::next_after_cli_failure(&chain, &chain[2], &quota, usable), None, "chain exhausted");
}

#[tokio::test]
async fn cli_agent_falls_back_to_the_next_model_when_its_usage_limit_is_hit() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let limited = h.cli(
        ProviderKind::ClaudeCode,
        Vec::new(),
        vec![
            cli_text("Working…"),
            orkestr_providers::CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::QuotaExceeded(
                "Claude AI usage limit reached|1760000000".into(),
            ))),
        ],
    );
    let backup = h.cli(
        ProviderKind::Codex,
        Vec::new(),
        vec![
            cli_text("Done.\n```json\n{\"status\": \"done\", \"summary\": \"finished on the fallback\"}\n```\n"),
            cli_done(),
        ],
    );
    *h.gateway.chain.lock() = vec![
        ModelRef { provider_id: limited.id, model: "default".into() },
        ModelRef { provider_id: backup.id, model: "default".into() },
    ];
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(a.model.as_ref().map(|m| m.provider_id), Some(backup.id), "the agent moved to the fallback model");
    assert_eq!(limited.requests.lock().len(), 1);
    let req = backup.requests.lock()[0].clone();
    assert!(req.prompt.contains("usage limit"), "the fallback gets the structured handoff: {}", req.prompt);
}

#[tokio::test]
async fn claude_bridge_denies_forbidden_commands_and_files_outside_the_worktree() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let perm = |rid: &str, call: &str, tool: &str, input: serde_json::Value| {
        orkestr_providers::CodingAgentEvent::PermissionRequest {
            request_id: rid.into(),
            tool: tool.into(),
            tool_use_id: Some(call.into()),
            input,
            description: None,
            blocked_path: None,
        }
    };
    let started = |call: &str, tool: &str, summary: &str| orkestr_providers::CodingAgentEvent::ToolStarted {
        call_id: call.into(),
        tool: tool.into(),
        summary: summary.into(),
    };
    let finished = |call: &str| orkestr_providers::CodingAgentEvent::ToolFinished {
        call_id: call.into(),
        ok: false,
        summary: "denied".into(),
    };
    let outside = h.root.parent().unwrap().join("elsewhere.txt").to_string_lossy().into_owned();
    let cli = h.cli(
        ProviderKind::ClaudeCode,
        Vec::new(),
        vec![
            started("c1", "Bash", "rm -rf /"),
            perm("r1", "c1", "Bash", json!({"command": "rm -rf /"})),
            finished("c1"),
            started("c2", "Write", &outside),
            perm("r2", "c2", "Write", json!({"file_path": outside, "content": "x"})),
            finished("c2"),
            started("c3", "Bash", "cargo test"),
            perm("r3", "c3", "Bash", json!({"command": "cargo test"})),
            orkestr_providers::CodingAgentEvent::ToolFinished { call_id: "c3".into(), ok: true, summary: "ok".into() },
            cli_text(
                "Done.
```json
{\"status\": \"done\", \"summary\": \"ok\"}
```
",
            ),
            cli_done(),
        ],
    );
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    let req = cli.requests.lock()[0].clone();
    let mut rx = req.permission_bridge.unwrap().take().expect("the scripted CLI left the receiver");
    let r1 = rx.try_recv().unwrap();
    assert_eq!(r1.request_id, "r1");
    assert!(!r1.allow, "a forbidden command is denied");
    let r2 = rx.try_recv().unwrap();
    assert!(!r2.allow && r2.message.unwrap_or_default().contains("worktree"), "outside the worktree is denied");
    let r3 = rx.try_recv().unwrap();
    assert!(r3.allow, "a safe command is allowed in Full Auto");
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let status = |tool: &str| {
        tr.tool_calls.iter().filter(|c| c.tool_call.tool == tool).map(|c| c.tool_call.status).collect::<Vec<_>>()
    };
    assert_eq!(status("Bash"), vec![ToolCallStatus::Denied, ToolCallStatus::Succeeded]);
    assert_eq!(status("Write"), vec![ToolCallStatus::Denied]);
}

/// ADR-0047: Codex approvals use the same bridge (forbidden command and a
/// multi-file change leaving the worktree denied, a safe command allowed),
/// writes need a write tool, and a Codex usage limit falls back to Claude Code.
#[tokio::test]
async fn codex_bridge_decides_approvals_and_falls_back_to_claude_on_its_usage_limit() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let perm = |rid: &str, call: &str, tool: &str, input: serde_json::Value| {
        orkestr_providers::CodingAgentEvent::PermissionRequest {
            request_id: rid.into(),
            tool: tool.into(),
            tool_use_id: Some(call.into()),
            input,
            description: None,
            blocked_path: None,
        }
    };
    let inside = h.root.join("src").join("a.rs").to_string_lossy().into_owned();
    let outside = h.root.parent().unwrap().join("elsewhere.txt").to_string_lossy().into_owned();
    let codex = h.cli(
        ProviderKind::Codex,
        Vec::new(),
        vec![
            perm("r1", "c1", "Bash", json!({"command": "rm -rf /"})),
            perm("r2", "f1", "Edit", json!({"file_path": inside, "paths": [inside, outside]})),
            perm("r3", "c3", "Bash", json!({"command": "cargo test"})),
            orkestr_providers::CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::QuotaExceeded(
                "You've hit your usage limit".into(),
            ))),
        ],
    );
    let claude = h.cli(
        ProviderKind::ClaudeCode,
        Vec::new(),
        vec![
            cli_text("Done.\n```json\n{\"status\": \"done\", \"summary\": \"finished on Claude\"}\n```\n"),
            cli_done(),
        ],
    );
    *h.gateway.chain.lock() = vec![
        ModelRef { provider_id: codex.id, model: "default".into() },
        ModelRef { provider_id: claude.id, model: "sonnet".into() },
    ];
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert_eq!(a.model.as_ref().map(|m| m.provider_id), Some(claude.id), "moved to Claude Code");
    let req = codex.requests.lock()[0].clone();
    assert!(req.tool_policy.allow_writes, "a template with write tools gets the workspace-write sandbox");
    assert!(req.subagent.is_none());
    let mut rx = req.permission_bridge.unwrap().take().expect("receiver left");
    assert!(!rx.try_recv().unwrap().allow, "forbidden command denied");
    let r2 = rx.try_recv().unwrap();
    assert!(!r2.allow && r2.message.unwrap_or_default().contains("worktree"), "a path outside the worktree is denied");
    assert!(rx.try_recv().unwrap().allow, "a safe command is allowed in Full Auto");
    assert!(claude.requests.lock()[0].prompt.contains("usage limit"), "structured handoff");
}

#[tokio::test]
async fn cli_dangerous_command_stops_the_run_and_is_audited() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["write_file", "run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.cli(
        ProviderKind::Codex,
        Vec::new(),
        vec![
            orkestr_providers::CodingAgentEvent::ToolStarted {
                call_id: "c1".into(),
                tool: "Bash".into(),
                summary: "git push --force origin main".into(),
            },
            // Codex (ADR-0047) is bridged: a command it ran without asking
            // is classified when it finishes and stops the run.
            orkestr_providers::CodingAgentEvent::ToolFinished { call_id: "c1".into(), ok: true, summary: "ok".into() },
            cli_text("pushed\n```json\n{\"status\": \"done\", \"summary\": \"pushed\"}\n```\n"),
            cli_done(),
        ],
    );
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Error, "{:?}", a.last_error);
    assert!(a.last_error.as_deref().unwrap_or_default().contains("without asking"), "{:?}", a.last_error);
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let call = tr.tool_calls.iter().find(|c| c.tool_call.tool == "Bash").unwrap();
    assert_eq!(call.tool_call.status, ToolCallStatus::Denied);
    let audit = h
        .sec
        .audit
        .query(orkestr_security::AuditQuery::default())
        .await
        .unwrap()
        .into_iter()
        .any(|r| r.event.action == "agent.cliPostHocViolation");
    assert!(audit, "the dangerous CLI command was audited");
}

#[tokio::test]
async fn mutating_command_alone_gets_a_checkpoint_pair() {
    let h = harness(ApprovalMode::FullAuto).await;
    let tpl = h.template(&["run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![tool("run_command", json!({"command": "echo changed > notes.txt"})), submit("done", "wrote")]);
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    assert!(h.root.join("notes.txt").is_file());
    let task_id = a.task_id.unwrap();
    let snaps = h.snapshots.list(h.project, Some(task_id), None, 10).await.unwrap();
    let phases: Vec<_> = snaps.iter().filter_map(|s| s.task_phase).collect();
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::Before), "{phases:?}");
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::After), "{phases:?}");
    let rb = h.snapshots.rollback_task(h.project, task_id, None, None, false).await.unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(!h.root.join("notes.txt").exists(), "the command's change was rolled back");
}

// ---------------------------------------------------------------------------
// external (plugin) tools — Phase 7, ADR-0034
// ---------------------------------------------------------------------------

struct FakePluginTools {
    calls: Mutex<Vec<(String, Value)>>,
    capability: Option<orkestr_domain::Capability>,
}

#[async_trait]
impl crate::tools::external::ExternalTools for FakePluginTools {
    fn list(&self, _project: &ProjectId) -> Vec<crate::tools::external::ExternalToolDecl> {
        vec![crate::tools::external::ExternalToolDecl {
            name: "plugin_acme__hello".into(),
            description: "Greets".into(),
            input_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
            capability: self.capability,
        }]
    }
    async fn call(&self, tool: &str, input: Value, ctx: crate::tools::external::ToolCallCtx) -> Result<Value, String> {
        assert!(!ctx.cancel.is_cancelled());
        self.calls.lock().push((tool.to_owned(), input.clone()));
        Ok(json!({ "greeting": format!("hi {}", input["name"].as_str().unwrap_or("?")) }))
    }
}

#[tokio::test]
async fn plugin_tools_are_offered_and_dispatched() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let fake = Arc::new(FakePluginTools { calls: Mutex::new(Vec::new()), capability: None });
    h.rt.set_external_tools(Arc::clone(&fake) as Arc<dyn crate::tools::external::ExternalTools>);
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let p = h.provider(vec![
        tool("plugin_acme__hello", json!({"wrong": 1})),
        tool("plugin_acme__hello", json!({"name": "Ada"})),
        submit("done", "greeted"),
    ]);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Completed, "{:?}", a.last_error);
    // Invalid arguments never reach the plugin; the valid call does.
    assert_eq!(fake.calls.lock().clone(), vec![("plugin_acme__hello".to_owned(), json!({"name": "Ada"}))]);
    let reqs = p.requests.lock().clone();
    assert!(reqs[0].tools.iter().any(|t| t.name == "plugin_acme__hello"));
    assert!(reqs[2].messages.iter().any(|m| m.role == orkestr_providers::Role::Tool && m.content.contains("hi Ada")));
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let calls: Vec<_> = tr.tool_calls.iter().filter(|c| c.tool_call.tool == "plugin_acme__hello").collect();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().any(|c| c.tool_call.status == ToolCallStatus::Succeeded));
}

#[tokio::test]
async fn plugin_tool_capability_needs_the_agent_permission() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let fake = Arc::new(FakePluginTools {
        calls: Mutex::new(Vec::new()),
        capability: Some(orkestr_domain::Capability::Network),
    });
    h.rt.set_external_tools(Arc::clone(&fake) as Arc<dyn crate::tools::external::ExternalTools>);
    // The user denies the agent Network for this tool: the plugin is never called.
    h.sec
        .permissions
        .set(
            None,
            orkestr_domain::PermissionRule {
                capability: orkestr_domain::Capability::Network,
                effect: orkestr_domain::PermissionEffect::Deny,
                resource: Some("plugin_acme__hello".into()),
                scope: orkestr_domain::PermissionScope::Global,
            },
            None,
            None,
            h.sec.user(),
        )
        .await
        .unwrap();
    let tpl = h.template(&["submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![tool("plugin_acme__hello", json!({"name": "Ada"})), submit("done", "greeted")]);
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert!(fake.calls.lock().is_empty());
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    assert!(tr
        .tool_calls
        .iter()
        .any(|c| c.tool_call.tool == "plugin_acme__hello" && c.tool_call.status == ToolCallStatus::Denied));
}
