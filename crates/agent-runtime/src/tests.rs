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
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self { id: ProviderId::new(), steps: Mutex::new(steps.into()), requests: Mutex::new(Vec::new()) })
    }
}

#[async_trait]
impl ProviderAdapter for Scripted {
    fn id(&self) -> ProviderId {
        self.id
    }
    fn descriptor(&self) -> ProviderDescriptor {
        MockAdapter::new(self.id, "Mock (offline demo)", MockOptions::default()).descriptor()
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
        let step = self.steps.lock().pop_front().unwrap_or_else(|| {
            Step::Events(vec![
                ChatEvent::TextDelta("script exhausted".into()),
                ChatEvent::Done { stop_reason: StopReason::EndTurn },
            ])
        });
        Ok(match step {
            Step::Events(evs) => futures::stream::iter(evs).boxed(),
            Step::Hang => futures::stream::once(async move {
                cancel.cancelled().await;
                ChatEvent::Error(ProviderError::Cancelled)
            })
            .boxed(),
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
        orkestr_snapshots::ScanOptions::default(),
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

#[tokio::test]
async fn dangerous_command_waits_for_approval_then_runs() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["run_command", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    h.provider(vec![tool("run_command", json!({"command": "git push --force origin main"})), submit("done", "pushed")]);
    let a = h.spawn(tpl, None).await;
    let ap = h.pending_approval(a.id).await;
    h.wait_state(a.id, AgentState::Waiting).await;
    assert_eq!(h.task_status(&a).await, TaskStatus::Waiting);
    assert!(ap.command_preview.as_deref().unwrap().contains("push --force"));
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
    h.provider(vec![
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
    let a = h.spawn(tpl, None).await;
    let a = h.wait_done(a.id).await;
    assert_eq!(a.state, AgentState::Error);
    assert!(a.last_error.as_deref().unwrap().contains("budget_exceeded"), "{:?}", a.last_error);
    assert!((a.usage.cost_usd - 0.5).abs() < 1e-9);
    assert_eq!(a.turns_used, 1, "no further model call after the budget was exhausted");
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
}

#[tokio::test]
async fn fallback_to_a_provider_over_its_budget_is_refused() {
    let h = harness(ApprovalMode::SemiAuto).await;
    let tpl = h.template(&["read_file", "submit_result"], WorktreePolicy::SharedWorktree, None).await;
    let first =
        h.provider(vec![Step::Events(vec![ChatEvent::Error(ProviderError::QuotaExceeded("out of credits".into()))])]);
    let second = Scripted::new(vec![submit("done", "finished on the fallback model")]);
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
    // Full Auto + run_command still never pre-approves the CLI's own shell.
    let req = cli.requests.lock()[0].clone();
    assert!(!req.tool_policy.allowed_tools.iter().any(|t| t == "Bash"), "{:?}", req.tool_policy);
    assert!(req.tool_policy.disallowed_tools.iter().any(|t| t == "Bash"));
    let task_id = a.task_id.unwrap();
    let snaps = h.snapshots.list(h.project, Some(task_id), None, 10).await.unwrap();
    let phases: Vec<_> = snaps.iter().filter_map(|s| s.task_phase).collect();
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::Before), "{phases:?}");
    assert!(phases.contains(&orkestr_domain::TaskSnapshotPhase::After), "{phases:?}");
    let rb = h.snapshots.rollback_task(h.project, task_id, None, None, false).await.unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(!h.root.join("src/cli.txt").exists(), "the CLI agent's change was rolled back");
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
                tool: "command_execution".into(),
                summary: "git push --force origin main".into(),
            },
            cli_text("pushed\n```json\n{\"status\": \"done\", \"summary\": \"pushed\"}\n```\n"),
            cli_done(),
        ],
    );
    let a = h.wait_done(h.spawn(tpl, None).await.id).await;
    assert_eq!(a.state, AgentState::Error, "{:?}", a.last_error);
    assert!(a.last_error.as_deref().unwrap_or_default().contains("dangerous"), "{:?}", a.last_error);
    assert_eq!(h.task_status(&a).await, TaskStatus::Failed);
    let tr = h.rt.transcript(a.id, None).await.unwrap();
    let call = tr.tool_calls.iter().find(|c| c.tool_call.tool == "command_execution").unwrap();
    assert_eq!(call.tool_call.status, ToolCallStatus::Denied);
    let audit = h
        .sec
        .audit
        .query(orkestr_security::AuditQuery::default())
        .await
        .unwrap()
        .into_iter()
        .any(|r| r.event.action == "agent.cliDangerousCommand");
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
