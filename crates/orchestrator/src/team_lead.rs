//! Hierarchy (§6): Manager → Team Lead → Executors → Reviewers.
//!
//! When a Manager run delegates a plan task to a team whose policy enables
//! the hierarchy, the run loop hands the task to [`delegate`]. The lead
//! (an orchestrator-driven agent record of the team's lead template):
//!
//! 1. decomposes the task into executor sub-tasks with the lead model
//!    (structured output, validated, one repair round; bounded fan-out);
//! 2. creates them in the task engine as children of the delegated task
//!    (`parentId`) with their dependencies;
//! 3. spawns one executor per ready sub-task from the team's members —
//!    within the team's parallel cap, attempts per sub-task, the budget it
//!    inherited from the run (split between the sub-tasks) and its time
//!    limit — each with the team's permission scope for the project and the
//!    lead as `parentAgentId`;
//! 4. accepts executor results, retries failures within bounds, then asks a
//!    reviewer member to check the combined work;
//! 5. reports to the run: the delegated task goes to `Review` with the
//!    lead's summary (the run applies its approval policy) or `Failed`.
//!
//! Depth is bounded: a lead is level 1 below the Manager, its executors and
//! reviewer level 2; executors are ordinary agents and cannot delegate.
//! Emergency stop, run cancel, pause and shutdown are honoured.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use orkestr_agent_runtime::runtime::stricter_mode;
use orkestr_agent_runtime::{AgentError, SpawnExtras};
use orkestr_domain::{
    now, AgentId, AgentInstance, AgentRole, AgentState, ApprovalMode, Budget, BudgetPeriod, ManagerRunId, ModelRef,
    Project, TaskActor, TaskId, TaskResult, TaskStatus, TeamMember, UsageSummary, WorktreePolicy,
};
use orkestr_protocol::event::{AgentSpawnedPayload, AgentStateChangedPayload, EventKind};
use orkestr_protocol::methods::agents::{AgentStopMode, SpawnAgentParams};
use orkestr_protocol::methods::tasks::{CreateTaskParams, MAX_CRITERION_LEN, MAX_DESCRIPTION_LEN, MAX_TITLE_LEN};
use orkestr_protocol::methods::teams::{TeamLeadRun, TeamLeadStatus, TeamLeadSubtask, MAX_HIERARCHY_DEPTH};
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::mock_teams_pm::TEAM_LEAD_MARKER;
use orkestr_task_engine::AgentRunUpdate;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::run::{RunControl, StopKind};
use crate::service::{Inner, MessageExtras};
use crate::structured::StructuredCall;
use crate::teams::{pick_member, scope_rules, LeadRequest};
use crate::util::{cap, is_ru, one_line, role_name, ru_plural};

/// Structured-output schema name of a lead decomposition.
pub const LEAD_SCHEMA_NAME: &str = "orkestr_team_lead_plan";

/// JSON schema of a lead decomposition.
#[must_use]
pub fn lead_schema(max: u32) -> Value {
    json!({
        "type": "object",
        "required": ["summary", "subtasks"],
        "properties": {
            "summary": {"type": "string"},
            "subtasks": {
                "type": "array",
                "minItems": 1,
                "maxItems": max,
                "items": {
                    "type": "object",
                    "required": ["key", "title", "role", "dependsOn"],
                    "properties": {
                        "key": {"type": "string"},
                        "title": {"type": "string"},
                        "description": {"type": "string"},
                        "role": {"type": "string"},
                        "acceptanceCriteria": {"type": "array", "items": {"type": "string"}},
                        "dependsOn": {"type": "array", "items": {"type": "string"}},
                        "files": {"type": "array", "items": {"type": "string"}}
                    }
                }
            }
        }
    })
}

/// One normalised executor sub-task.
#[derive(Debug, Clone, PartialEq)]
pub struct SubSpec {
    /// Key.
    pub key: String,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Acceptance criteria.
    pub acceptance: Vec<String>,
    /// Role asked for.
    pub role: AgentRole,
    /// Member that executes it.
    pub member: TeamMember,
    /// Keys of prerequisites.
    pub depends_on: Vec<String>,
    /// Expected files.
    pub files: Vec<String>,
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.trim().to_owned()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `deps` (key → prerequisite keys) contain a cycle.
#[must_use]
pub fn has_cycle(deps: &BTreeMap<String, Vec<String>>) -> bool {
    let mut done: HashSet<&str> = HashSet::new();
    loop {
        let ready: Vec<&str> = deps
            .iter()
            .filter(|(k, ds)| !done.contains(k.as_str()) && ds.iter().all(|d| done.contains(d.as_str())))
            .map(|(k, _)| k.as_str())
            .collect();
        if ready.is_empty() {
            return done.len() != deps.len();
        }
        done.extend(ready);
    }
}

/// Validates and normalises a lead decomposition: 1..=`max_fan_out`
/// sub-tasks (extra ones are dropped with a warning), unique keys, known
/// acyclic dependencies, every role mapped onto an executor member.
pub fn normalize_subtasks(
    doc: &Value,
    members: &[(TeamMember, AgentRole)],
    max_fan_out: u32,
) -> OrchResult<(String, Vec<SubSpec>, Vec<String>)> {
    let raw = doc
        .get("subtasks")
        .and_then(Value::as_array)
        .ok_or_else(|| OrchestratorError::Planning("`subtasks` must be an array".into()))?;
    if raw.is_empty() {
        return Err(OrchestratorError::Planning("the decomposition has no sub-tasks".into()));
    }
    let mut warnings = Vec::new();
    let max = max_fan_out.max(1) as usize;
    if raw.len() > max {
        warnings.push(format!("{} sub-tasks proposed; only the first {max} are kept (fan-out limit)", raw.len()));
    }
    let mut out: Vec<SubSpec> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (i, t) in raw.iter().take(max).enumerate() {
        let mut key = t
            .get("key")
            .and_then(|k| k.as_str().map(str::to_owned).or_else(|| k.as_u64().map(|n| n.to_string())))
            .map(|k| cap(k.trim(), 40))
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| format!("s{}", i + 1));
        if seen.contains(&key) {
            key = format!("{key}-{}", i + 1);
            warnings.push(format!("duplicate sub-task key renamed to {key}"));
        }
        seen.insert(key.clone());
        let title = t.get("title").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let Some(title) = title else {
            return Err(OrchestratorError::Planning(format!("sub-task {key} has no title")));
        };
        let wanted = t
            .get("role")
            .and_then(Value::as_str)
            .and_then(crate::plan::parse_role)
            .unwrap_or(AgentRole::Backend);
        // Reviewers are reserved for the final review step.
        let lookup = if wanted == AgentRole::Reviewer { AgentRole::Qa } else { wanted.clone() };
        let execs: Vec<(TeamMember, AgentRole)> =
            members.iter().filter(|(m, r)| !m.is_lead && *r != AgentRole::Reviewer).cloned().collect();
        let Some((member, role)) = pick_member(&execs, &lookup).cloned() else {
            return Err(OrchestratorError::Invalid("the team has no executor member".into()));
        };
        if role != wanted {
            warnings.push(format!("{key}: no {} member — {} does it", role_name(&wanted), role_name(&role)));
        }
        out.push(SubSpec {
            key,
            title: cap(title, MAX_TITLE_LEN),
            description: cap(t.get("description").and_then(Value::as_str).unwrap_or("").trim(), MAX_DESCRIPTION_LEN),
            acceptance: strs(t.get("acceptanceCriteria")).into_iter().take(20).map(|c| cap(&c, MAX_CRITERION_LEN)).collect(),
            role,
            member,
            depends_on: strs(t.get("dependsOn")),
            files: strs(t.get("files"))
                .into_iter()
                .filter(|f| !f.starts_with('/') && !f.contains(':') && !f.split(['/', '\\']).any(|s| s == ".."))
                .take(50)
                .collect(),
        });
    }
    let keys: HashSet<String> = out.iter().map(|s| s.key.clone()).collect();
    for s in &mut out {
        let before = s.depends_on.len();
        let me = s.key.clone();
        s.depends_on.retain(|d| keys.contains(d) && *d != me);
        s.depends_on.dedup();
        if s.depends_on.len() != before {
            warnings.push(format!("{}: unknown or dropped dependencies removed", s.key));
        }
    }
    let graph: BTreeMap<String, Vec<String>> = out.iter().map(|s| (s.key.clone(), s.depends_on.clone())).collect();
    if has_cycle(&graph) {
        return Err(OrchestratorError::Planning("the sub-task dependencies contain a cycle".into()));
    }
    let summary = doc.get("summary").and_then(Value::as_str).map(|s| cap(s.trim(), 2000)).unwrap_or_default();
    Ok((summary, out, warnings))
}

/// Lead agents currently working (their records stand in for the delegated
/// task's agent in the run loop, which treats them as live).
static LIVE_LEADS: std::sync::LazyLock<std::sync::Mutex<HashSet<AgentId>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Whether a team lead agent is still working on its delegated task.
#[must_use]
pub fn is_lead_live(agent: AgentId) -> bool {
    LIVE_LEADS.lock().is_ok_and(|s| s.contains(&agent))
}

/// Registration of a working lead; removed when the lead's driver ends
/// (also on panic).
struct LiveLead(AgentId);

impl LiveLead {
    fn register(agent: AgentId) -> Self {
        if let Ok(mut s) = LIVE_LEADS.lock() {
            s.insert(agent);
        }
        Self(agent)
    }
}

impl Drop for LiveLead {
    fn drop(&mut self) {
        if let Ok(mut s) = LIVE_LEADS.lock() {
            s.remove(&self.0);
        }
    }
}

/// Context of a delegation (from the run loop).
pub(crate) struct Delegation {
    /// The delegated plan task (must be `Ready`).
    pub parent_task: TaskId,
    /// Run that delegates.
    pub run_id: Option<ManagerRunId>,
    /// Budget left in the run (USD).
    pub budget_usd: Option<f64>,
    /// The run's approval policy (team policy can only tighten it).
    pub approval_mode: ApprovalMode,
    /// Model override of the request (Expert mode).
    pub model: Option<ModelRef>,
    /// The run's control block (stop requests, pause, emergency token).
    pub control: Option<Arc<RunControl>>,
    /// Cancellation of this lead (child of the run's emergency token).
    pub cancel: CancellationToken,
}

enum End {
    Done(String),
    Failed(String),
    Cancelled(String),
    Interrupted(String),
}

/// Hands a task to the team lead. Returns the lead's agent id; the lead
/// works in the background and reports through the task's status.
pub(crate) async fn delegate(inner: Arc<Inner>, req: LeadRequest, d: Delegation) -> OrchResult<AgentId> {
    if req.depth == 0 || req.depth >= MAX_HIERARCHY_DEPTH {
        return Err(OrchestratorError::Invalid(format!(
            "hierarchy depth {} is not allowed: a lead works at level 1 and its executors at level {MAX_HIERARCHY_DEPTH}",
            req.depth
        )));
    }
    if inner.deps.security.emergency.flags().all_stopped {
        return Err(OrchestratorError::Forbidden("Stop All AI is active — reset the emergency controls first".into()));
    }
    let members = inner.member_roles(&req.team).await;
    let Some((lead_member, lead_role)) = members.iter().find(|(m, _)| m.is_lead).cloned() else {
        return Err(OrchestratorError::Invalid(format!("team \"{}\" has no lead", req.team.name)));
    };
    let lead_template = inner.deps.agents.templates().get(lead_member.template_id).await?;
    let task = inner.deps.tasks.get(d.parent_task).await?;
    if task.project_id != req.assignment.project_id {
        return Err(OrchestratorError::Forbidden("the team's assignment is for another project".into()));
    }
    if task.status != TaskStatus::Ready {
        return Err(OrchestratorError::Conflict(format!("the task is {:?}, not Ready", task.status)));
    }
    let project = inner.project(task.project_id).await?;
    inner
        .deps
        .tasks
        .transition(
            task.id,
            TaskStatus::Running,
            TaskActor::Manager,
            Some(cap(&format!("delegated to the lead of {}", req.team.name), 300)),
            None,
        )
        .await?;
    let at = now();
    let agent = AgentInstance {
        id: AgentId::new(),
        template_id: Some(lead_template.id),
        project_id: project.id,
        task_id: Some(task.id),
        team_id: Some(req.team.id),
        parent_agent_id: None,
        name: cap(&format!("{} — {}", req.team.name, lead_template.name), 80),
        role: lead_role,
        state: AgentState::Thinking,
        model: lead_member.model.clone().or_else(|| d.model.clone()).or(lead_template.preferred_model.clone()),
        worktree: None,
        usage: UsageSummary::default(),
        turns_used: 0,
        last_error: None,
        created_at: at,
        started_at: Some(at),
        stopped_at: None,
        last_activity_at: Some(at),
        template_version: Some(lead_template.version.clone()),
        worktree_policy: WorktreePolicy::None,
        max_turns: 1,
        budget: None,
        execution_id: None,
        execution_style: Some("lead".into()),
        cli_session: None,
        activity: Some("Decomposing the task".into()),
    };
    inner.deps.agents.store().save(&agent).await?;
    inner.deps.publish(NewEvent::new(EventKind::AgentSpawned(AgentSpawnedPayload { agent: agent.clone() }))).await;
    if let Err(e) = inner
        .deps
        .tasks
        .record_agent_run(task.id, AgentRunUpdate { agent_id: Some(agent.id), ..AgentRunUpdate::default() })
        .await
    {
        tracing::warn!(error = %e, "could not record the lead on its task");
    }
    let budget = match (d.budget_usd, req.team.policies.budget_usd) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let lead = TeamLeadRun {
        id: uuid::Uuid::now_v7().to_string(),
        team_id: req.team.id,
        project_id: project.id,
        run_id: d.run_id,
        parent_task_id: task.id,
        lead_agent_id: Some(agent.id),
        title: task.title.clone(),
        status: TeamLeadStatus::Planning,
        status_reason: None,
        depth: req.depth,
        subtasks: Vec::new(),
        budget_usd: budget,
        cost_usd: 0.0,
        summary: None,
        warnings: Vec::new(),
        model: agent.model.clone(),
        created_at: at,
        updated_at: at,
        finished_at: None,
    };
    inner.save_lead(&lead).await;
    tracing::info!(lead = %lead.id, team = %req.team.id, task = %task.id, "task delegated to a team lead");
    let id = agent.id;
    let driver = LeadDriver {
        inner,
        req,
        d,
        lead,
        agent,
        task_description: task.description.clone(),
        task_criteria: task.acceptance_criteria.iter().map(|c| c.text.clone()).collect(),
        project,
        members,
        started: Instant::now(),
        git: None,
        rolled_up: 0.0,
        model_cost: 0.0,
        live: BTreeMap::new(),
    };
    let live = LiveLead::register(id);
    tokio::spawn(async move {
        driver.drive().await;
        drop(live);
    });
    Ok(id)
}

struct LeadDriver {
    inner: Arc<Inner>,
    req: LeadRequest,
    d: Delegation,
    lead: TeamLeadRun,
    agent: AgentInstance,
    task_description: String,
    task_criteria: Vec<String>,
    project: Project,
    members: Vec<(TeamMember, AgentRole)>,
    started: Instant,
    git: Option<bool>,
    /// Sub-task cost already added to the parent task.
    rolled_up: f64,
    model_cost: f64,
    /// Sub-task key → member that executes it.
    live: BTreeMap<String, TeamMember>,
}

impl LeadDriver {
    fn ru(&self) -> bool {
        is_ru(&self.inner.deps.config.ui_language.get())
    }

    async fn set_state(&mut self, to: AgentState, activity: &str) {
        let from = self.agent.state;
        self.agent.state = to;
        self.agent.activity = Some(activity.to_owned());
        self.agent.last_activity_at = Some(now());
        if to.is_finished() {
            self.agent.stopped_at = Some(now());
        }
        if let Err(e) = self.inner.deps.agents.store().save(&self.agent).await {
            tracing::warn!(error = %e, "could not save the lead agent");
        }
        if from != to {
            self.inner
                .deps
                .publish(
                    NewEvent::new(EventKind::AgentStateChanged(AgentStateChangedPayload {
                        agent_id: self.agent.id,
                        from,
                        to,
                        activity: self.agent.activity.clone(),
                    }))
                    .project(self.project.id)
                    .agent(self.agent.id),
                )
                .await;
        }
    }

    async fn publish(&mut self) {
        self.lead.updated_at = now();
        self.inner.save_lead(&self.lead).await;
    }

    async fn drive(mut self) {
        let end = self.phases().await;
        self.finish(end).await;
    }

    async fn phases(&mut self) -> End {
        // 1. decompose
        let subs = match self.decompose().await {
            Ok(s) => s,
            Err(e) => {
                if self.d.cancel.is_cancelled() {
                    return End::Cancelled("emergency stop".into());
                }
                return End::Failed(format!("the lead could not decompose the task: {e}"));
            }
        };
        // 2. sub-tasks
        if let Err(e) = self.create_subtasks(&subs).await {
            return End::Failed(format!("could not create the sub-tasks: {e}"));
        }
        let n = subs.len();
        let text = if self.ru() {
            format!(
                "Лид команды «{}» разбил задачу «{}» на {n} {}.",
                self.req.team.name,
                self.lead.title,
                ru_plural(n, "подзадачу", "подзадачи", "подзадач")
            )
        } else {
            format!("The {} lead split \"{}\" into {n} sub-task(s).", self.req.team.name, self.lead.title)
        };
        self.inner
            .system_event(self.project.id, &text, MessageExtras { run_id: self.d.run_id, ..MessageExtras::default() })
            .await;
        // 3. executors
        self.lead.status = TeamLeadStatus::Executing;
        self.set_state(AgentState::Waiting, "Coordinating executors").await;
        self.publish().await;
        let executors: Vec<usize> = (0..self.lead.subtasks.len()).collect();
        if let Some(end) = self.run_until_done(&executors, self.req.team.policies.max_attempts).await {
            return end;
        }
        // 4. review
        let reviewer = self.members.iter().find(|(m, r)| !m.is_lead && *r == AgentRole::Reviewer).cloned();
        if self.req.team.policies.require_review {
            if let Some((member, role)) = reviewer {
                self.lead.status = TeamLeadStatus::Reviewing;
                self.set_state(AgentState::Reviewing, "Waiting for the reviewer").await;
                match self.create_review(&member, role).await {
                    Ok(i) => {
                        if let Some(end) = self.run_until_done(&[i], 1).await {
                            return end;
                        }
                    }
                    Err(e) => return End::Failed(format!("could not create the review sub-task: {e}")),
                }
            } else {
                self.lead.warnings.push("review requested but the team has no reviewer member".into());
            }
        }
        let summary = self.result_summary().await;
        End::Done(summary)
    }

    async fn decompose(&mut self) -> OrchResult<Vec<SubSpec>> {
        let policies = &self.req.team.policies;
        let max = policies.max_fan_out;
        let roles: Vec<String> = {
            let mut v: Vec<String> = self
                .members
                .iter()
                .filter(|(m, r)| !m.is_lead && *r != AgentRole::Reviewer)
                .map(|(_, r)| role_name(r))
                .collect();
            v.dedup();
            v
        };
        let language = self.inner.deps.config.ui_language.get();
        let mut system = format!(
            "{TEAM_LEAD_MARKER} You are the team lead of the \"{}\" agent team in Orkestr (hierarchy level 1 below the AI Manager). \
             Decompose the delegated task into at most {max} concrete sub-tasks for your executors. Each sub-task is done by ONE \
             executor alone in its own git worktree; give it testable acceptance criteria, one role from the executor roles, the \
             keys of the sub-tasks it really depends on and the files it will likely touch. Independent sub-tasks run in parallel. \
             Do not add a review sub-task: a reviewer checks the combined result afterwards.\n\n",
            self.req.team.name
        );
        system.push_str(&orkestr_agent_runtime::reply_language_instruction(&language));
        system.push_str("- Field names and keys stay in English.\n");
        let mut user = format!("Task: {}\n", self.lead.title);
        if !self.task_description.trim().is_empty() {
            user.push_str(&format!("Description: {}\n", cap(&self.task_description, 4000)));
        }
        if !self.task_criteria.is_empty() {
            user.push_str("Acceptance criteria:\n");
            for c in &self.task_criteria {
                user.push_str(&format!("- {}\n", one_line(c, 300)));
            }
        }
        user.push_str(&format!("Executor roles: {}\nMax sub-tasks: {max}\n", roles.join(", ")));
        // Context of THIS project only (cross-project isolation, §68.27).
        user.push_str(&format!("\nProject: {}\n", self.project.name));
        if !self.project.description.trim().is_empty() {
            user.push_str(&format!("Project description: {}\n", one_line(&self.project.description, 400)));
        }
        if let Ok(rules) = self.inner.deps.memory.effective_rules(Some(self.project.id), None).await {
            let own: Vec<_> = rules
                .iter()
                .filter(|r| match &r.item.scope {
                    orkestr_domain::MemoryScope::Global => true,
                    orkestr_domain::MemoryScope::Project { project_id } => *project_id == self.project.id,
                    _ => false,
                })
                .collect();
            if !own.is_empty() {
                user.push_str("Project rules:\n");
                for r in own.iter().take(10) {
                    user.push_str(&format!("- {}: {}\n", r.item.title, one_line(&r.item.content, 200)));
                }
            }
        }
        let explicit = self
            .members
            .iter()
            .find(|(m, _)| m.is_lead)
            .and_then(|(m, _)| m.model.clone())
            .or_else(|| self.d.model.clone());
        let members = self.members.clone();
        let call = StructuredCall {
            project: &self.project,
            model: explicit.as_ref(),
            system,
            user,
            schema_name: LEAD_SCHEMA_NAME,
            schema: lead_schema(max),
            key: "subtasks",
        };
        let out = self.inner.structured_call(call, &self.d.cancel, |v| normalize_subtasks(v, &members, max)).await?;
        let (summary, subs, warnings) = out.value;
        self.model_cost += out.usage.cost_usd;
        self.lead.model = out.model.clone().or(self.lead.model.take());
        self.lead.summary = (!summary.is_empty()).then_some(summary);
        self.lead.warnings.extend(warnings);
        if out.repaired {
            self.lead.warnings.push("the first decomposition was invalid and was repaired".into());
        }
        if let Err(e) = self
            .inner
            .deps
            .tasks
            .record_agent_run(
                self.lead.parent_task_id,
                AgentRunUpdate { usage_delta: Some(out.usage), model: out.model, ..AgentRunUpdate::default() },
            )
            .await
        {
            tracing::warn!(error = %e, "could not record the lead's usage");
        }
        Ok(subs)
    }

    async fn create_subtasks(&mut self, subs: &[SubSpec]) -> OrchResult<()> {
        // Topological order so prerequisites exist first.
        let mut created: HashMap<String, TaskId> = HashMap::new();
        let mut remaining: Vec<&SubSpec> = subs.iter().collect();
        while !remaining.is_empty() {
            let before = remaining.len();
            let mut next = Vec::new();
            for s in remaining {
                if !s.depends_on.iter().all(|d| created.contains_key(d)) {
                    next.push(s);
                    continue;
                }
                let params = CreateTaskParams {
                    project_id: self.project.id,
                    title: s.title.clone(),
                    description: Some(s.description.clone()),
                    acceptance_criteria: Some(s.acceptance.clone()),
                    parent_id: Some(self.lead.parent_task_id),
                    depends_on: Some(s.depends_on.iter().filter_map(|d| created.get(d).copied()).collect()),
                    tags: Some(vec![
                        "team".into(),
                        cap(&format!("team:{}", self.req.team.name), 40),
                        cap(&role_name(&s.role), 40),
                    ]),
                    files: (!s.files.is_empty()).then(|| s.files.clone()),
                    ..CreateTaskParams::default()
                };
                let task = self.inner.deps.tasks.create(params, TaskActor::Manager).await?;
                created.insert(s.key.clone(), task.id);
                self.live.insert(s.key.clone(), s.member.clone());
                self.lead.subtasks.push(TeamLeadSubtask {
                    task_id: task.id,
                    key: s.key.clone(),
                    title: task.title,
                    role: s.role.clone(),
                    template_id: s.member.template_id,
                    status: task.status,
                    agent_id: None,
                    attempts: 0,
                    is_review: false,
                    note: None,
                });
            }
            if next.len() == before {
                return Err(OrchestratorError::Invalid("unresolvable sub-task dependencies".into()));
            }
            remaining = next;
        }
        self.publish().await;
        Ok(())
    }

    async fn create_review(&mut self, member: &TeamMember, role: AgentRole) -> OrchResult<usize> {
        let deps: Vec<TaskId> = self.lead.subtasks.iter().map(|s| s.task_id).collect();
        let (title, ac) = if self.ru() {
            (format!("Ревью: {}", self.lead.title), vec!["Изменения исполнителей проверены, замечания перечислены".to_owned()])
        } else {
            (format!("Review: {}", self.lead.title), vec!["The executors' changes are reviewed and findings listed".to_owned()])
        };
        let mut description = String::from("Review the combined work of the team's executors:\n");
        for s in &self.lead.subtasks {
            description.push_str(&format!("- {}\n", s.title));
        }
        let task = self
            .inner
            .deps
            .tasks
            .create(
                CreateTaskParams {
                    project_id: self.project.id,
                    title: cap(&title, MAX_TITLE_LEN),
                    description: Some(cap(&description, MAX_DESCRIPTION_LEN)),
                    acceptance_criteria: Some(ac),
                    parent_id: Some(self.lead.parent_task_id),
                    depends_on: Some(deps),
                    tags: Some(vec!["team".into(), "review".into()]),
                    ..CreateTaskParams::default()
                },
                TaskActor::Manager,
            )
            .await?;
        self.live.insert("review".into(), member.clone());
        self.lead.subtasks.push(TeamLeadSubtask {
            task_id: task.id,
            key: "review".into(),
            title: task.title,
            role,
            template_id: member.template_id,
            status: task.status,
            agent_id: None,
            attempts: 0,
            is_review: true,
            note: None,
        });
        self.publish().await;
        Ok(self.lead.subtasks.len() - 1)
    }

    fn agent_live(&self, i: usize) -> bool {
        self.lead.subtasks[i].agent_id.is_some_and(|a| self.inner.deps.agents.is_live(a))
    }

    fn stop_request(&self) -> Option<StopKind> {
        self.d.control.as_ref().and_then(|c| c.stop_kind())
    }

    fn paused(&self) -> bool {
        self.d.control.as_ref().is_some_and(|c| *c.paused.borrow())
    }

    /// Drives the given sub-tasks to `Completed`. `None` = all completed.
    async fn run_until_done(&mut self, idx: &[usize], max_attempts: u32) -> Option<End> {
        let limit = std::time::Duration::from_secs(u64::from(self.req.team.policies.time_limit_mins) * 60);
        let tick = self.inner.deps.config.tick;
        loop {
            if self.d.cancel.is_cancelled() {
                return Some(End::Cancelled("emergency stop".into()));
            }
            match self.stop_request() {
                Some(StopKind::Cancel) => return Some(End::Cancelled("the Manager run was cancelled".into())),
                Some(StopKind::Shutdown) => return Some(End::Interrupted("Orkestr is shutting down".into())),
                None => {}
            }
            if self.started.elapsed() >= limit {
                return Some(End::Failed(format!(
                    "the team's time limit of {} min was reached",
                    self.req.team.policies.time_limit_mins
                )));
            }
            self.refresh().await;
            if let Some(b) = self.lead.budget_usd {
                if self.lead.cost_usd >= b && (b > 0.0 || self.lead.cost_usd > 0.0) {
                    return Some(End::Failed(format!(
                        "budget exhausted: ${:.4} spent of ${b:.2}",
                        self.lead.cost_usd
                    )));
                }
            }
            if let Some(end) = self.handle_finished(idx, max_attempts).await {
                return Some(end);
            }
            if idx.iter().all(|i| self.lead.subtasks[*i].status == TaskStatus::Completed) {
                self.publish().await;
                return None;
            }
            if !self.paused() {
                if let Some(end) = self.spawn_ready(idx, max_attempts).await {
                    return Some(end);
                }
            }
            self.publish().await;
            let stop = self.d.control.as_ref().map(|c| c.stop_token.clone()).unwrap_or_default();
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = self.d.cancel.cancelled() => {}
                () = stop.cancelled() => {}
            }
        }
    }

    async fn refresh(&mut self) {
        let mut children = 0.0;
        for s in &mut self.lead.subtasks {
            if let Ok(t) = self.inner.deps.tasks.get(s.task_id).await {
                s.status = t.status;
                s.title = t.title;
                children += t.usage.cost_usd;
            }
        }
        // Roll the executors' cost up into the delegated task so the run's
        // budget (sum of its tasks) sees it.
        let delta = children - self.rolled_up;
        if delta > 0.0 {
            self.rolled_up = children;
            let _ = self
                .inner
                .deps
                .tasks
                .record_agent_run(
                    self.lead.parent_task_id,
                    AgentRunUpdate {
                        usage_delta: Some(UsageSummary { input_tokens: 0, output_tokens: 0, cost_usd: delta }),
                        ..AgentRunUpdate::default()
                    },
                )
                .await;
        }
        self.lead.cost_usd = ((self.model_cost + children) * 1_000_000.0).round() / 1_000_000.0;
    }

    async fn handle_finished(&mut self, idx: &[usize], max_attempts: u32) -> Option<End> {
        for &i in idx {
            if self.agent_live(i) {
                continue;
            }
            let s = self.lead.subtasks[i].clone();
            let Ok(task) = self.inner.deps.tasks.get(s.task_id).await else {
                return Some(End::Failed(format!("sub-task \"{}\" was deleted", s.title)));
            };
            match task.status {
                TaskStatus::Review | TaskStatus::Testing if s.agent_id.is_some() => {
                    let ok = task.result.as_ref().is_some_and(|r| r.success);
                    let (to, why) = if ok {
                        (TaskStatus::Completed, "accepted by the team lead")
                    } else {
                        (TaskStatus::Failed, "the executor reported a partial result")
                    };
                    match self.inner.deps.tasks.transition(s.task_id, to, TaskActor::Manager, Some(why.into()), None).await
                    {
                        Ok(c) => self.lead.subtasks[i].status = c.task.status,
                        Err(e) => tracing::warn!(error = %e, "lead could not decide a sub-task"),
                    }
                }
                TaskStatus::Failed | TaskStatus::Interrupted if s.agent_id.is_some() => {
                    let why = task.result.as_ref().map(|r| one_line(&r.summary, 300));
                    self.lead.subtasks[i].note = why.clone();
                    if s.attempts < max_attempts && !s.is_review {
                        match self.inner.deps.tasks.retry(s.task_id, TaskActor::Manager).await {
                            Ok(c) => {
                                self.lead.subtasks[i].status = c.task.status;
                                self.lead.subtasks[i].agent_id = None;
                            }
                            Err(e) => return Some(End::Failed(format!("could not retry \"{}\": {e}", s.title))),
                        }
                    } else {
                        return Some(End::Failed(format!(
                            "sub-task \"{}\" failed after {} attempt(s){}",
                            s.title,
                            s.attempts,
                            why.map(|w| format!(": {w}")).unwrap_or_default()
                        )));
                    }
                }
                TaskStatus::Cancelled | TaskStatus::Blocked => {
                    return Some(End::Failed(format!("sub-task \"{}\" is {:?}", s.title, task.status)));
                }
                _ => {}
            }
        }
        None
    }

    async fn ensure_git(&mut self) -> bool {
        if let Some(g) = self.git {
            return g;
        }
        let root = self.project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir());
        let ok = match root {
            Some(root) => {
                let git = self.inner.deps.vcs.git();
                git.is_available() && matches!(git.rev_parse(&root, "HEAD").await, Ok(Some(_)))
            }
            None => false,
        };
        self.git = Some(ok);
        ok
    }

    /// Branch of the most recent prerequisite (dedicated worktrees).
    async fn base_branch(&self, task: TaskId) -> Option<String> {
        let t = self.inner.deps.tasks.get(task).await.ok()?;
        let mut branch = None;
        for d in &t.dependencies {
            if let Ok(dt) = self.inner.deps.tasks.get(*d).await {
                if let Some(w) = dt.worktree.filter(|w| w.branch.starts_with("agent/")) {
                    branch = Some(w.branch);
                }
            }
        }
        branch
    }

    async fn spawn_ready(&mut self, idx: &[usize], max_attempts: u32) -> Option<End> {
        let max_parallel = self.req.team.policies.max_parallel.max(1) as usize;
        let mut live = (0..self.lead.subtasks.len()).filter(|i| self.agent_live(*i)).count();
        for &i in idx {
            if live >= max_parallel {
                break;
            }
            let s = self.lead.subtasks[i].clone();
            if self.agent_live(i) || s.attempts >= max_attempts {
                continue;
            }
            let Ok(task) = self.inner.deps.tasks.get(s.task_id).await else { continue };
            let startable = match task.status {
                TaskStatus::Ready => true,
                TaskStatus::Planned => {
                    let mut all = true;
                    for d in &task.dependencies {
                        all &= self.inner.deps.tasks.get(*d).await.is_ok_and(|t| t.status == TaskStatus::Completed);
                    }
                    all
                }
                _ => false,
            };
            if !startable {
                continue;
            }
            let Some(member) = self.live.get(&s.key).cloned() else { continue };
            let Ok(template) = self.inner.deps.agents.templates().get(member.template_id).await else {
                return Some(End::Failed(format!("the template of \"{}\" is missing", s.title)));
            };
            let git = self.ensure_git().await;
            let base = if git { self.base_branch(s.task_id).await } else { None };
            let policy = if git {
                if template.worktree_policy == WorktreePolicy::ReadOnlyMain && base.is_none() {
                    WorktreePolicy::ReadOnlyMain
                } else {
                    WorktreePolicy::DedicatedWorktree
                }
            } else if template.worktree_policy == WorktreePolicy::ReadOnlyMain {
                WorktreePolicy::ReadOnlyMain
            } else {
                WorktreePolicy::SharedWorktree
            };
            // The lead's budget is split between its sub-tasks.
            let budget = self.lead.budget_usd.map(|b| {
                let open = self.lead.subtasks.iter().filter(|x| x.status != TaskStatus::Completed).count().max(1);
                let left = (b - self.lead.cost_usd).max(0.0);
                Budget { max_cost_usd: Some(left / open as f64), ..Budget::unrestricted(BudgetPeriod::PerTask) }
            });
            let mode = match self.req.team.policies.approval_mode {
                Some(cap_mode) => stricter_mode(self.d.approval_mode, cap_mode),
                None => self.d.approval_mode,
            };
            let params = SpawnAgentParams {
                project_id: self.project.id,
                template_id: member.template_id,
                task_id: Some(s.task_id),
                new_task: None,
                model: member.model.clone().or_else(|| self.d.model.clone()),
                worktree_policy: Some(policy),
                name: None,
                max_turns: None,
                base_branch: base,
                approval_mode: Some(mode),
                budget,
            };
            let extras = SpawnExtras {
                team_id: Some(self.req.team.id),
                parent_agent_id: Some(self.agent.id),
                rules: scope_rules(&self.req.assignment.permissions),
                rule_note: Some(format!("team scope: {}", self.req.team.name)),
            };
            match self.inner.deps.agents.spawn_with(params, extras).await {
                Ok((agent, _)) => {
                    live += 1;
                    let st = &mut self.lead.subtasks[i];
                    st.agent_id = Some(agent.id);
                    st.attempts += 1;
                    st.note = None;
                    st.status = TaskStatus::Running;
                    tracing::info!(lead = %self.lead.id, agent = %agent.id, task = %s.task_id, "team executor spawned");
                }
                Err(AgentError::Conflict(m)) => self.lead.subtasks[i].note = Some(one_line(&m, 300)),
                Err(e @ (AgentError::NoModel(_) | AgentError::Forbidden(_))) => {
                    return Some(End::Failed(format!("could not start an executor for \"{}\": {e}", s.title)));
                }
                Err(e) => {
                    let st = &mut self.lead.subtasks[i];
                    st.attempts += 1;
                    st.note = Some(format!("could not start an executor: {}", one_line(&e.to_string(), 300)));
                    if st.attempts >= max_attempts {
                        return Some(End::Failed(format!("could not start an executor for \"{}\": {e}", s.title)));
                    }
                }
            }
        }
        None
    }

    async fn result_summary(&self) -> String {
        let mut s = String::new();
        if let Some(sum) = &self.lead.summary {
            s.push_str(&one_line(sum, 600));
            s.push('\n');
        }
        for st in &self.lead.subtasks {
            let res = self.inner.deps.tasks.get(st.task_id).await.ok().and_then(|t| t.result).map(|r| r.summary);
            s.push_str(&format!(
                "- {} ({}): {}\n",
                st.title,
                role_name(&st.role),
                res.map(|r| one_line(&r, 240)).unwrap_or_else(|| format!("{:?}", st.status))
            ));
        }
        cap(&s, 1900)
    }

    async fn stop_executors(&self, mode: AgentStopMode) {
        for i in 0..self.lead.subtasks.len() {
            if let Some(a) = self.lead.subtasks[i].agent_id.filter(|_| self.agent_live(i)) {
                if let Err(e) = self.inner.deps.agents.stop(a, mode).await {
                    tracing::warn!(agent = %a, error = %e, "could not stop an executor");
                }
            }
        }
    }

    async fn cancel_open_subtasks(&self, why: &str) {
        for s in &self.lead.subtasks {
            let Ok(t) = self.inner.deps.tasks.get(s.task_id).await else { continue };
            if matches!(t.status, TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Running | TaskStatus::Waiting)
            {
                continue;
            }
            let _ = self
                .inner
                .deps
                .tasks
                .transition(s.task_id, TaskStatus::Cancelled, TaskActor::Manager, Some(why.into()), None)
                .await;
        }
    }

    async fn finish(mut self, end: End) {
        let parent = self.lead.parent_task_id;
        let tasks = Arc::clone(&self.inner.deps.tasks);
        let (status, agent_state, target, result, reason) = match end {
            End::Done(summary) => (
                TeamLeadStatus::Completed,
                AgentState::Completed,
                TaskStatus::Review,
                Some(TaskResult { success: true, summary: summary.clone() }),
                None,
            ),
            End::Failed(r) => {
                self.stop_executors(AgentStopMode::Interrupt).await;
                self.cancel_open_subtasks(&r).await;
                (
                    TeamLeadStatus::Failed,
                    AgentState::Error,
                    TaskStatus::Failed,
                    Some(TaskResult { success: false, summary: cap(&format!("team lead: {r}"), 1900) }),
                    Some(r),
                )
            }
            End::Cancelled(r) => {
                self.stop_executors(AgentStopMode::Cancel).await;
                self.cancel_open_subtasks(&r).await;
                (TeamLeadStatus::Cancelled, AgentState::Stopped, TaskStatus::Cancelled, None, Some(r))
            }
            End::Interrupted(r) => {
                self.stop_executors(AgentStopMode::Interrupt).await;
                (TeamLeadStatus::Failed, AgentState::Stopped, TaskStatus::Interrupted, None, Some(r))
            }
        };
        self.refresh().await;
        if let Some(res) = result {
            if let Err(e) =
                tasks.record_agent_run(parent, AgentRunUpdate { result: Some(res), ..AgentRunUpdate::default() }).await
            {
                tracing::warn!(error = %e, "could not record the lead's result");
            }
        }
        if let Ok(t) = tasks.get(parent).await {
            if !t.status.is_terminal() && t.status != target {
                let why = reason.clone().unwrap_or_else(|| "the team lead finished".into());
                if let Err(e) =
                    tasks.transition(parent, target, TaskActor::Manager, Some(cap(&why, 1900)), None).await
                {
                    tracing::warn!(task = %parent, error = %e, "could not report the lead's outcome");
                }
            }
        }
        self.lead.status = status;
        self.lead.status_reason = reason.clone();
        self.lead.finished_at = Some(now());
        if agent_state == AgentState::Error {
            self.agent.last_error = reason.clone();
        }
        let activity = match status {
            TeamLeadStatus::Completed => "Delegated task done",
            TeamLeadStatus::Cancelled => "Cancelled",
            _ => "Stopped",
        };
        self.set_state(agent_state, activity).await;
        self.publish().await;
        let text = match (status, self.ru()) {
            (TeamLeadStatus::Completed, false) => {
                format!("The {} lead finished \"{}\" — it waits for acceptance.", self.req.team.name, self.lead.title)
            }
            (TeamLeadStatus::Completed, true) => {
                format!("Лид команды «{}» завершил «{}» — задача ждёт приёмки.", self.req.team.name, self.lead.title)
            }
            (_, false) => format!(
                "The {} lead stopped \"{}\": {}",
                self.req.team.name,
                self.lead.title,
                reason.unwrap_or_default()
            ),
            (_, true) => format!(
                "Лид команды «{}» остановил «{}»: {}",
                self.req.team.name,
                self.lead.title,
                reason.unwrap_or_default()
            ),
        };
        self.inner
            .system_event(self.project.id, &text, MessageExtras { run_id: self.d.run_id, ..MessageExtras::default() })
            .await;
        if let Some(c) = &self.d.control {
            c.wake.notify_one();
        }
        tracing::info!(lead = %self.lead.id, ?status, "team lead finished");
    }
}

/// Keys of `subs` in a stable order (helper for tests and the UI tree).
#[must_use]
pub fn keys(subs: &[SubSpec]) -> BTreeSet<String> {
    subs.iter().map(|s| s.key.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::AgentTemplateId;

    fn m(lead: bool) -> TeamMember {
        TeamMember { template_id: AgentTemplateId::new(), is_lead: lead, count: 1, model: None }
    }

    fn team() -> Vec<(TeamMember, AgentRole)> {
        vec![(m(true), AgentRole::Architect), (m(false), AgentRole::Backend), (m(false), AgentRole::Qa), (m(false), AgentRole::Reviewer)]
    }

    #[test]
    fn normalizes_fan_out_roles_and_dependencies() {
        let doc = json!({"summary": "s", "subtasks": [
            {"key": "a", "title": "A", "role": "Backend", "dependsOn": []},
            {"key": "b", "title": "B", "role": "tester", "dependsOn": ["a", "zzz", "b"]},
            {"key": "c", "title": "C", "role": "Reviewer", "dependsOn": ["b"]},
            {"key": "d", "title": "D", "role": "Frontend", "dependsOn": []}
        ]});
        let (summary, subs, warnings) = normalize_subtasks(&doc, &team(), 3).unwrap();
        assert_eq!(summary, "s");
        assert_eq!(subs.len(), 3, "fan-out limit");
        assert!(warnings.iter().any(|w| w.contains("fan-out")));
        assert_eq!(subs[1].role, AgentRole::Qa);
        assert_eq!(subs[1].depends_on, vec!["a".to_owned()]);
        assert_eq!(subs[2].role, AgentRole::Qa, "reviewer work goes to an executor");
        assert!(subs.iter().all(|s| !s.member.is_lead));
        assert_eq!(keys(&subs).len(), 3);
    }

    #[test]
    fn rejects_cycles_and_empty() {
        let cyc = json!({"subtasks": [
            {"key": "a", "title": "A", "role": "Backend", "dependsOn": ["b"]},
            {"key": "b", "title": "B", "role": "Backend", "dependsOn": ["a"]}
        ]});
        assert!(normalize_subtasks(&cyc, &team(), 5).is_err());
        assert!(normalize_subtasks(&json!({"subtasks": []}), &team(), 5).is_err());
        assert!(normalize_subtasks(&json!({"x": 1}), &team(), 5).is_err());
        let only_lead = vec![(m(true), AgentRole::Architect)];
        let one = json!({"subtasks": [{"key": "a", "title": "A", "role": "Backend", "dependsOn": []}]});
        assert!(normalize_subtasks(&one, &only_lead, 5).is_err());
    }

    #[test]
    fn cycle_detection() {
        let mut g = BTreeMap::new();
        g.insert("a".to_owned(), vec![]);
        g.insert("b".to_owned(), vec!["a".to_owned()]);
        assert!(!has_cycle(&g));
        g.insert("a".to_owned(), vec!["b".to_owned()]);
        assert!(has_cycle(&g));
    }
}
