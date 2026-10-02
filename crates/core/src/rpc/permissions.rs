//! `permissions.*` handlers (§23).

use std::path::PathBuf;

use orkestr_domain::{now, Actor, AuditOutcome, Project, ProjectId};
use orkestr_protocol::event::{EventKind, ProjectPayload};
use orkestr_protocol::methods::permissions::{
    self as m, CheckParams, CheckResource, CommandShell, DecisionKind, DecisionView, DeleteRuleParams, DeletedResult, GetModeParams,
    ListRulesParams, ListRulesResult, ModeResult, RuleResult, SetModeParams, SetRuleParams,
};
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_security::permissions::RuleFilter;
use orkestr_security::{CheckContext, Decision, NewAudit, PermissionEngine, Resource, ShellKind};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, security_to_rpc, storage_to_rpc};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(list)
        .method::<m::Set, _, _>(set)
        .method::<m::Delete, _, _>(delete)
        .method::<m::Check, _, _>(check)
        .method::<m::GetMode, _, _>(get_mode)
        .method::<m::SetMode, _, _>(set_mode);
}

pub(crate) async fn load_project(ctx: &RpcContext, id: ProjectId) -> Result<Project, RpcError> {
    ctx.services
        .projects
        .get(id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {id} not found")))
}

async fn list(ctx: RpcContext, p: ListRulesParams) -> Result<ListRulesResult, RpcError> {
    let filter = RuleFilter { project_id: p.project_id, agent_id: p.agent_id, task_id: p.task_id };
    let rules = ctx.services.security.permissions.list(&filter).await.map_err(security_to_rpc)?;
    Ok(ListRulesResult { rules })
}

async fn set(ctx: RpcContext, p: SetRuleParams) -> Result<RuleResult, RpcError> {
    let sec = &ctx.services.security;
    let rule = sec.permissions.set(p.id, p.rule, p.note, p.expires_at, sec.user()).await.map_err(security_to_rpc)?;
    tracing::info!(rule_id = %rule.id, capability = ?rule.rule.capability, effect = ?rule.rule.effect, "permission rule saved");
    Ok(RuleResult { rule })
}

async fn delete(ctx: RpcContext, p: DeleteRuleParams) -> Result<DeletedResult, RpcError> {
    let sec = &ctx.services.security;
    let deleted = sec.permissions.delete(&p.id, sec.user()).await.map_err(security_to_rpc)?;
    Ok(DeletedResult { deleted })
}

fn to_resource(r: CheckResource) -> Resource {
    match r {
        CheckResource::None => Resource::None,
        CheckResource::Path { path, recursive } => Resource::Path { path, recursive },
        CheckResource::Command { command, shell, cwd } => Resource::Command {
            command,
            shell: match shell {
                CommandShell::PowerShell => ShellKind::PowerShell,
                CommandShell::Cmd => ShellKind::Cmd,
                CommandShell::Bash => ShellKind::Bash,
                CommandShell::Sh => ShellKind::Sh,
            },
            cwd,
        },
        CheckResource::Git { args } => Resource::Git { args },
        CheckResource::Url { url } => Resource::Url { url },
        CheckResource::Secret { name } => Resource::Secret { name },
        CheckResource::Sql { statement } => Resource::Sql { statement },
        CheckResource::Deploy { environment, protected } => Resource::Deploy { environment, protected },
        CheckResource::Other { value } => Resource::Other { value },
    }
}

/// Converts an engine decision to its wire form.
#[must_use]
pub fn decision_view(d: Decision, effective_mode: orkestr_domain::ApprovalMode) -> DecisionView {
    match d {
        Decision::Allow => DecisionView {
            decision: DecisionKind::Allow,
            code: String::new(),
            message: "allowed".into(),
            risk: None,
            danger: None,
            rememberable: false,
            hard: false,
            rule_id: None,
            effective_mode,
        },
        Decision::Ask(a) => DecisionView {
            decision: DecisionKind::Ask,
            code: a.code,
            message: a.message,
            risk: Some(a.risk),
            danger: a.danger,
            rememberable: a.rememberable,
            hard: false,
            rule_id: a.rule_id,
            effective_mode,
        },
        Decision::Deny(r) => DecisionView {
            decision: DecisionKind::Deny,
            code: r.code,
            message: r.message,
            risk: None,
            danger: r.danger,
            rememberable: false,
            hard: r.hard,
            rule_id: r.rule_id,
            effective_mode,
        },
    }
}

async fn check(ctx: RpcContext, p: CheckParams) -> Result<DecisionView, RpcError> {
    let sec = &ctx.services.security;
    let project = match p.project_id {
        Some(id) => Some(load_project(&ctx, id).await?),
        None => None,
    };
    let base_mode = p.approval_mode.or(project.as_ref().map(|pr| pr.approval_mode)).unwrap_or_default();
    let mode = sec.permissions.effective_mode(base_mode, p.agent_id).await.map_err(security_to_rpc)?;
    let actor = p.actor.unwrap_or(match p.agent_id {
        Some(a) => Actor::Agent(a),
        None => Actor::Manager,
    });
    let worktree_root = p.worktree_root.or(project.as_ref().and_then(|pr| pr.root_path.clone())).map(PathBuf::from);
    let cctx = CheckContext {
        project_id: p.project_id,
        agent_id: p.agent_id,
        task_id: p.task_id,
        approval_mode: mode,
        worktree_root,
        protected_environment: false,
    };
    let resource = to_resource(p.resource);
    let engine = std::sync::Arc::clone(&sec.engine);
    let capability = p.capability;
    // Path confinement touches the filesystem: run off the async workers.
    let decision = tokio::task::spawn_blocking(move || engine.check(&actor, capability, &resource, &cctx))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "permission check task failed");
            RpcError::internal("permission check failed")
        })?;
    Ok(decision_view(decision, mode))
}

async fn mode_result(ctx: &RpcContext, project: &Project, agent: Option<orkestr_domain::AgentId>) -> Result<ModeResult, RpcError> {
    let sec = &ctx.services.security;
    let agent_mode = match agent {
        Some(a) => sec.permissions.agent_mode(a).await.map_err(security_to_rpc)?,
        None => None,
    };
    let effective_mode = sec.engine.effective_mode(agent_mode.unwrap_or(project.approval_mode));
    Ok(ModeResult {
        project_id: project.id,
        project_mode: project.approval_mode,
        agent_id: agent,
        agent_mode,
        effective_mode,
        full_auto_disabled: sec.emergency.flags().full_auto_disabled,
    })
}

async fn get_mode(ctx: RpcContext, p: GetModeParams) -> Result<ModeResult, RpcError> {
    let project = load_project(&ctx, p.project_id).await?;
    mode_result(&ctx, &project, p.agent_id).await
}

async fn set_mode(ctx: RpcContext, p: SetModeParams) -> Result<ModeResult, RpcError> {
    let sec = &ctx.services.security;
    let mut project = load_project(&ctx, p.project_id).await?;
    match p.agent_id {
        Some(agent) => {
            sec.permissions.set_agent_mode(agent, p.mode, sec.user()).await.map_err(security_to_rpc)?;
        }
        None => {
            let mode = p.mode.ok_or_else(|| RpcError::bad_request("mode is required for a project"))?;
            if project.approval_mode != mode {
                let from = project.approval_mode;
                project.approval_mode = mode;
                project.updated_at = now();
                ctx.services.projects.update(&project).await.map_err(storage_to_rpc)?;
                ctx.services
                    .events
                    .publish(NewEvent::new(EventKind::ProjectUpdated(ProjectPayload { project: project.clone() })))
                    .await
                    .map_err(events_to_rpc)?;
                sec.audit
                    .record(
                        NewAudit::new(sec.user(), "permissions.setMode", format!("project:{}", project.id), AuditOutcome::Executed)
                            .project(Some(project.id))
                            .details(serde_json::json!({ "from": from, "to": mode })),
                    )
                    .await;
                tracing::info!(project_id = %project.id, ?mode, "approval mode changed");
            }
        }
    }
    mode_result(&ctx, &project, p.agent_id).await
}
