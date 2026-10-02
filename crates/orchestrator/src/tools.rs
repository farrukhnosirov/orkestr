//! Tools of the AI Manager's model (§5: "Manager sees the real state").
//!
//! * **State tools** (read-only): `list_projects`, `project_overview`,
//!   `list_tasks`, `task_graph`, `list_agents`, `agent_status`,
//!   `provider_status`, `pending_approvals`, `cost_summary`, `git_status`,
//!   `memory_search`, `list_snapshots`. Every call is checked by the
//!   permission engine as `Actor::Manager` with capability `Read`.
//! * **Action tools**: `propose_plan`, `create_tasks`, `spawn_agent`,
//!   `stop_agent`, `request_approval`, `record_decision`, `answer_user`.
//!   With the `Ask` approval policy, `create_tasks` and `spawn_agent` wait
//!   for the user's approval first; emergency stop refuses them.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_domain::{
    AgentId, ApprovalMode, ApprovalStatus, Capability, ConversationId, ManagerPlanId, Project, RiskLevel, TaskActor,
    TaskId, TaskPriority, TaskStatus,
};
use orkestr_protocol::methods::agents::{AgentStopMode, ListAgentsParams, SpawnAgentParams};
use orkestr_protocol::methods::manager::{ManagerRequestOptions, MAX_PLAN_TASKS};
use orkestr_protocol::methods::memory::{CreateDecisionParams, SearchMemoryParams};
use orkestr_protocol::methods::tasks::{CreateTaskParams, ListTasksParams};
use orkestr_providers::ToolSpec;
use orkestr_security::{ApprovalFilter, ApprovalRequest, CheckContext, Decision, PermissionEngine, Resource};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::plan::parse_role;
use crate::service::Inner;
use crate::util::{cap, one_line, role_name};

/// Names of the read-only state tools.
pub const STATE_TOOLS: [&str; 12] = [
    "list_projects",
    "project_overview",
    "list_tasks",
    "task_graph",
    "list_agents",
    "agent_status",
    "provider_status",
    "pending_approvals",
    "cost_summary",
    "git_status",
    "memory_search",
    "list_snapshots",
];

/// Names of the action tools.
pub const ACTION_TOOLS: [&str; 7] =
    ["propose_plan", "create_tasks", "spawn_agent", "stop_agent", "request_approval", "record_decision", "answer_user"];

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Tool specs offered to the Manager model.
#[must_use]
pub fn tool_specs() -> Vec<ToolSpec> {
    let t = |name: &str, description: &str, parameters: Value| ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
    };
    let statuses: Vec<&str> = vec![
        "Planned",
        "Ready",
        "Running",
        "Waiting",
        "Blocked",
        "Review",
        "Testing",
        "Completed",
        "Failed",
        "Cancelled",
        "Interrupted",
        "Recovering",
    ];
    vec![
        t("list_projects", "List the user's projects (id, name, root folder, approval mode).", obj(json!({}), &[])),
        t(
            "project_overview",
            "Overview of the current project: task counts by status, running agents, the active Manager run, pending approvals, last snapshot, spend.",
            obj(json!({}), &[]),
        ),
        t(
            "list_tasks",
            "List tasks of the current project with status, priority, agent and dependencies.",
            obj(
                json!({
                    "status": {"type": "array", "items": {"type": "string", "enum": statuses}},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200}
                }),
                &[],
            ),
        ),
        t("task_graph", "The task dependency graph of the current project: edges, ready set, critical path.", obj(json!({}), &[])),
        t(
            "list_agents",
            "List agents of the current project (state, task, model, branch, usage).",
            obj(json!({ "active_only": {"type": "boolean"} }), &[]),
        ),
        t(
            "agent_status",
            "Details of one agent and the end of its transcript.",
            obj(json!({ "agent_id": {"type": "string"} }), &["agent_id"]),
        ),
        t("provider_status", "Configured AI providers with health, auth and concurrency.", obj(json!({}), &[])),
        t("pending_approvals", "Approvals waiting for the user in this project.", obj(json!({}), &[])),
        t("cost_summary", "Spend of this project: today, this month, total.", obj(json!({}), &[])),
        t("git_status", "Git branch and changed files of the project's main checkout.", obj(json!({}), &[])),
        t(
            "memory_search",
            "Full-text search over project and global memory (rules, decisions, conventions, known issues).",
            obj(
                json!({ "query": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 20} }),
                &["query"],
            ),
        ),
        t(
            "list_snapshots",
            "Recent workspace snapshots (checkpoints) of the project.",
            obj(json!({ "limit": {"type": "integer", "minimum": 1, "maximum": 50} }), &[]),
        ),
        t(
            "propose_plan",
            "Study the project and propose a structured plan (epics, tasks with acceptance criteria, roles, dependencies, cost, parallelism) for a goal. The user sees it as a plan card and approves it. Use this for any request to build, change or fix something.",
            obj(
                json!({
                    "goal": {"type": "string", "description": "The goal, in the user's words plus any clarifications"}
                }),
                &["goal"],
            ),
        ),
        t(
            "create_tasks",
            "Create tasks directly (outside a plan) in the current project. `depends_on` lists indexes of earlier tasks in this call or existing task ids.",
            obj(
                json!({
                    "tasks": {"type": "array", "minItems": 1, "maxItems": 20, "items": {"type": "object", "properties": {
                        "title": {"type": "string"},
                        "description": {"type": "string"},
                        "acceptance_criteria": {"type": "array", "items": {"type": "string"}},
                        "priority": {"type": "string", "enum": ["Critical", "High", "Normal", "Low", "Background"]},
                        "depends_on": {"type": "array", "items": {"type": "string"}}
                    }, "required": ["title"]}}
                }),
                &["tasks"],
            ),
        ),
        t(
            "spawn_agent",
            "Start an isolated agent (own worktree) on an existing Planned/Ready task.",
            obj(
                json!({
                    "task_id": {"type": "string"},
                    "role": {"type": "string", "description": "Agent role, e.g. Backend, QA, Reviewer"}
                }),
                &["task_id", "role"],
            ),
        ),
        t(
            "stop_agent",
            "Stop a running agent (its task becomes Interrupted).",
            obj(json!({ "agent_id": {"type": "string"}, "reason": {"type": "string"} }), &["agent_id"]),
        ),
        t(
            "request_approval",
            "Ask the user to approve or reject a decision and wait for the answer.",
            obj(
                json!({
                    "summary": {"type": "string"},
                    "reason": {"type": "string"},
                    "risk": {"type": "string", "enum": ["Low", "Medium", "High", "Critical"]}
                }),
                &["summary"],
            ),
        ),
        t(
            "record_decision",
            "Record an architecture/product decision in the project decision log (rationale_summary is a short user-facing explanation).",
            obj(
                json!({
                    "title": {"type": "string"},
                    "decision": {"type": "string"},
                    "context": {"type": "string"},
                    "rationale_summary": {"type": "string"},
                    "alternatives": {"type": "array", "items": {"type": "string"}}
                }),
                &["title", "decision"],
            ),
        ),
        t(
            "answer_user",
            "Give the final answer to the user (markdown) and end your turn.",
            obj(json!({ "text": {"type": "string"} }), &["text"]),
        ),
    ]
}

/// Context of one Manager turn.
pub struct TurnCtx {
    /// Project.
    pub project: Project,
    /// Conversation.
    pub conversation: ConversationId,
    /// Request overrides.
    pub options: ManagerRequestOptions,
    /// Effective approval policy.
    pub policy: ApprovalMode,
    /// Turn cancellation.
    pub cancel: CancellationToken,
}

/// Result of one tool call.
#[derive(Debug, Default)]
pub struct ToolOutcome {
    /// Content for the model.
    pub content: String,
    /// One-line summary for the transcript.
    pub summary: String,
    /// Failed / denied.
    pub is_error: bool,
    /// Plan proposed.
    pub plan_id: Option<ManagerPlanId>,
    /// Final answer (`answer_user`).
    pub final_answer: Option<String>,
    /// Usage spent inside the tool (planning).
    pub usage: Option<orkestr_domain::UsageSummary>,
}

impl ToolOutcome {
    fn ok(content: impl Into<String>, summary: impl Into<String>) -> Self {
        Self { content: content.into(), summary: summary.into(), ..Self::default() }
    }

    fn err(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self { content: format!("error: {m}"), summary: m, is_error: true, ..Self::default() }
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn to_json(v: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
}

impl Inner {
    /// Permission check of a state read (`Read` as the Manager).
    fn check_read(&self, ctx: &TurnCtx, resource: Resource, root: Option<PathBuf>) -> Result<(), String> {
        let check = CheckContext {
            project_id: Some(ctx.project.id),
            agent_id: None,
            task_id: None,
            approval_mode: ctx.project.approval_mode,
            worktree_root: Some(root.unwrap_or_else(|| self.deps.config.state_root.clone())),
            protected_environment: false,
        };
        match self.deps.security.engine.check(&orkestr_domain::Actor::Manager, Capability::Read, &resource, &check) {
            Decision::Allow => Ok(()),
            Decision::Ask(a) => Err(format!("reading this needs the user's approval ({})", a.message)),
            Decision::Deny(d) => Err(format!("permission denied: {}", d.message)),
        }
    }

    /// Approval gate of action tools under the `Ask` policy.
    async fn gate_action(&self, ctx: &TurnCtx, summary: &str, preview: Option<String>) -> Result<(), String> {
        if self.deps.security.emergency.flags().all_stopped {
            return Err("Stop All AI is active".into());
        }
        if ctx.policy != ApprovalMode::Ask {
            return Ok(());
        }
        let mut req = ApprovalRequest::new(
            Capability::Write,
            summary,
            "the AI Manager's approval policy is Ask",
            RiskLevel::Medium,
        );
        req.project_id = Some(ctx.project.id);
        req.command_preview = preview;
        match self.deps.security.approvals.request_and_wait(req, self.deps.config.approval_timeout, &ctx.cancel).await {
            Ok((_, ApprovalStatus::Approved)) => Ok(()),
            Ok((_, ApprovalStatus::Rejected)) => Err("the user rejected this action".into()),
            Ok((_, s)) => Err(format!("no decision ({s:?})")),
            Err(e) => Err(format!("approval failed: {e}")),
        }
    }

    /// Executes one Manager tool call.
    #[allow(clippy::too_many_lines)]
    pub async fn execute_tool(self: &Arc<Self>, ctx: &TurnCtx, name: &str, args: &Value) -> ToolOutcome {
        let pid = ctx.project.id;
        if STATE_TOOLS.contains(&name) {
            let (resource, root) = match name {
                "git_status" => {
                    let Some(root) = ctx.project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir()) else {
                        return ToolOutcome::err("the project has no root folder on this machine");
                    };
                    (Resource::Path { path: ".".into(), recursive: false }, Some(root))
                }
                other => (Resource::Other { value: format!("orkestr-state:{other}") }, None),
            };
            if let Err(e) = self.check_read(ctx, resource, root) {
                return ToolOutcome::err(e);
            }
        }
        let limit = |k: &str, d: u64, m: u64| args.get(k).and_then(Value::as_u64).unwrap_or(d).clamp(1, m);
        match name {
            "list_projects" => match self.deps.projects.list(false).await {
                Ok(ps) => {
                    let rows: Vec<Value> = ps
                        .iter()
                        .filter(|p| !p.archived)
                        .map(|p| json!({"id": p.id, "name": p.name, "root": p.root_path, "approvalMode": p.approval_mode}))
                        .collect();
                    ToolOutcome::ok(to_json(&rows), format!("{} project(s)", rows.len()))
                }
                Err(e) => ToolOutcome::err(e.to_string()),
            },
            "project_overview" => self.overview(&ctx.project).await,
            "list_tasks" => {
                let status: Option<Vec<TaskStatus>> = args
                    .get("status")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .filter(|v: &Vec<TaskStatus>| !v.is_empty());
                match self
                    .deps
                    .tasks
                    .list(ListTasksParams { project_id: Some(pid), status, ..ListTasksParams::default() })
                    .await
                {
                    Ok(tasks) => {
                        let n = limit("limit", 100, 200) as usize;
                        let rows: Vec<Value> = tasks
                            .iter()
                            .rev()
                            .take(n)
                            .map(|t| {
                                json!({"id": t.id, "title": t.title, "status": t.status, "priority": t.priority,
                                       "agentId": t.agent_id, "dependsOn": t.dependencies, "tags": t.tags,
                                       "result": t.result.as_ref().map(|r| one_line(&r.summary, 200))})
                            })
                            .collect();
                        ToolOutcome::ok(to_json(&rows), format!("{} task(s)", rows.len()))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "task_graph" => match self.deps.tasks.graph(pid, None).await {
                Ok(g) => {
                    let title = |id: &TaskId| {
                        g.nodes.iter().find(|n| n.id == *id).map_or_else(|| id.to_string(), |n| n.title.clone())
                    };
                    let v = json!({
                        "nodes": g.nodes.iter().map(|n| json!({"id": n.id, "title": n.title, "status": n.status})).collect::<Vec<_>>(),
                        "edges": g.edges.iter().map(|e| format!("{} -> {}", title(&e.depends_on), title(&e.task_id))).collect::<Vec<_>>(),
                        "ready": g.ready.iter().map(title).collect::<Vec<_>>(),
                        "criticalPath": g.critical_path.iter().map(title).collect::<Vec<_>>(),
                    });
                    ToolOutcome::ok(to_json(&v), format!("{} node(s), {} edge(s)", g.nodes.len(), g.edges.len()))
                }
                Err(e) => ToolOutcome::err(e.to_string()),
            },
            "list_agents" => {
                let active = args.get("active_only").and_then(Value::as_bool);
                match self
                    .deps
                    .agents
                    .list(ListAgentsParams { project_id: Some(pid), task_id: None, active_only: active })
                    .await
                {
                    Ok(agents) => {
                        let rows: Vec<Value> = agents
                            .iter()
                            .take(50)
                            .map(|a| {
                                json!({"id": a.id, "name": a.name, "role": role_name(&a.role), "state": a.state,
                                       "taskId": a.task_id, "model": a.model.as_ref().map(|m| m.model.clone()),
                                       "branch": a.worktree.as_ref().map(|w| w.branch.clone()),
                                       "costUsd": a.usage.cost_usd, "activity": a.activity})
                            })
                            .collect();
                        ToolOutcome::ok(to_json(&rows), format!("{} agent(s)", rows.len()))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "agent_status" => {
                let Some(id) = arg_str(args, "agent_id").and_then(|s| AgentId::parse(s).ok()) else {
                    return ToolOutcome::err("agent_id must be an agent id");
                };
                match self.deps.agents.transcript(id, Some(12)).await {
                    Ok(t) if t.agent.project_id == pid => {
                        let v = json!({
                            "agent": {"id": t.agent.id, "name": t.agent.name, "state": t.agent.state, "turnsUsed": t.agent.turns_used,
                                       "maxTurns": t.agent.max_turns, "lastError": t.agent.last_error, "activity": t.agent.activity},
                            "recentMessages": t.messages.iter().rev().take(6).map(|m| one_line(&m.text, 300)).collect::<Vec<_>>(),
                            "recentTools": t.tool_calls.iter().rev().take(8).map(|c| format!("{} → {:?}", c.tool_call.tool, c.tool_call.status)).collect::<Vec<_>>(),
                        });
                        ToolOutcome::ok(to_json(&v), format!("{} is {:?}", t.agent.name, t.agent.state))
                    }
                    Ok(_) => ToolOutcome::err("the agent belongs to another project"),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "provider_status" => {
                let ps = self.deps.hub.providers();
                ToolOutcome::ok(to_json(&ps), format!("{} provider(s)", ps.len()))
            }
            "pending_approvals" => {
                match self
                    .deps
                    .security
                    .approvals
                    .list(&ApprovalFilter {
                        status: Some(ApprovalStatus::Pending),
                        project_id: Some(pid),
                        limit: Some(50),
                    })
                    .await
                {
                    Ok(list) => {
                        let rows: Vec<Value> = list
                            .iter()
                            .map(|a| json!({"id": a.id, "summary": a.action_summary, "risk": a.risk, "agentId": a.agent_id, "requestedAt": a.requested_at}))
                            .collect();
                        ToolOutcome::ok(to_json(&rows), format!("{} pending approval(s)", rows.len()))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "cost_summary" => match self.deps.cost.summary(Some(pid)).await {
                Ok(s) => ToolOutcome::ok(
                    to_json(
                        &json!({"todayUsd": s.today.cost_usd, "monthUsd": s.month.cost_usd, "totalUsd": s.total.cost_usd,
                                    "requests": s.total.requests, "budgetUsed": s.budget_used}),
                    ),
                    format!("${:.4} total", s.total.cost_usd),
                ),
                Err(e) => ToolOutcome::err(e.to_string()),
            },
            "git_status" => {
                let root = ctx.project.root_path.as_ref().map(PathBuf::from).unwrap_or_default();
                match self.deps.vcs.git().status(&root).await {
                    Ok(st) if st.is_repo => {
                        let files: Vec<String> = st.entries.iter().take(100).map(|e| e.path.clone()).collect();
                        let v = json!({"branch": st.branch, "head": st.head, "ahead": st.ahead, "changedFiles": files});
                        ToolOutcome::ok(to_json(&v), format!("{} changed file(s)", st.entries.len()))
                    }
                    Ok(_) => ToolOutcome::err("the project folder is not a git repository"),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "memory_search" => {
                let Some(q) = arg_str(args, "query") else { return ToolOutcome::err("query is required") };
                match self
                    .deps
                    .memory
                    .search(SearchMemoryParams {
                        query: cap(q, 300),
                        project_id: Some(pid),
                        include_global: Some(true),
                        kinds: None,
                        limit: Some(u32::try_from(limit("limit", 8, 20)).unwrap_or(8)),
                    })
                    .await
                {
                    Ok(hits) => {
                        let rows: Vec<Value> = hits
                            .iter()
                            .map(|h| json!({"title": h.item.title, "kind": h.item.kind, "content": cap(&h.item.content, 600)}))
                            .collect();
                        ToolOutcome::ok(to_json(&rows), format!("{} hit(s)", rows.len()))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "list_snapshots" => {
                match self
                    .deps
                    .snapshots
                    .list(pid, None, None, u32::try_from(limit("limit", 10, 50)).unwrap_or(10))
                    .await
                {
                    Ok(list) => {
                        let rows: Vec<Value> = list
                            .iter()
                            .map(|s| json!({"id": s.id, "label": s.label, "reason": s.reason, "createdAt": s.created_at, "files": s.file_count}))
                            .collect();
                        ToolOutcome::ok(to_json(&rows), format!("{} snapshot(s)", rows.len()))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "propose_plan" => {
                let Some(goal) = arg_str(args, "goal") else { return ToolOutcome::err("goal is required") };
                match self
                    .plan_and_maybe_run(&ctx.project, goal, ctx.options.clone(), Some(ctx.conversation), &ctx.cancel)
                    .await
                {
                    Ok((plan, usage)) => {
                        let tasks: Vec<String> = plan
                            .tasks
                            .iter()
                            .map(|t| {
                                format!(
                                    "{} [{}] {}{}",
                                    t.key,
                                    role_name(&t.role),
                                    t.title,
                                    if t.depends_on.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" (after {})", t.depends_on.join(", "))
                                    }
                                )
                            })
                            .collect();
                        let content = format!(
                            "Plan {} ({:?}) with {} task(s), up to {} in parallel{}:\n{}\nSummary: {}",
                            plan.id,
                            plan.status,
                            plan.tasks.len(),
                            plan.parallelism,
                            plan.estimated_cost_usd.map(|c| format!(", estimated ${c:.2}")).unwrap_or_default(),
                            tasks.join("\n"),
                            plan.summary
                        );
                        ToolOutcome {
                            summary: format!("plan with {} task(s)", plan.tasks.len()),
                            content,
                            plan_id: Some(plan.id),
                            usage: Some(usage),
                            ..ToolOutcome::default()
                        }
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "create_tasks" => self.create_tasks_tool(ctx, args).await,
            "spawn_agent" => {
                let Some(task_id) = arg_str(args, "task_id").and_then(|s| TaskId::parse(s).ok()) else {
                    return ToolOutcome::err("task_id must be a task id");
                };
                let role = arg_str(args, "role").and_then(parse_role).unwrap_or(orkestr_domain::AgentRole::Backend);
                let task = match self.deps.tasks.get(task_id).await {
                    Ok(t) if t.project_id == pid => t,
                    Ok(_) => return ToolOutcome::err("the task belongs to another project"),
                    Err(e) => return ToolOutcome::err(e.to_string()),
                };
                if let Err(e) = self
                    .gate_action(ctx, &format!("Start a {} agent on \"{}\"", role_name(&role), task.title), None)
                    .await
                {
                    return ToolOutcome::err(e);
                }
                let templates = self.deps.agents.templates();
                let Some(template) = templates.list(Some(role.clone()), None).await.ok().and_then(|l| {
                    l.iter()
                        .find(|t| t.source == orkestr_domain::TemplateSource::Builtin)
                        .cloned()
                        .or_else(|| l.into_iter().next())
                }) else {
                    return ToolOutcome::err(format!("no template for role {}", role_name(&role)));
                };
                match self
                    .deps
                    .agents
                    .spawn(SpawnAgentParams {
                        project_id: pid,
                        template_id: template.id,
                        task_id: Some(task_id),
                        new_task: None,
                        model: None,
                        worktree_policy: None,
                        name: None,
                        max_turns: None,
                        base_branch: None,
                        approval_mode: Some(ctx.policy),
                        budget: None,
                    })
                    .await
                {
                    Ok((agent, _)) => ToolOutcome::ok(
                        format!("Agent {} ({}) started on task {}.", agent.name, agent.id, task.title),
                        format!("started {}", agent.name),
                    ),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "stop_agent" => {
                let Some(id) = arg_str(args, "agent_id").and_then(|s| AgentId::parse(s).ok()) else {
                    return ToolOutcome::err("agent_id must be an agent id");
                };
                match self.deps.agents.get(id).await {
                    Ok(a) if a.project_id == pid => match self.deps.agents.stop(id, AgentStopMode::Interrupt).await {
                        Ok(a) => ToolOutcome::ok(
                            format!("{} stopped ({:?}).", a.name, a.state),
                            format!("stopped {}", a.name),
                        ),
                        Err(e) => ToolOutcome::err(e.to_string()),
                    },
                    Ok(_) => ToolOutcome::err("the agent belongs to another project"),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "request_approval" => {
                let Some(summary) = arg_str(args, "summary") else { return ToolOutcome::err("summary is required") };
                let risk = match arg_str(args, "risk").map(str::to_ascii_lowercase).as_deref() {
                    Some("low") => RiskLevel::Low,
                    Some("high") => RiskLevel::High,
                    Some("critical") => RiskLevel::Critical,
                    _ => RiskLevel::Medium,
                };
                let mut req = ApprovalRequest::new(
                    Capability::Write,
                    cap(summary, 300),
                    cap(arg_str(args, "reason").unwrap_or("the AI Manager asks for your decision"), 1000),
                    risk,
                );
                req.project_id = Some(pid);
                match self
                    .deps
                    .security
                    .approvals
                    .request_and_wait(req, self.deps.config.approval_timeout, &ctx.cancel)
                    .await
                {
                    Ok((a, status)) => {
                        let note = self
                            .deps
                            .security
                            .approvals
                            .get(a.id)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|a| a.resolution_note)
                            .map(|n| format!(" Note: {n}"))
                            .unwrap_or_default();
                        ToolOutcome::ok(format!("The user's decision: {status:?}.{note}"), format!("{status:?}"))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "record_decision" => {
                let (Some(title), Some(decision)) = (arg_str(args, "title"), arg_str(args, "decision")) else {
                    return ToolOutcome::err("title and decision are required");
                };
                let alternatives: Vec<orkestr_domain::DecisionAlternative> = args
                    .get("alternatives")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .take(10)
                            .map(|s| orkestr_domain::DecisionAlternative {
                                title: cap(s, 200),
                                summary: String::new(),
                                proposed_by: Some("AI Manager".into()),
                                rejected_reason: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self
                    .deps
                    .memory
                    .create_decision(CreateDecisionParams {
                        project_id: pid,
                        title: cap(title, 200),
                        decision: cap(decision, 10_000),
                        context: arg_str(args, "context").map(|s| cap(s, 10_000)),
                        alternatives: Some(alternatives),
                        reasoning_summary: arg_str(args, "rationale_summary").map(|s| cap(s, 4_000)),
                        consequences: None,
                        status: None,
                        task_ids: None,
                        files: None,
                        decided_by: Some(orkestr_domain::Actor::Manager),
                    })
                    .await
                {
                    Ok(d) => ToolOutcome::ok(
                        format!("Decision recorded: {} ({})", d.title, d.id),
                        format!("recorded \"{}\"", d.title),
                    ),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "answer_user" => {
                let text = arg_str(args, "text").unwrap_or_default().to_owned();
                ToolOutcome {
                    content: "delivered".into(),
                    summary: "answered".into(),
                    final_answer: Some(text),
                    ..ToolOutcome::default()
                }
            }
            other => ToolOutcome::err(format!("unknown tool `{other}`")),
        }
    }

    async fn overview(&self, project: &Project) -> ToolOutcome {
        let tasks = self
            .deps
            .tasks
            .list(ListTasksParams { project_id: Some(project.id), ..ListTasksParams::default() })
            .await
            .unwrap_or_default();
        let mut by_status: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for t in &tasks {
            *by_status.entry(format!("{:?}", t.status)).or_default() += 1;
        }
        let agents = self
            .deps
            .agents
            .list(ListAgentsParams { project_id: Some(project.id), task_id: None, active_only: Some(true) })
            .await
            .unwrap_or_default();
        let approvals = self
            .deps
            .security
            .approvals
            .list(&ApprovalFilter {
                status: Some(ApprovalStatus::Pending),
                project_id: Some(project.id),
                limit: Some(100),
            })
            .await
            .map(|l| l.len())
            .unwrap_or(0);
        let run = self.active_run(project.id).await.ok().flatten();
        let snapshot =
            self.deps.snapshots.list(project.id, None, None, 1).await.ok().and_then(|l| l.into_iter().next());
        let cost = self.deps.cost.summary(Some(project.id)).await.ok().map(|s| s.total.cost_usd);
        let v = json!({
            "project": {"name": project.name, "root": project.root_path, "approvalMode": project.approval_mode},
            "tasks": {"total": tasks.len(), "byStatus": by_status},
            "runningAgents": agents.iter().map(|a| json!({"name": a.name, "state": a.state, "taskId": a.task_id})).collect::<Vec<_>>(),
            "activeRun": run.as_ref().map(|r| json!({"id": r.id, "goal": r.goal, "status": r.status,
                "completed": r.tasks.iter().filter(|t| t.status == TaskStatus::Completed).count(), "total": r.tasks.len()})),
            "pendingApprovals": approvals,
            "lastSnapshot": snapshot.map(|s| json!({"label": s.label, "createdAt": s.created_at})),
            "totalSpendUsd": cost,
        });
        ToolOutcome::ok(to_json(&v), format!("{} task(s), {} running agent(s)", tasks.len(), agents.len()))
    }

    async fn create_tasks_tool(&self, ctx: &TurnCtx, args: &Value) -> ToolOutcome {
        let Some(items) = args.get("tasks").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
            return ToolOutcome::err("tasks must be a non-empty array");
        };
        if items.len() > MAX_PLAN_TASKS {
            return ToolOutcome::err(format!("at most {MAX_PLAN_TASKS} tasks per call"));
        }
        let titles: Vec<String> =
            items.iter().filter_map(|t| t.get("title").and_then(Value::as_str)).map(|t| one_line(t, 120)).collect();
        if let Err(e) = self.gate_action(ctx, &format!("Create {} task(s)", items.len()), Some(titles.join("\n"))).await
        {
            return ToolOutcome::err(e);
        }
        let mut created: Vec<TaskId> = Vec::new();
        for item in items {
            let Some(title) = arg_str(item, "title") else { return ToolOutcome::err("every task needs a title") };
            let deps: Vec<TaskId> = item
                .get("depends_on")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|d| match d {
                            Value::Number(n) => n.as_u64().and_then(|i| created.get(i as usize).copied()),
                            Value::String(s) => s
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| created.get(i).copied())
                                .or_else(|| TaskId::parse(s.trim()).ok()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let priority =
                arg_str(item, "priority").map(|p| crate::plan::parse_priority(Some(p))).unwrap_or(TaskPriority::Normal);
            let criteria: Option<Vec<String>> = item
                .get("acceptance_criteria")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|s| cap(s, 500)).collect());
            match self
                .deps
                .tasks
                .create(
                    CreateTaskParams {
                        project_id: ctx.project.id,
                        title: cap(title, 200),
                        description: arg_str(item, "description").map(|d| cap(d, 20_000)),
                        acceptance_criteria: criteria,
                        priority: Some(priority),
                        depends_on: Some(deps),
                        tags: Some(vec!["manager".into()]),
                        ..CreateTaskParams::default()
                    },
                    TaskActor::Manager,
                )
                .await
            {
                Ok(t) => created.push(t.id),
                Err(e) => {
                    return ToolOutcome::err(format!(
                        "created {} task(s), then failed on \"{}\": {e}",
                        created.len(),
                        one_line(title, 80)
                    ))
                }
            }
        }
        ToolOutcome::ok(
            format!(
                "Created {} task(s): {}",
                created.len(),
                created.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ),
            format!("created {} task(s)", created.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_cover_every_tool_with_object_schemas() {
        let specs = tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        for n in STATE_TOOLS.iter().chain(ACTION_TOOLS.iter()) {
            assert!(names.contains(n), "missing tool {n}");
        }
        assert_eq!(specs.len(), STATE_TOOLS.len() + ACTION_TOOLS.len());
        for s in &specs {
            assert_eq!(s.parameters["type"], "object", "{}", s.name);
        }
    }
}
