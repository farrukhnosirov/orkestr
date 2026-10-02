//! `vcs.*` handlers (§16): git status/diff/history/branches/worktrees/merge.
//!
//! The repository is the project's `rootPath`; `worktree` params are
//! validated against the repository's worktree list. Dangerous operations
//! (`discard`, forced branch delete / worktree removal) require
//! `confirm: true` and take a safety snapshot first where work could be lost.

use std::path::{Path, PathBuf};

use orkestr_domain::{Actor, ProjectId, SnapshotId, SnapshotReason};
use orkestr_protocol::event::{EventKind, FileChangeKind, FileChangedPayload, SnapshotCreatedPayload};
use orkestr_protocol::methods::vcs::{
    self as m, AddWorktreeParams, BranchResult, CheckoutParams, CommitParams, CommitResult, CreateBranchParams,
    DeleteBranchParams, DeleteBranchResult, DiffParams, DiffResult, DiscardParams, DiscardResult, InitParams,
    LogParams, LogResult, MergeParams, MergePreview, MergePreviewParams, MergeResult, PathsParams,
    RemoveWorktreeParams, RemoveWorktreeResult, RepoParams, RepoStatus, ShowParams, ShowResult, WorktreeResult,
    WorktreesResult, DEFAULT_LOG_LIMIT, DEFAULT_MAX_PATCH_BYTES,
};
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_snapshots::CreateSnapshot;
use orkestr_vcs::{AddWorktreeRequest, DiffRequest, VcsError};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};

/// Max `FileChanged` events emitted per operation.
pub(crate) const MAX_FILE_EVENTS: usize = 200;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(status)
        .method::<m::Diff, _, _>(diff)
        .method::<m::Log, _, _>(log)
        .method::<m::Show, _, _>(show)
        .method::<m::Branches, _, _>(|ctx: RpcContext, p: RepoParams| async move {
            let dir = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?.1;
            ctx.services.vcs.git().branches(&dir).await.map_err(vcs_to_rpc)
        })
        .method::<m::CreateBranch, _, _>(create_branch)
        .method::<m::DeleteBranch, _, _>(delete_branch)
        .method::<m::Checkout, _, _>(checkout)
        .method::<m::Stage, _, _>(stage)
        .method::<m::Unstage, _, _>(unstage)
        .method::<m::Discard, _, _>(discard)
        .method::<m::Commit, _, _>(commit)
        .method::<m::Worktrees, _, _>(worktrees)
        .method::<m::AddWorktree, _, _>(add_worktree)
        .method::<m::RemoveWorktree, _, _>(remove_worktree)
        .method::<m::MergePreviewMethod, _, _>(merge_preview)
        .method::<m::Merge, _, _>(merge)
        .method::<m::AbortMerge, _, _>(abort_merge)
        .method::<m::Init, _, _>(init);
}

/// Maps VCS errors to RPC errors.
pub(crate) fn vcs_to_rpc(err: VcsError) -> RpcError {
    match err {
        VcsError::GitNotFound
        | VcsError::Unsupported(_)
        | VcsError::NotARepository(_)
        | VcsError::InvalidArgument(_) => RpcError::bad_request(err.to_string()),
        VcsError::Command { .. } => RpcError::bad_request(err.to_string()),
        VcsError::NotFound(m) => RpcError::not_found(m),
        VcsError::ConfirmationRequired(_) => {
            RpcError::new(orkestr_protocol::RpcErrorCode::ApprovalRequired, err.to_string())
        }
        VcsError::Conflict(ref paths) => {
            RpcError::conflict(err.to_string()).with_data(serde_json::json!({ "conflicts": paths }))
        }
        VcsError::Dirty(_) | VcsError::RefChanged(_) | VcsError::NothingToCommit | VcsError::Locked(_) => {
            RpcError::conflict(err.to_string())
        }
        VcsError::Storage(e) => storage_to_rpc(e),
        VcsError::Timeout { .. } | VcsError::Io(_) => {
            tracing::error!(error = %err, "vcs failure");
            RpcError::internal(err.to_string())
        }
    }
}

pub(crate) fn approval_required(what: &str) -> RpcError {
    RpcError::new(
        orkestr_protocol::RpcErrorCode::ApprovalRequired,
        format!("{what} is a Dangerous action: repeat the request with `confirm: true`"),
    )
}

/// The project's root folder.
pub(crate) async fn project_root(ctx: &RpcContext, project_id: ProjectId) -> Result<PathBuf, RpcError> {
    let project = ctx
        .services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))?;
    let root = project
        .root_path
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| RpcError::bad_request("the project has no root folder; set rootPath first"))?;
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err(RpcError::bad_request(format!("project root {} does not exist", root.display())));
    }
    Ok(root)
}

/// `(project root, addressed directory)`.
pub(crate) async fn resolve(
    ctx: &RpcContext,
    project_id: ProjectId,
    worktree: Option<&str>,
) -> Result<(PathBuf, PathBuf), RpcError> {
    let root = project_root(ctx, project_id).await?;
    let dir = ctx.services.vcs.resolve_dir(&root, worktree).await.map_err(vcs_to_rpc)?;
    Ok((root, dir))
}

pub(crate) async fn publish(ctx: &RpcContext, ev: NewEvent) -> Result<(), RpcError> {
    ctx.services.events.publish(ev).await.map(|_| ()).map_err(events_to_rpc)
}

pub(crate) async fn publish_file_changes(
    ctx: &RpcContext,
    project_id: ProjectId,
    changes: Vec<(String, FileChangeKind)>,
) {
    for (path, change) in changes.into_iter().take(MAX_FILE_EVENTS) {
        let ev = NewEvent::new(EventKind::FileChanged(FileChangedPayload {
            path,
            old_path: None,
            change,
            actor: orkestr_protocol::event::FileChangeActor::User,
            agent_id: None,
        }))
            .project(project_id);
        if let Err(e) = publish(ctx, ev).await {
            tracing::warn!(error = %e.message, "failed to publish FileChanged");
        }
    }
}

/// Takes an automatic safety snapshot of `dir` and publishes `SnapshotCreated`.
pub(crate) async fn safety_snapshot(
    ctx: &RpcContext,
    project_id: ProjectId,
    dir: &Path,
    label: String,
) -> Result<SnapshotId, RpcError> {
    let req = CreateSnapshot {
        label: Some(label),
        reason: SnapshotReason::BeforeRiskyAction,
        created_by: Actor::System,
        ..CreateSnapshot::manual(project_id, dir)
    };
    let snap = ctx.services.snapshots.create(req).await.map_err(super::snapshots::snapshot_to_rpc)?;
    let id = snap.id;
    publish(ctx, NewEvent::new(EventKind::SnapshotCreated(SnapshotCreatedPayload { snapshot: snap }))).await?;
    Ok(id)
}

async fn status(ctx: RpcContext, p: RepoParams) -> Result<RepoStatus, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services.vcs.git().status(&dir).await.map_err(vcs_to_rpc)
}

async fn diff(ctx: RpcContext, p: DiffParams) -> Result<DiffResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let req = DiffRequest {
        target: p.target,
        paths: p.paths.unwrap_or_default(),
        context_lines: p.context_lines.unwrap_or(3),
        include_untracked: p.include_untracked.unwrap_or(false),
        max_patch_bytes: usize::try_from(p.max_patch_bytes.unwrap_or(DEFAULT_MAX_PATCH_BYTES).min(16 * 1024 * 1024))
            .unwrap_or(usize::MAX),
    };
    ctx.services.vcs.git().diff(&dir, &req).await.map_err(vcs_to_rpc)
}

async fn log(ctx: RpcContext, p: LogParams) -> Result<LogResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services
        .vcs
        .git()
        .log(&dir, p.rev.as_deref(), p.path.as_deref(), p.skip.unwrap_or(0), p.limit.unwrap_or(DEFAULT_LOG_LIMIT))
        .await
        .map_err(vcs_to_rpc)
}

async fn show(ctx: RpcContext, p: ShowParams) -> Result<ShowResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services.vcs.git().show(&dir, &p.path, &p.rev).await.map_err(vcs_to_rpc)
}

async fn create_branch(ctx: RpcContext, p: CreateBranchParams) -> Result<BranchResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services
        .vcs
        .git()
        .create_branch(&dir, &p.name, p.start_point.as_deref(), p.checkout.unwrap_or(false))
        .await
        .map_err(vcs_to_rpc)
}

async fn delete_branch(ctx: RpcContext, p: DeleteBranchParams) -> Result<DeleteBranchResult, RpcError> {
    let force = p.force.unwrap_or(false);
    if force && !p.confirm.unwrap_or(false) {
        return Err(approval_required("force-deleting an unmerged branch"));
    }
    let root = project_root(&ctx, p.project_id).await?;
    ctx.services.vcs.git().delete_branch(&root, &p.name, force).await.map_err(vcs_to_rpc)
}

async fn checkout(ctx: RpcContext, p: CheckoutParams) -> Result<BranchResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services.vcs.git().checkout(&dir, &p.branch).await.map_err(vcs_to_rpc)
}

async fn stage(ctx: RpcContext, p: PathsParams) -> Result<RepoStatus, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let git = ctx.services.vcs.git();
    git.stage(&dir, &p.paths, p.all.unwrap_or(false)).await.map_err(vcs_to_rpc)?;
    git.status(&dir).await.map_err(vcs_to_rpc)
}

async fn unstage(ctx: RpcContext, p: PathsParams) -> Result<RepoStatus, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let git = ctx.services.vcs.git();
    git.unstage(&dir, &p.paths, p.all.unwrap_or(false)).await.map_err(vcs_to_rpc)?;
    git.status(&dir).await.map_err(vcs_to_rpc)
}

async fn discard(ctx: RpcContext, p: DiscardParams) -> Result<DiscardResult, RpcError> {
    if !p.confirm {
        return Err(approval_required("discarding uncommitted changes"));
    }
    if p.paths.is_empty() {
        return Err(RpcError::bad_request("no paths given"));
    }
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let git = ctx.services.vcs.git();
    let root = git.require_repo(&dir).await.map_err(vcs_to_rpc)?.root;
    let label = format!("Before discarding {} path(s)", p.paths.len());
    let safety = safety_snapshot(&ctx, p.project_id, &root, label).await?;
    let (restored, deleted) = git.discard(&dir, &p.paths).await.map_err(vcs_to_rpc)?;
    let changes = restored
        .iter()
        .map(|p| (p.clone(), FileChangeKind::Modified))
        .chain(deleted.iter().map(|p| (p.clone(), FileChangeKind::Deleted)))
        .collect::<Vec<_>>();
    publish_file_changes(&ctx, p.project_id, changes).await;
    let status = git.status(&dir).await.map_err(vcs_to_rpc)?;
    Ok(DiscardResult { restored, deleted, safety_snapshot_id: Some(safety), status })
}

async fn commit(ctx: RpcContext, p: CommitParams) -> Result<CommitResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let trailers = p.trailers.unwrap_or_default();
    let commit = ctx
        .services
        .vcs
        .git()
        .commit(&dir, &p.message, p.allow_empty.unwrap_or(false), &trailers)
        .await
        .map_err(vcs_to_rpc)?;
    Ok(CommitResult { commit })
}

async fn worktrees(ctx: RpcContext, p: RepoParams) -> Result<WorktreesResult, RpcError> {
    let root = project_root(&ctx, p.project_id).await?;
    let worktrees = ctx.services.vcs.worktrees(p.project_id, &root).await.map_err(vcs_to_rpc)?;
    Ok(WorktreesResult { worktrees })
}

async fn add_worktree(ctx: RpcContext, p: AddWorktreeParams) -> Result<WorktreeResult, RpcError> {
    let root = project_root(&ctx, p.project_id).await?;
    let req =
        AddWorktreeRequest { agent_id: p.agent_id, role: p.role, task_id: p.task_id, branch: p.branch, base: p.base };
    let worktree = ctx.services.vcs.add_worktree(p.project_id, &root, req).await.map_err(vcs_to_rpc)?;
    Ok(WorktreeResult { worktree })
}

async fn remove_worktree(ctx: RpcContext, p: RemoveWorktreeParams) -> Result<RemoveWorktreeResult, RpcError> {
    let force = p.force.unwrap_or(false);
    if force && !p.confirm.unwrap_or(false) {
        return Err(approval_required("force-removing a worktree"));
    }
    let root = project_root(&ctx, p.project_id).await?;
    let vcs = &ctx.services.vcs;
    let mut safety = None;
    if force {
        let dir = vcs.resolve_dir(&root, Some(&p.path)).await.map_err(vcs_to_rpc)?;
        if dir.is_dir() && vcs.is_dirty(&dir).await.map_err(vcs_to_rpc)? {
            let label = format!("Before removing worktree {}", dir.display());
            safety = Some(safety_snapshot(&ctx, p.project_id, &dir, label).await?);
        }
    }
    let path = vcs.remove_worktree(p.project_id, &root, &p.path, force).await.map_err(vcs_to_rpc)?;
    Ok(RemoveWorktreeResult { path, safety_snapshot_id: safety })
}

async fn merge_preview(ctx: RpcContext, p: MergePreviewParams) -> Result<MergePreview, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services.vcs.git().merge_preview(&dir, &p.source, p.target.as_deref()).await.map_err(vcs_to_rpc)
}

async fn merge(ctx: RpcContext, p: MergeParams) -> Result<MergeResult, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    ctx.services
        .vcs
        .git()
        .merge(&dir, &p.source, p.target.as_deref(), p.message.as_deref(), p.allow_conflicts.unwrap_or(false))
        .await
        .map_err(vcs_to_rpc)
}

async fn abort_merge(ctx: RpcContext, p: RepoParams) -> Result<RepoStatus, RpcError> {
    let (_, dir) = resolve(&ctx, p.project_id, p.worktree.as_deref()).await?;
    let git = ctx.services.vcs.git();
    git.abort_merge(&dir).await.map_err(vcs_to_rpc)?;
    git.status(&dir).await.map_err(vcs_to_rpc)
}

async fn init(ctx: RpcContext, p: InitParams) -> Result<RepoStatus, RpcError> {
    let root = project_root(&ctx, p.project_id).await?;
    let git = ctx.services.vcs.git();
    if git.repo_info(&root).await.map_err(vcs_to_rpc)?.is_some() {
        return Err(RpcError::conflict("the project folder is already inside a git repository"));
    }
    git.init(&root, p.initial_branch.as_deref()).await.map_err(vcs_to_rpc)?;
    tracing::info!(project_id = %p.project_id, "git repository initialised");
    git.status(&root).await.map_err(vcs_to_rpc)
}
