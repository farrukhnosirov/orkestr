//! [`AgentRuntime`]: spawn / stop / pause / list / transcript / handoffs /
//! stats over live runs and persisted instances.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{
    now, AgentId, AgentInstance, AgentRole, AgentState, ExecutionId, ExecutionStatus, ModelRef, PermissionRule,
    PermissionScope, Project, Task, TaskActor, TaskStatus, UsageSummary, WorktreePolicy, WorktreeRef,
};
use orkestr_protocol::event::{
    AgentSpawnedPayload, AgentStateChangedPayload, AgentStoppedPayload, EventKind, StopReason,
};
use orkestr_protocol::methods::agents::{
    AgentStats, AgentStopMode, AgentTranscriptResult, Handoff, ListAgentsParams, SpawnAgentParams, MAX_AGENT_NAME_LEN,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_protocol::NewEvent;
use orkestr_providers::{RequiredCapabilities, RouteRequest};
use orkestr_task_engine::AgentRunUpdate;
use parking_lot::Mutex;

use crate::deps::RuntimeDeps;
use crate::error::{AgentError, AgentResult};
use crate::run::{Run, RunControl, StopCause};
use crate::state::is_final;
use crate::store::{AgentFilter, AgentStore};
use crate::templates::{TemplateService, MAX_TURNS_LIMIT};
use crate::tools::{descriptor, ToolEnv};

/// Orchestrator-only spawn options (not on the wire): team membership,
/// hierarchy parent and narrowing permission rules (§6, §68.27). Phase 2.
#[derive(Debug, Clone, Default)]
pub struct SpawnExtras {
    /// Team the agent belongs to.
    pub team_id: Option<orkestr_domain::AgentTeamId>,
    /// Parent agent in the hierarchy (the team lead).
    pub parent_agent_id: Option<AgentId>,
    /// Extra rules installed at agent scope before the agent runs. `Allow`
    /// rules are ignored: extras only narrow (no privilege escalation).
    pub rules: Vec<PermissionRule>,
    /// Note stored with those rules.
    pub rule_note: Option<String>,
}

struct Live {
    control: Arc<RunControl>,
    project_id: orkestr_domain::ProjectId,
}

/// The agent runtime service (cheap to clone).
#[derive(Clone)]
pub struct AgentRuntime {
    deps: Arc<RuntimeDeps>,
    store: AgentStore,
    templates: TemplateService,
    live: Arc<Mutex<HashMap<AgentId, Live>>>,
}

impl std::fmt::Debug for AgentRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRuntime").field("live", &self.live.lock().len()).finish_non_exhaustive()
    }
}

const ACTIVE_STATES: [AgentState; 7] = [
    AgentState::Idle,
    AgentState::Listening,
    AgentState::Thinking,
    AgentState::Coding,
    AgentState::Testing,
    AgentState::Reviewing,
    AgentState::Waiting,
];

impl AgentRuntime {
    /// Creates the runtime.
    #[must_use]
    pub fn new(deps: RuntimeDeps) -> Self {
        let store = AgentStore::new(deps.db.clone());
        let templates = TemplateService::new(deps.db.clone());
        Self { deps: Arc::new(deps), store, templates, live: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Template service.
    #[must_use]
    pub fn templates(&self) -> &TemplateService {
        &self.templates
    }

    /// Persistence.
    #[must_use]
    pub fn store(&self) -> &AgentStore {
        &self.store
    }

    /// Engines the runtime is wired with (context previews, Phase 2).
    #[must_use]
    pub fn deps(&self) -> &Arc<RuntimeDeps> {
        &self.deps
    }

    /// Seeds built-in templates and marks agents of a previous session that
    /// were still active as stopped (their tasks are interrupted by the task
    /// engine's recovery).
    pub async fn init(&self) -> AgentResult<()> {
        self.templates.seed_builtins().await?;
        let stale = self
            .store
            .list(AgentFilter { states: Some(ACTIVE_STATES.to_vec()), limit: Some(5000), ..AgentFilter::default() })
            .await?;
        for mut a in stale {
            if self.live.lock().contains_key(&a.id) {
                continue;
            }
            let from = a.state;
            a.state = AgentState::Stopped;
            a.stopped_at = Some(now());
            a.activity = Some("Interrupted: Orkestr stopped while the agent was running".into());
            a.last_error = Some("interrupted by a core restart".into());
            self.store.save(&a).await?;
            if let Ok(execs) = self.store.executions(a.id).await {
                for mut e in execs.into_iter().filter(|e| e.status == ExecutionStatus::Running) {
                    e.status = ExecutionStatus::Interrupted;
                    e.finished_at = Some(now());
                    let _ = self.store.save_execution(&e).await;
                }
            }
            crate::run::remove_agent_rules(&self.deps, a.id).await;
            self.publish_stopped(&a, from, StopReason::Shutdown, "interrupted by a core restart").await;
            tracing::warn!(agent_id = %a.id, "stale agent from a previous session marked stopped");
        }
        Ok(())
    }

    async fn publish_stopped(&self, a: &AgentInstance, from: AgentState, reason: StopReason, detail: &str) {
        let scope = |k| NewEvent::new(k).project(a.project_id).agent(a.id);
        if from != a.state {
            self.deps
                .publish(scope(EventKind::AgentStateChanged(AgentStateChangedPayload {
                    agent_id: a.id,
                    from,
                    to: a.state,
                    activity: a.activity.clone(),
                })))
                .await;
        }
        self.deps
            .publish(scope(EventKind::AgentStopped(AgentStoppedPayload {
                agent_id: a.id,
                reason,
                detail: Some(detail.to_owned()),
            })))
            .await;
    }

    /// Whether an agent's run is live in this process.
    #[must_use]
    pub fn is_live(&self, id: AgentId) -> bool {
        self.live.lock().contains_key(&id)
    }

    /// Number of live runs.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.lock().len()
    }

    /// One agent.
    pub async fn get(&self, id: AgentId) -> AgentResult<AgentInstance> {
        self.store.get(id).await
    }

    /// Lists agents, newest first.
    pub async fn list(&self, p: ListAgentsParams) -> AgentResult<Vec<AgentInstance>> {
        self.store
            .list(AgentFilter {
                project_id: p.project_id,
                task_id: p.task_id,
                states: p.active_only.unwrap_or(false).then(|| ACTIVE_STATES.to_vec()),
                limit: Some(500),
            })
            .await
    }

    /// Transcript (messages + tool calls).
    pub async fn transcript(&self, id: AgentId, limit: Option<u32>) -> AgentResult<AgentTranscriptResult> {
        let agent = self.store.get(id).await?;
        let limit = limit.unwrap_or(500).clamp(1, 2000);
        Ok(AgentTranscriptResult {
            agent,
            messages: self.store.messages(id, limit).await?,
            tool_calls: self.store.tool_calls(id, limit).await?,
        })
    }

    /// Handoffs produced by an agent.
    pub async fn handoffs(&self, id: AgentId) -> AgentResult<Vec<Handoff>> {
        self.store.get(id).await?;
        self.store.handoffs(id).await
    }

    /// Aggregate counters.
    pub async fn stats(&self, project: Option<orkestr_domain::ProjectId>) -> AgentResult<AgentStats> {
        let active = {
            let live = self.live.lock();
            live.values().filter(|l| project.is_none_or(|p| l.project_id == p)).count()
        };
        self.store.stats(project, u32::try_from(active).unwrap_or(u32::MAX)).await
    }

    // ------------------------------------------------------------------
    // spawn
    // ------------------------------------------------------------------

    async fn resolve_chain(
        &self,
        project: &Project,
        role: &AgentRole,
        explicit: Vec<ModelRef>,
    ) -> AgentResult<(Vec<ModelRef>, bool)> {
        let models = &self.deps.models;
        let mut chain: Vec<ModelRef> = Vec::new();
        for m in explicit {
            let info = models.provider(m.provider_id).ok_or_else(|| {
                AgentError::Invalid(format!("provider {} of model `{}` is not configured", m.provider_id, m.model))
            })?;
            if !project.privacy.allows_provider(m.provider_id, info.is_local) {
                return Err(AgentError::Forbidden(format!(
                    "model `{}` is not allowed by the project's AI privacy policy",
                    m.model
                )));
            }
            if !chain.contains(&m) {
                chain.push(m);
            }
        }
        let routed = models
            .route(&RouteRequest {
                purpose: Some(crate::prompt::role_name(role)),
                required: RequiredCapabilities { tools: true, ..RequiredCapabilities::default() },
                preferred: chain.first().cloned(),
                fallback_chain: chain.iter().skip(1).cloned().collect(),
                policy: project.privacy.ai.clone(),
                max_cost_usd: None,
                est_input_tokens: 4_000,
                max_output_tokens: u64::from(self.deps.config.max_output_tokens),
                exclude: Vec::new(),
            })
            .await
            .map_err(AgentError::NoModel)?;
        for m in routed {
            if !chain.contains(&m) {
                chain.push(m);
            }
        }
        let Some(head) = chain.first().cloned() else {
            return Err(AgentError::NoModel(
                "no usable model — connect a provider in Settings → AI Providers (the offline Mock provider also works)"
                    .into(),
            ));
        };
        let head_cli = models.provider(head.provider_id).is_some_and(|i| i.is_cli_agent());
        if head_cli {
            // CLI coding agents run their own loop; no chat fallback chain.
            return Ok((vec![head], true));
        }
        chain.retain(|m| models.provider(m.provider_id).is_some_and(|i| !i.is_cli_agent() && i.enabled));
        if chain.is_empty() {
            return Err(AgentError::NoModel("no enabled chat model is available".into()));
        }
        Ok((chain, false))
    }

    async fn resolve_task(&self, p: &SpawnAgentParams) -> AgentResult<Task> {
        let task = match (&p.task_id, &p.new_task) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(AgentError::Invalid("give exactly one of taskId or newTask".into()));
            }
            (Some(id), None) => {
                let t = self.deps.tasks.get(*id).await?;
                if t.project_id != p.project_id {
                    return Err(AgentError::Invalid("the task belongs to another project".into()));
                }
                t
            }
            (None, Some(spec)) => {
                self.deps
                    .tasks
                    .create(
                        CreateTaskParams {
                            project_id: p.project_id,
                            title: spec.title.clone(),
                            description: spec.description.clone(),
                            acceptance_criteria: spec.acceptance_criteria.clone(),
                            priority: spec.priority,
                            ..CreateTaskParams::default()
                        },
                        TaskActor::User,
                    )
                    .await?
            }
        };
        let busy = self
            .store
            .list(AgentFilter {
                task_id: Some(task.id),
                states: Some(ACTIVE_STATES.to_vec()),
                ..AgentFilter::default()
            })
            .await?;
        if let Some(other) = busy.into_iter().find(|a| self.is_live(a.id)) {
            return Err(AgentError::Conflict(format!("task is already being worked on by {}", other.name)));
        }
        let task = match task.status {
            TaskStatus::Ready => task,
            TaskStatus::Planned => {
                self.deps
                    .tasks
                    .transition(task.id, TaskStatus::Ready, TaskActor::User, Some("assigned to an agent".into()), None)
                    .await?
                    .task
            }
            TaskStatus::Failed | TaskStatus::Interrupted => self.deps.tasks.retry(task.id, TaskActor::User).await?.task,
            other => {
                return Err(AgentError::Conflict(format!(
                    "the task is {other:?}; only Planned, Ready, Failed or Interrupted tasks can be assigned"
                )));
            }
        };
        if task.status != TaskStatus::Ready {
            return Err(AgentError::Conflict(format!("the task is {:?} (unfinished dependencies?)", task.status)));
        }
        Ok(task)
    }

    /// Spawns an agent and starts its run.
    pub async fn spawn(&self, p: SpawnAgentParams) -> AgentResult<(AgentInstance, Task)> {
        self.spawn_with(p, SpawnExtras::default()).await
    }

    /// Spawns an agent with orchestrator-only extras (team membership,
    /// hierarchy parent, narrowing permission rules). Phase 2.
    #[allow(clippy::too_many_lines)]
    pub async fn spawn_with(&self, p: SpawnAgentParams, extras: SpawnExtras) -> AgentResult<(AgentInstance, Task)> {
        if let Some(n) = &p.name {
            if n.trim().is_empty() || n.chars().count() > MAX_AGENT_NAME_LEN {
                return Err(AgentError::Invalid(format!("name must be 1..={MAX_AGENT_NAME_LEN} characters")));
            }
        }
        if p.max_turns.is_some_and(|t| t == 0 || t > MAX_TURNS_LIMIT) {
            return Err(AgentError::Invalid(format!("maxTurns must be 1..={MAX_TURNS_LIMIT}")));
        }
        if self.deps.security.emergency.flags().all_stopped {
            return Err(AgentError::Forbidden("Stop All AI is active — reset the emergency controls first".into()));
        }
        if self.live_count() >= self.deps.config.max_running_agents {
            return Err(AgentError::Conflict(format!(
                "{} agents are already running (limit {})",
                self.live_count(),
                self.deps.config.max_running_agents
            )));
        }
        let project = self
            .deps
            .projects
            .get(p.project_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("project {}", p.project_id)))?;
        let template = self.templates.get(p.template_id).await?;
        let policy = p.worktree_policy.unwrap_or(template.worktree_policy);
        let project_root = project.root_path.as_ref().map(PathBuf::from);
        if policy != WorktreePolicy::None && project_root.as_ref().is_none_or(|r| !r.is_dir()) {
            return Err(AgentError::Invalid(
                "the project has no root folder on this machine; set one or use worktree policy None".into(),
            ));
        }
        let explicit: Vec<ModelRef> = p
            .model
            .iter()
            .chain(template.preferred_model.iter())
            .chain(template.fallback_models.iter())
            .cloned()
            .collect();
        let (chain, cli) = self.resolve_chain(&project, &template.role, explicit).await?;
        if cli && policy == WorktreePolicy::None {
            return Err(AgentError::Invalid("CLI coding agents need a workspace (worktree policy None)".into()));
        }
        let task = self.resolve_task(&p).await?;

        let agent_id = AgentId::new();
        let cancel = self
            .deps
            .security
            .emergency
            .register_agent(agent_id, Some(project.id))
            .map_err(|e| AgentError::Forbidden(e.to_string()))?;
        let unregister = |rt: &Self| rt.deps.security.emergency.unregister_agent(agent_id);

        // Workspace per policy.
        let (worktree, own_branch) = match (policy, project_root.as_ref()) {
            (WorktreePolicy::DedicatedWorktree, Some(root)) => {
                let req = orkestr_vcs::AddWorktreeRequest {
                    agent_id: Some(agent_id),
                    role: Some(crate::prompt::role_name(&template.role)),
                    task_id: Some(task.id),
                    branch: None,
                    base: p.base_branch.clone().filter(|b| !b.trim().is_empty()),
                };
                match self.deps.vcs.add_worktree(project.id, root, req).await {
                    Ok(w) => {
                        let branch = w.branch.clone().unwrap_or_default();
                        (Some(WorktreeRef { branch: branch.clone(), path: w.path }), Some(branch))
                    }
                    Err(e) => {
                        unregister(self);
                        return Err(AgentError::Invalid(format!(
                            "could not create the agent worktree: {e}. A dedicated worktree needs a git repository with at least one commit — or choose the Shared worktree policy"
                        )));
                    }
                }
            }
            (WorktreePolicy::SharedWorktree | WorktreePolicy::ReadOnlyMain, Some(root)) => {
                let branch = self.deps.vcs.git().current_branch(root).await.ok().flatten().unwrap_or_default();
                (Some(WorktreeRef { branch, path: root.to_string_lossy().into_owned() }), None)
            }
            _ => (None, None),
        };

        // Tools offered = template allow-list ∩ policy ceiling.
        let tools: Vec<String> = template
            .tools
            .iter()
            .filter(|t| {
                descriptor(t).is_some_and(|d| {
                    !(d.needs_root && policy == WorktreePolicy::None)
                        && !(d.writes && policy == WorktreePolicy::ReadOnlyMain)
                        && !(d.name == "git_commit" && policy != WorktreePolicy::DedicatedWorktree)
                        && !(matches!(d.name, "record_memory" | "record_decision")
                            && template.memory_access == orkestr_domain::MemoryAccess::None)
                })
            })
            .cloned()
            .collect();

        let n = self.store.count_for_template(project.id, &template.id.to_string()).await.unwrap_or(0) + 1;
        let max_turns = p.max_turns.unwrap_or(template.max_turns).clamp(1, MAX_TURNS_LIMIT);
        let agent = AgentInstance {
            id: agent_id,
            template_id: Some(template.id),
            project_id: project.id,
            task_id: Some(task.id),
            team_id: extras.team_id,
            parent_agent_id: extras.parent_agent_id,
            name: p.name.as_deref().map_or_else(|| format!("{} #{n}", template.name), |s| s.trim().to_owned()),
            role: template.role.clone(),
            state: AgentState::Idle,
            model: chain.first().cloned(),
            worktree: worktree.clone(),
            usage: UsageSummary::default(),
            turns_used: 0,
            last_error: None,
            created_at: now(),
            started_at: None,
            stopped_at: None,
            last_activity_at: Some(now()),
            template_version: Some(template.version.clone()),
            worktree_policy: policy,
            max_turns,
            budget: tighten_budget(template.budget.clone(), p.budget.clone()),
            execution_id: None,
            execution_style: Some(if cli { "cli" } else { "chat" }.into()),
            cli_session: None,
            activity: Some("Spawned".into()),
        };
        // Orchestrator scoping (team assignment of the project): only `Deny`
        // / `Ask` rules are accepted, so extras can never widen a grant. They
        // are installed before the agent exists anywhere else (fail closed).
        for rule in extras.rules.iter().filter(|r| r.effect != orkestr_domain::PermissionEffect::Allow) {
            let scoped = PermissionRule { scope: PermissionScope::Agent { agent_id }, ..rule.clone() };
            let note = extras.rule_note.clone().unwrap_or_else(|| "team scope".into());
            if let Err(e) =
                self.deps.security.permissions.set(None, scoped, Some(note), None, self.deps.security.user()).await
            {
                tracing::error!(agent_id = %agent_id, error = %e, "could not install a team scope rule");
                unregister(self);
                return Err(AgentError::Forbidden(format!("could not install the agent's permission scope: {e}")));
            }
        }
        if let Err(e) = self.store.save(&agent).await {
            unregister(self);
            return Err(e);
        }
        self.deps.publish(NewEvent::new(EventKind::AgentSpawned(AgentSpawnedPayload { agent: agent.clone() }))).await;

        // Template grants become Agent-scope rules (the user accepted them by
        // spawning a built-in or by creating / importing the template).
        for rule in &template.permissions {
            let scoped = PermissionRule { scope: PermissionScope::Agent { agent_id }, ..rule.clone() };
            if let Err(e) = self
                .deps
                .security
                .permissions
                .set(None, scoped, Some(format!("template grant: {}", template.name)), None, self.deps.security.user())
                .await
            {
                tracing::warn!(agent_id = %agent_id, error = %e, "could not install a template permission rule");
            }
        }
        let task = self
            .deps
            .tasks
            .record_agent_run(
                task.id,
                AgentRunUpdate {
                    agent_id: Some(agent_id),
                    model: agent.model.clone(),
                    worktree: worktree.clone(),
                    ..AgentRunUpdate::default()
                },
            )
            .await?;

        let project_mode = project.approval_mode;
        let approval_mode = self
            .deps
            .security
            .permissions
            .effective_mode(project_mode, Some(agent_id))
            .await
            .unwrap_or_else(|_| self.deps.security.engine.effective_mode(project_mode));
        let approval_mode = match p.approval_mode {
            Some(cap) => stricter_mode(approval_mode, cap),
            None => approval_mode,
        };
        let env = ToolEnv {
            agent_id,
            project_id: project.id,
            task_id: task.id,
            execution_id: ExecutionId::new(),
            root: worktree.as_ref().filter(|_| policy != WorktreePolicy::None).map(|w| PathBuf::from(&w.path)),
            own_branch: own_branch.filter(|b| !b.is_empty()),
            deps: Arc::clone(&self.deps),
        };
        let control = Arc::new(RunControl::new(cancel));
        let run = Run::new(
            Arc::clone(&self.deps),
            self.store.clone(),
            Arc::clone(&control),
            agent.clone(),
            template,
            project.clone(),
            task.clone(),
            chain,
            cli,
            env,
            approval_mode,
            tools,
        );
        self.live.lock().insert(agent_id, Live { control, project_id: project.id });
        let live = Arc::clone(&self.live);
        tokio::spawn(async move {
            run.drive().await;
            live.lock().remove(&agent_id);
        });
        tracing::info!(%agent_id, project_id = %project.id, task_id = %task.id, cli, "agent spawned");
        Ok((agent, task))
    }

    // ------------------------------------------------------------------
    // control
    // ------------------------------------------------------------------

    fn control(&self, id: AgentId) -> Option<Arc<RunControl>> {
        self.live.lock().get(&id).map(|l| Arc::clone(&l.control))
    }

    /// Stops an agent locally (no provider round-trip): cancels its token,
    /// kills its commands, cancels its pending approvals and waits ≤ 10 s for
    /// the run to wind down.
    pub async fn stop(&self, id: AgentId, mode: AgentStopMode) -> AgentResult<AgentInstance> {
        match self.control(id) {
            Some(control) => {
                control.request_stop(StopCause::User(mode));
                let _ = self
                    .deps
                    .security
                    .approvals
                    .cancel_scope(orkestr_security::CancelScope::Agent(id), "agent stopped")
                    .await;
                self.deps.terminals.stop_agent(id).await;
                let mut rx = control.finished.subscribe();
                let _ = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|d| *d)).await;
                self.store.get(id).await
            }
            None => {
                let mut a = self.store.get(id).await?;
                if a.state != AgentState::Stopped {
                    let from = a.state;
                    a.state = AgentState::Stopped;
                    a.stopped_at.get_or_insert_with(now);
                    a.activity = Some("Stopped".into());
                    self.store.save(&a).await?;
                    self.publish_stopped(&a, from, StopReason::UserRequested, "stopped by the user").await;
                }
                Ok(a)
            }
        }
    }

    /// Pauses a live chat agent at its next turn / tool boundary.
    pub async fn pause(&self, id: AgentId) -> AgentResult<AgentInstance> {
        let a = self.store.get(id).await?;
        let control = self.control(id).ok_or_else(|| AgentError::Conflict(format!("{} is not running", a.name)))?;
        if a.execution_style.as_deref() == Some("cli") {
            return Err(AgentError::Unimplemented(
                "pausing CLI coding agents (Claude Code / Codex run their own loop; stop them instead)".into(),
            ));
        }
        let _ = control.paused.send(true);
        Ok(a)
    }

    /// Resumes a paused agent.
    pub async fn resume(&self, id: AgentId) -> AgentResult<AgentInstance> {
        let a = self.store.get(id).await?;
        let control = self.control(id).ok_or_else(|| AgentError::Conflict(format!("{} is not running", a.name)))?;
        let _ = control.paused.send(false);
        Ok(a)
    }

    /// Stops every live run (core shutdown); waits ≤ 5 s.
    pub async fn shutdown(&self) {
        let controls: Vec<Arc<RunControl>> = self.live.lock().values().map(|l| Arc::clone(&l.control)).collect();
        for c in &controls {
            c.request_stop(StopCause::Shutdown);
        }
        for c in controls {
            let mut rx = c.finished.subscribe();
            let _ = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|d| *d)).await;
        }
    }

    /// Whether the agent's run has ended (persisted state).
    pub async fn is_finished(&self, id: AgentId) -> AgentResult<bool> {
        Ok(!self.is_live(id) && is_final(self.store.get(id).await?.state))
    }
}

/// Permissiveness rank of an approval mode (`Ask` < `SemiAuto` < `FullAuto`).
fn mode_rank(m: orkestr_domain::ApprovalMode) -> u8 {
    match m {
        orkestr_domain::ApprovalMode::Ask => 0,
        orkestr_domain::ApprovalMode::SemiAuto => 1,
        orkestr_domain::ApprovalMode::FullAuto => 2,
    }
}

/// The stricter of two approval modes.
#[must_use]
pub fn stricter_mode(a: orkestr_domain::ApprovalMode, b: orkestr_domain::ApprovalMode) -> orkestr_domain::ApprovalMode {
    if mode_rank(b) < mode_rank(a) {
        b
    } else {
        a
    }
}

/// Merges a caller budget cap into the template budget: every limit only
/// tightens (the smaller value wins; a missing value never loosens).
#[must_use]
pub fn tighten_budget(
    template: Option<orkestr_domain::Budget>,
    cap: Option<orkestr_domain::Budget>,
) -> Option<orkestr_domain::Budget> {
    fn min_opt<T: PartialOrd + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
        match (a, b) {
            (Some(x), Some(y)) => Some(if y < x { y } else { x }),
            (x, None) => x,
            (None, y) => y,
        }
    }
    match (template, cap) {
        (t, None) => t,
        (None, c) => c,
        (Some(t), Some(c)) => Some(orkestr_domain::Budget {
            max_cost_usd: min_opt(t.max_cost_usd, c.max_cost_usd),
            max_tokens: min_opt(t.max_tokens, c.max_tokens),
            max_duration_secs: min_opt(t.max_duration_secs, c.max_duration_secs),
            period: t.period,
            warn_at: t.warn_at.min(c.warn_at),
        }),
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;
    use orkestr_domain::{ApprovalMode, Budget, BudgetPeriod};

    #[test]
    fn caps_only_tighten() {
        assert_eq!(stricter_mode(ApprovalMode::FullAuto, ApprovalMode::Ask), ApprovalMode::Ask);
        assert_eq!(stricter_mode(ApprovalMode::Ask, ApprovalMode::FullAuto), ApprovalMode::Ask);
        assert_eq!(stricter_mode(ApprovalMode::SemiAuto, ApprovalMode::FullAuto), ApprovalMode::SemiAuto);
        let t = Budget { max_cost_usd: Some(5.0), ..Budget::unrestricted(BudgetPeriod::PerTask) };
        let c = Budget { max_cost_usd: Some(1.0), max_tokens: Some(10), ..Budget::unrestricted(BudgetPeriod::PerTask) };
        let m = tighten_budget(Some(t.clone()), Some(c)).unwrap();
        assert_eq!(m.max_cost_usd, Some(1.0));
        assert_eq!(m.max_tokens, Some(10));
        let loose = Budget { max_cost_usd: Some(50.0), ..Budget::unrestricted(BudgetPeriod::PerTask) };
        assert_eq!(tighten_budget(Some(t.clone()), Some(loose)).unwrap().max_cost_usd, Some(5.0));
        assert_eq!(tighten_budget(Some(t.clone()), None), Some(t));
    }
}
