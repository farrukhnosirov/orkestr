//! `files.*` handlers (§14 Files, §15 Code) on top of `orkestr-files`.
//!
//! The workspace is the project's `rootPath`, or an agent worktree below
//! `<data_dir>/worktrees` when `worktreePath` is given. Every path is
//! confined to that root by `orkestr_files::path` (SR-21). Mutations publish
//! `FileChanged` with the actor (`user`, or `agent` when `agentId` is given)
//! under the request's correlation id; the watcher publishes `external` ones.

use std::path::PathBuf;

use orkestr_domain::{AgentId, ProjectId};
use orkestr_events::EventBus;
use orkestr_files::{ChangeActor, ChangeSink, FilesError, SearchOptions, Workspace};
use orkestr_protocol::event::{EventKind, FileChangedPayload};
use orkestr_protocol::methods::files::{
    self as m, CreateFileParams, DeleteFileParams, FileEntryResult, FilePathParams, FindFilesParams, ListFilesParams,
    ListTrashParams, ListTrashResult, MkdirParams, OpenExternalParams, OpenExternalResult, ReadFileParams,
    RenameFileParams, RestoreTrashedParams, SearchFilesParams, WatchFilesParams, WatchFilesResult, WriteFileParams,
};
use orkestr_protocol::{NewEvent, RpcError};
use serde_json::json;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListFilesParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let path = p.path.unwrap_or_default();
            ctx.services.files.list(&ws, &path, p.show_ignored.unwrap_or(false)).await.map_err(files_to_rpc)
        })
        .method::<m::Stat, _, _>(|ctx: RpcContext, p: FilePathParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let entry = ctx.services.files.stat(&ws, &p.path).await.map_err(files_to_rpc)?;
            Ok(FileEntryResult { entry })
        })
        .method::<m::Read, _, _>(|ctx: RpcContext, p: ReadFileParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            ctx.services.files.read(&ws, &p.path, p.max_bytes).await.map_err(files_to_rpc)
        })
        .method::<m::Write, _, _>(write)
        .method::<m::Create, _, _>(create)
        .method::<m::Mkdir, _, _>(mkdir)
        .method::<m::Rename, _, _>(rename)
        .method::<m::Delete, _, _>(delete)
        .method::<m::Trash, _, _>(|ctx: RpcContext, p: ListTrashParams| async move {
            project_exists(&ctx, p.project_id).await?;
            let items = ctx.services.files.trash_list(p.project_id, p.limit).await.map_err(files_to_rpc)?;
            Ok(ListTrashResult { items })
        })
        .method::<m::Restore, _, _>(restore)
        .method::<m::Search, _, _>(search)
        .method::<m::Find, _, _>(|ctx: RpcContext, p: FindFilesParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let max = p.max_results.map(|n| n as usize);
            ctx.services.files.find(&ws, &p.query, max, p.show_ignored.unwrap_or(false)).await.map_err(files_to_rpc)
        })
        .method::<m::Watch, _, _>(|ctx: RpcContext, p: WatchFilesParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let subscribers = ctx.services.files.watch(&ws, &ctx.session_id.to_string()).map_err(files_to_rpc)?;
            Ok(WatchFilesResult { watching: true, subscribers })
        })
        .method::<m::Unwatch, _, _>(|ctx: RpcContext, p: WatchFilesParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let subscribers = ctx.services.files.unwatch(&ws, &ctx.session_id.to_string());
            Ok(WatchFilesResult { watching: ctx.services.files.is_watching(&ws), subscribers })
        })
        .method::<m::OpenExternal, _, _>(|ctx: RpcContext, p: OpenExternalParams| async move {
            let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
            let path = p.path.unwrap_or_default();
            let program = ctx
                .services
                .files
                .open_external(&ws, &path, p.target.unwrap_or_default(), p.ide)
                .await
                .map_err(files_to_rpc)?;
            Ok(OpenExternalResult { program })
        });
}

/// Maps file service errors to RPC errors (`data.reason` lets clients branch).
pub fn files_to_rpc(err: FilesError) -> RpcError {
    match err {
        FilesError::InvalidPath { .. } | FilesError::InvalidRoot(_) | FilesError::InvalidQuery(_) => {
            RpcError::bad_request(err.to_string())
        }
        FilesError::OutsideRoot(_) | FilesError::Protected(_) | FilesError::PermissionDenied(_) => {
            RpcError::forbidden(err.to_string())
        }
        FilesError::NotFound(_) => RpcError::not_found(err.to_string()),
        FilesError::AlreadyExists(_) => RpcError::conflict(err.to_string()).with_data(json!({"reason": "exists"})),
        FilesError::Conflict { ref actual_hash, .. } => {
            let data = json!({"reason": "stale", "actualHash": actual_hash});
            RpcError::conflict(err.to_string()).with_data(data)
        }
        FilesError::TooLarge { size, limit, .. } => RpcError::bad_request(err.to_string())
            .with_data(json!({"reason": "too_large", "size": size, "limit": limit})),
        FilesError::RecursiveRequired(_) => {
            RpcError::bad_request(err.to_string()).with_data(json!({"reason": "recursive_required"}))
        }
        FilesError::NotAFile(_)
        | FilesError::NotADirectory(_)
        | FilesError::Encoding(_)
        | FilesError::Unsupported(_) => RpcError::bad_request(err.to_string()),
        FilesError::Watch(_) | FilesError::Storage(_) | FilesError::Internal(_) | FilesError::Io { .. } => {
            tracing::error!(error = %err, "file service failure");
            RpcError::internal(format!("file operation failed: {err}"))
        }
    }
}

async fn project_exists(ctx: &RpcContext, project_id: ProjectId) -> Result<orkestr_domain::Project, RpcError> {
    ctx.services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))
}

/// Resolves the workspace of a request.
async fn workspace(ctx: &RpcContext, project_id: ProjectId, worktree: Option<&str>) -> Result<Workspace, RpcError> {
    let project = project_exists(ctx, project_id).await?;
    let root: PathBuf = match worktree.map(str::trim).filter(|s| !s.is_empty()) {
        Some(w) => {
            let base = std::fs::canonicalize(ctx.services.paths.data_dir.join("worktrees"))
                .map_err(|_| RpcError::bad_request("no agent worktrees exist yet"))?;
            let wt = std::fs::canonicalize(w).map_err(|_| RpcError::bad_request("worktreePath does not exist"))?;
            if wt == base || !wt.starts_with(&base) {
                return Err(RpcError::forbidden(
                    "worktreePath must be an agent worktree inside the Orkestr data directory",
                ));
            }
            wt
        }
        None => PathBuf::from(
            project
                .root_path
                .ok_or_else(|| RpcError::bad_request("the project has no root folder (set rootPath first)"))?,
        ),
    };
    ctx.services.files.workspace(project.id, &root).map_err(files_to_rpc)
}

/// Permission check for agent-attributed mutations (contract §6): the
/// security engine decides with the workspace as confinement root. Users
/// are not gated here (they act through the UI on their own files).
/// `Ask` → `approval_required` (the agent runtime is expected to use
/// `ApprovalService::request_and_wait` itself); `Deny` → `forbidden`.
async fn agent_check(
    ctx: &RpcContext,
    ws: &Workspace,
    agent: Option<AgentId>,
    capability: orkestr_domain::Capability,
    paths: &[&str],
    recursive: bool,
) -> Result<(), RpcError> {
    use orkestr_security::{CheckContext, Decision, PermissionEngine, Resource};
    let Some(agent) = agent else { return Ok(()) };
    let sec = &ctx.services.security;
    let project_mode = project_exists(ctx, ws.project_id).await?.approval_mode;
    let mode =
        sec.permissions.effective_mode(project_mode, Some(agent)).await.map_err(crate::error::security_to_rpc)?;
    let check_ctx = CheckContext {
        project_id: Some(ws.project_id),
        agent_id: Some(agent),
        task_id: None,
        approval_mode: mode,
        worktree_root: Some(ws.root.path().to_path_buf()),
        protected_environment: false,
    };
    for path in paths {
        let resource = Resource::Path { path: (*path).to_owned(), recursive };
        match sec.engine.check(&orkestr_domain::Actor::Agent(agent), capability, &resource, &check_ctx) {
            Decision::Allow => {}
            Decision::Ask(r) => {
                return Err(RpcError::new(orkestr_protocol::RpcErrorCode::ApprovalRequired, r.message.clone())
                    .with_data(json!({ "code": r.code, "risk": r.risk, "danger": r.danger, "path": path })));
            }
            Decision::Deny(r) => {
                return Err(RpcError::forbidden(r.message.clone()).with_data(json!({ "code": r.code, "path": path })));
            }
        }
    }
    Ok(())
}

async fn publish(ctx: &RpcContext, project_id: ProjectId, changes: Vec<FileChangedPayload>) {
    publish_changes(&ctx.services.events, project_id, changes).await;
}

/// Publishes `FileChanged` events (failures are logged: the change already happened).
pub async fn publish_changes(events: &EventBus, project_id: ProjectId, changes: Vec<FileChangedPayload>) {
    for change in changes {
        let agent: Option<AgentId> = change.agent_id;
        let mut ev = NewEvent::new(EventKind::FileChanged(change)).project(project_id);
        if let Some(a) = agent {
            ev = ev.agent(a);
        }
        if let Err(e) = events.publish(ev).await {
            tracing::warn!(error = %e, "failed to publish FileChanged");
        }
    }
}

/// Publishes watcher batches on the event bus.
pub struct EventBusChangeSink {
    events: EventBus,
    handle: tokio::runtime::Handle,
}

impl EventBusChangeSink {
    /// Uses the current tokio runtime (call inside it).
    #[must_use]
    pub fn new(events: EventBus) -> Self {
        Self { events, handle: tokio::runtime::Handle::current() }
    }
}

impl ChangeSink for EventBusChangeSink {
    fn publish(&self, project_id: ProjectId, changes: Vec<FileChangedPayload>) {
        let events = self.events.clone();
        self.handle.spawn(async move { publish_changes(&events, project_id, changes).await });
    }
}

async fn write(ctx: RpcContext, p: WriteFileParams) -> Result<m::WriteFileResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    agent_check(&ctx, &ws, p.agent_id, orkestr_domain::Capability::Write, &[&p.path], false).await?;
    let out = ctx
        .services
        .files
        .write(
            &ws,
            &p.path,
            &p.content,
            p.encoding,
            p.expected_hash,
            p.create.unwrap_or(false),
            ChangeActor::from_agent(p.agent_id),
        )
        .await
        .map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(out.result)
}

async fn create(ctx: RpcContext, p: CreateFileParams) -> Result<FileEntryResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    agent_check(&ctx, &ws, p.agent_id, orkestr_domain::Capability::Write, &[&p.path], false).await?;
    let out = ctx
        .services
        .files
        .create_file(&ws, &p.path, p.content.as_deref(), ChangeActor::from_agent(p.agent_id))
        .await
        .map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(FileEntryResult { entry: out.result })
}

async fn mkdir(ctx: RpcContext, p: MkdirParams) -> Result<FileEntryResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    agent_check(&ctx, &ws, p.agent_id, orkestr_domain::Capability::Write, &[&p.path], false).await?;
    let out =
        ctx.services.files.mkdir(&ws, &p.path, ChangeActor::from_agent(p.agent_id)).await.map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(FileEntryResult { entry: out.result })
}

async fn rename(ctx: RpcContext, p: RenameFileParams) -> Result<m::RenameFileResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    agent_check(&ctx, &ws, p.agent_id, orkestr_domain::Capability::Write, &[&p.from, &p.to], false).await?;
    let out = ctx
        .services
        .files
        .rename(&ws, &p.from, &p.to, p.overwrite.unwrap_or(false), ChangeActor::from_agent(p.agent_id))
        .await
        .map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(out.result)
}

async fn delete(ctx: RpcContext, p: DeleteFileParams) -> Result<m::DeleteFileResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    agent_check(&ctx, &ws, p.agent_id, orkestr_domain::Capability::Delete, &[&p.path], p.recursive.unwrap_or(false))
        .await?;
    let out = ctx
        .services
        .files
        .delete(&ws, &p.path, p.recursive.unwrap_or(false), ChangeActor::from_agent(p.agent_id))
        .await
        .map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(out.result)
}

async fn restore(ctx: RpcContext, p: RestoreTrashedParams) -> Result<m::RestoreFileResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    let out = ctx
        .services
        .files
        .restore(&ws, &p.trash_id, p.overwrite.unwrap_or(false), ChangeActor::User)
        .await
        .map_err(files_to_rpc)?;
    publish(&ctx, ws.project_id, out.changes).await;
    Ok(out.result)
}

async fn search(ctx: RpcContext, p: SearchFilesParams) -> Result<m::SearchFilesResult, RpcError> {
    let ws = workspace(&ctx, p.project_id, p.worktree_path.as_deref()).await?;
    let opts = SearchOptions {
        query: p.query,
        is_regex: p.is_regex.unwrap_or(false),
        case_sensitive: p.case_sensitive.unwrap_or(false),
        whole_word: p.whole_word.unwrap_or(false),
        include: p.include.unwrap_or_default(),
        exclude: p.exclude.unwrap_or_default(),
        max_results: p.max_results.map_or(0, |n| n as usize),
        show_ignored: p.show_ignored.unwrap_or(false),
        max_file_bytes: 0,
    };
    ctx.services.files.search(&ws, opts).await.map_err(files_to_rpc)
}
