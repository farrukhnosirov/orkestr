//! `projects.*` handlers (§13).
//!
//! Deleting a project removes its metadata only — never files on disk
//! (§60.8 no silent destructive actions).

use std::path::Path;

use orkestr_domain::{now, slugify, Project};
use orkestr_protocol::event::{EventKind, ProjectDeletedPayload, ProjectPayload};
use orkestr_protocol::methods::projects::{
    self as m, validate_color, validate_description, validate_name, CreateProjectParams, DeleteProjectParams,
    DeleteProjectResult, ListProjectsResult, ProjectIdParams, ProjectPatch, ProjectResult, UpdateProjectParams,
};
use orkestr_protocol::{NewEvent, RpcError};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p| async move {
            let projects =
                ctx.services.projects.list(p.include_archived.unwrap_or(false)).await.map_err(storage_to_rpc)?;
            Ok(ListProjectsResult { projects })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: ProjectIdParams| async move {
            Ok(ProjectResult { project: load(&ctx, &p).await? })
        })
        .method::<m::Create, _, _>(create)
        .method::<m::Update, _, _>(update)
        .method::<m::Delete, _, _>(delete)
        .method::<m::Open, _, _>(open);
}

fn bad(e: impl std::fmt::Display) -> RpcError {
    RpcError::bad_request(e.to_string())
}

fn validate_root_path(path: &str) -> Result<String, RpcError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(RpcError::bad_request("rootPath must be an absolute path"));
    }
    if !p.is_dir() {
        return Err(RpcError::bad_request("rootPath must be an existing directory"));
    }
    Ok(p.to_string_lossy().into_owned())
}

fn validate_tags(tags: &[String]) -> Result<Vec<String>, RpcError> {
    if tags.len() > 32 {
        return Err(RpcError::bad_request("at most 32 tags"));
    }
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if t.is_empty() || t.chars().count() > 40 {
            return Err(RpcError::bad_request("tags must be 1..=40 characters"));
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            out.push(t.to_owned());
        }
    }
    Ok(out)
}

fn validate_icon(icon: &str) -> Result<(), RpcError> {
    let ok = !icon.is_empty() && icon.len() <= 48 && icon.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then_some(()).ok_or_else(|| RpcError::bad_request("icon must be an icon name ([A-Za-z0-9-], ≤48)"))
}

async fn load(ctx: &RpcContext, p: &ProjectIdParams) -> Result<Project, RpcError> {
    ctx.services
        .projects
        .get(p.project_id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {} not found", p.project_id)))
}

async fn publish(ctx: &RpcContext, kind: EventKind) -> Result<(), RpcError> {
    ctx.services.events.publish(NewEvent::new(kind)).await.map(|_| ()).map_err(events_to_rpc)
}

async fn create(ctx: RpcContext, p: CreateProjectParams) -> Result<ProjectResult, RpcError> {
    let name = validate_name(&p.name).map_err(bad)?;
    let mut project = Project::new(name, now());
    if let Some(d) = p.description {
        validate_description(&d).map_err(bad)?;
        project.description = d;
    }
    if let Some(root) = p.root_path.filter(|r| !r.trim().is_empty()) {
        project.root_path = Some(validate_root_path(root.trim())?);
    }
    if let Some(c) = p.color.filter(|c| !c.is_empty()) {
        validate_color(&c).map_err(bad)?;
        project.color = Some(c);
    }
    if let Some(i) = p.icon.filter(|i| !i.is_empty()) {
        validate_icon(&i)?;
        project.icon = Some(i);
    }
    if let Some(c) = p.classification {
        project.classification = c;
    }
    // New projects start in the user's default approval mode (Settings /
    // onboarding) unless the request names one.
    project.approval_mode = p.approval_mode.unwrap_or_else(|| ctx.services.settings.get().default_approval_mode);
    if let Some(pr) = p.privacy {
        project.privacy = pr;
    }
    if let Some(tags) = p.tags {
        project.tags = validate_tags(&tags)?;
    }
    ctx.services.projects.insert(&project).await.map_err(storage_to_rpc)?;
    tracing::info!(project_id = %project.id, name = %project.name, "project created");
    publish(&ctx, EventKind::ProjectCreated(ProjectPayload { project: project.clone() })).await?;
    Ok(ProjectResult { project })
}

fn apply_patch(project: &mut Project, patch: ProjectPatch) -> Result<(), RpcError> {
    if let Some(name) = patch.name {
        let name = validate_name(&name).map_err(bad)?;
        project.slug = slugify(&name);
        project.name = name;
    }
    if let Some(d) = patch.description {
        validate_description(&d).map_err(bad)?;
        project.description = d;
    }
    if let Some(root) = patch.root_path {
        project.root_path = if root.trim().is_empty() { None } else { Some(validate_root_path(root.trim())?) };
    }
    if let Some(c) = patch.color {
        if c.is_empty() {
            project.color = None;
        } else {
            validate_color(&c).map_err(bad)?;
            project.color = Some(c);
        }
    }
    if let Some(i) = patch.icon {
        if i.is_empty() {
            project.icon = None;
        } else {
            validate_icon(&i)?;
            project.icon = Some(i);
        }
    }
    if let Some(c) = patch.classification {
        project.classification = c;
    }
    if let Some(a) = patch.approval_mode {
        project.approval_mode = a;
    }
    if let Some(pr) = patch.privacy {
        project.privacy = pr;
    }
    if let Some(tags) = patch.tags {
        project.tags = validate_tags(&tags)?;
    }
    if let Some(archived) = patch.archived {
        project.archived = archived;
    }
    Ok(())
}

async fn update(ctx: RpcContext, p: UpdateProjectParams) -> Result<ProjectResult, RpcError> {
    let mut project = load(&ctx, &ProjectIdParams { project_id: p.project_id }).await?;
    apply_patch(&mut project, p.patch)?;
    project.updated_at = now();
    ctx.services.projects.update(&project).await.map_err(storage_to_rpc)?;
    publish(&ctx, EventKind::ProjectUpdated(ProjectPayload { project: project.clone() })).await?;
    Ok(ProjectResult { project })
}

/// In-process work of one project that must end before its rows go away
/// (deleting cascades to agents, tasks, merge proposals and runs; running work
/// would otherwise keep editing files with no handle left to stop it).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ActiveWork {
    pub agents: usize,
    pub manager_runs: usize,
    pub manager_chat: bool,
    pub merges: usize,
    pub quality_runs: usize,
}

impl ActiveWork {
    fn is_idle(&self) -> bool {
        *self == Self::default()
    }
}

pub(crate) async fn active_work(ctx: &RpcContext, project: orkestr_domain::ProjectId) -> Result<ActiveWork, RpcError> {
    use orkestr_protocol::methods::manager::ManagerStatusParams;
    use orkestr_protocol::methods::merge::{MergeListParams, MergeProposalStatus};
    let s = &ctx.services;
    let status = s
        .manager
        .status(ManagerStatusParams { project_id: Some(project), ..Default::default() })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let manager_runs = status.runs.iter().filter(|r| status.live_run_ids.contains(&r.id)).count();
    let merges = s
        .merge
        .list(MergeListParams { project_id: project, run_id: None, limit: Some(500) })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .iter()
        .filter(|m| {
            matches!(
                m.status,
                MergeProposalStatus::AwaitingApproval
                    | MergeProposalStatus::Merging
                    | MergeProposalStatus::Testing
                    | MergeProposalStatus::Conflicted
            )
        })
        .count();
    Ok(ActiveWork {
        agents: s.security.emergency.active_agents_in(project),
        manager_runs,
        manager_chat: s.manager.is_busy(project),
        merges,
        quality_runs: s.quality.active_in_project(project),
    })
}

/// Stops a project's running work (project-scoped emergency stop, quality
/// runs, agent terminals, open merges aborted and rolled back).
async fn stop_project_work(ctx: &RpcContext, project: orkestr_domain::ProjectId) {
    use orkestr_protocol::methods::merge::{MergeAbortParams, MergeListParams};
    let s = &ctx.services;
    if let Err(e) = s.security.stop_project(project, s.security.user()).await {
        tracing::warn!(%project, error = %e, "stop project before delete");
    }
    s.terminals.stop_project_agents(project).await;
    s.quality.cancel_project(project);
    s.claude.manager.stop_project(project);
    if let Ok(list) = s.merge.list(MergeListParams { project_id: project, run_id: None, limit: Some(500) }).await {
        for m in list.into_iter().filter(|m| !m.status.is_final()) {
            let params = MergeAbortParams { proposal_id: m.id, reason: Some("project deleted".into()) };
            if let Err(e) = s.merge.abort(params).await {
                tracing::debug!(proposal = %m.id, error = %e, "abort merge before delete");
            }
        }
    }
}

/// How long `projects.delete {stopWork}` waits for stopped work to end.
const STOP_WORK_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

async fn delete(ctx: RpcContext, p: DeleteProjectParams) -> Result<DeleteProjectResult, RpcError> {
    load(&ctx, &ProjectIdParams { project_id: p.project_id }).await?;
    let mut work = active_work(&ctx, p.project_id).await?;
    if !work.is_idle() && p.stop_work.unwrap_or(false) {
        stop_project_work(&ctx, p.project_id).await;
        let deadline = tokio::time::Instant::now() + STOP_WORK_WAIT;
        loop {
            work = active_work(&ctx, p.project_id).await?;
            if work.is_idle() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    if !work.is_idle() {
        return Err(RpcError::conflict(format!(
            "the project still has running work ({} agent(s), {} Manager run(s){}, {} merge(s), {} quality run(s)); \
             stop it first (emergency.stopProject, merge.abort) and delete again",
            work.agents,
            work.manager_runs,
            if work.manager_chat { ", a Manager reply" } else { "" },
            work.merges,
            work.quality_runs,
        ))
        .with_data(serde_json::json!({
            "reason": "project_busy",
            "agents": work.agents,
            "managerRuns": work.manager_runs,
            "managerChat": work.manager_chat,
            "merges": work.merges,
            "qualityRuns": work.quality_runs,
        })));
    }
    let existed = ctx.services.projects.delete(p.project_id).await.map_err(storage_to_rpc)?;
    if !existed {
        return Err(RpcError::not_found(format!("project {} not found", p.project_id)));
    }
    tracing::info!(project_id = %p.project_id, "project deleted (metadata only)");
    // Language servers of the project (ADR-0025).
    ctx.services.lsp.stop_project(p.project_id);
    publish(&ctx, EventKind::ProjectDeleted(ProjectDeletedPayload { project_id: p.project_id })).await?;
    Ok(DeleteProjectResult { project_id: p.project_id })
}

async fn open(ctx: RpcContext, p: ProjectIdParams) -> Result<ProjectResult, RpcError> {
    let mut project = load(&ctx, &p).await?;
    let t = now();
    project.last_opened_at = Some(t);
    project.updated_at = t;
    ctx.services.projects.update(&project).await.map_err(storage_to_rpc)?;
    publish(&ctx, EventKind::ProjectUpdated(ProjectPayload { project: project.clone() })).await?;
    Ok(ProjectResult { project })
}
