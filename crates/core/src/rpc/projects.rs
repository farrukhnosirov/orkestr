//! `projects.*` handlers (§13).
//!
//! Deleting a project removes its metadata only — never files on disk
//! (§60.8 no silent destructive actions).

use std::path::Path;

use orkestr_domain::{now, slugify, Project};
use orkestr_protocol::event::{EventKind, ProjectDeletedPayload, ProjectPayload};
use orkestr_protocol::methods::projects::{
    self as m, validate_color, validate_description, validate_name, CreateProjectParams, DeleteProjectResult,
    ListProjectsResult, ProjectIdParams, ProjectPatch, ProjectResult, UpdateProjectParams,
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
    if let Some(a) = p.approval_mode {
        project.approval_mode = a;
    }
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

async fn delete(ctx: RpcContext, p: ProjectIdParams) -> Result<DeleteProjectResult, RpcError> {
    let existed = ctx.services.projects.delete(p.project_id).await.map_err(storage_to_rpc)?;
    if !existed {
        return Err(RpcError::not_found(format!("project {} not found", p.project_id)));
    }
    tracing::info!(project_id = %p.project_id, "project deleted (metadata only)");
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
