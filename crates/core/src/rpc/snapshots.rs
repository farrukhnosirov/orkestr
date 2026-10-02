//! `snapshots.*` handlers (§25, §63.J).
//!
//! Emits `SnapshotCreated` for every snapshot (including automatic
//! `PreRollback` safety snapshots), `RollbackPerformed` after each applied
//! rollback and `FileChanged` for every file a rollback wrote or deleted.

use std::path::{Path, PathBuf};

use orkestr_domain::{Actor, ProjectId, SnapshotId, SnapshotReason, WorkspaceSnapshot};
use orkestr_protocol::event::{
    EventKind, FileChangeKind, RollbackPerformedPayload, RollbackScope, SnapshotCreatedPayload,
};
use orkestr_protocol::methods::snapshots::{
    self as m, CreateSnapshotParams, DeleteSnapshotResult, ListSnapshotsParams, ListSnapshotsResult,
    PlanRollbackParams, RestoreFileParams, RollbackPlan, RollbackResult, RollbackTaskParams, RollbackWorkspaceParams,
    SnapshotDiffParams, SnapshotDiffResult, SnapshotIdParams, SnapshotResult,
};
use orkestr_protocol::{NewEvent, RpcError};
use orkestr_snapshots::{CreateSnapshot, SnapshotError, WorkspaceRollback};

use super::vcs::{approval_required, project_root, publish, publish_file_changes, vcs_to_rpc};
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(list)
        .method::<m::Get, _, _>(|ctx: RpcContext, p: SnapshotIdParams| async move {
            ctx.services.snapshots.detail(p.snapshot_id).await.map_err(snapshot_to_rpc)
        })
        .method::<m::Create, _, _>(create)
        .method::<m::Diff, _, _>(|ctx: RpcContext, p: SnapshotDiffParams| async move {
            let r: SnapshotDiffResult =
                ctx.services.snapshots.diff(p.snapshot_id, p.path.as_deref()).await.map_err(snapshot_to_rpc)?;
            Ok(r)
        })
        .method::<m::RestoreFile, _, _>(restore_file)
        .method::<m::PlanRollback, _, _>(|ctx: RpcContext, p: PlanRollbackParams| async move {
            let r: RollbackPlan = ctx
                .services
                .snapshots
                .plan_rollback(p.snapshot_id, p.delete_created.unwrap_or(false))
                .await
                .map_err(snapshot_to_rpc)?;
            Ok(r)
        })
        .method::<m::RollbackWorkspace, _, _>(rollback_workspace)
        .method::<m::RollbackTask, _, _>(rollback_task)
        .method::<m::Delete, _, _>(|ctx: RpcContext, p: SnapshotIdParams| async move {
            ctx.services.snapshots.delete(p.snapshot_id).await.map_err(snapshot_to_rpc)?;
            Ok(DeleteSnapshotResult { snapshot_id: p.snapshot_id })
        });
}

/// Maps snapshot errors to RPC errors.
pub(crate) fn snapshot_to_rpc(err: SnapshotError) -> RpcError {
    match err {
        SnapshotError::NotFound(m) => RpcError::not_found(m),
        SnapshotError::InvalidInput(m) | SnapshotError::TooLarge(m) => RpcError::bad_request(m),
        SnapshotError::ConfirmationRequired(_) => {
            RpcError::new(orkestr_protocol::RpcErrorCode::ApprovalRequired, err.to_string())
        }
        SnapshotError::Stale(m) => RpcError::conflict(m),
        SnapshotError::Storage(e) => storage_to_rpc(e),
        SnapshotError::Io(_) | SnapshotError::Json(_) => {
            tracing::error!(error = %err, "snapshot failure");
            RpcError::internal(err.to_string())
        }
    }
}

/// Resolves the directory to capture: the project root or one of its worktrees.
async fn workspace_dir(ctx: &RpcContext, project_id: ProjectId, path: Option<&str>) -> Result<PathBuf, RpcError> {
    let root = project_root(ctx, project_id).await?;
    let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(root);
    };
    if orkestr_vcs::naming::path_key(Path::new(p)) == orkestr_vcs::naming::path_key(&root) {
        return Ok(root);
    }
    ctx.services.vcs.resolve_dir(&root, Some(p)).await.map_err(vcs_to_rpc)
}

async fn announce(ctx: &RpcContext, snap: WorkspaceSnapshot) -> Result<(), RpcError> {
    publish(ctx, NewEvent::new(EventKind::SnapshotCreated(SnapshotCreatedPayload { snapshot: snap }))).await
}

async fn list(ctx: RpcContext, p: ListSnapshotsParams) -> Result<ListSnapshotsResult, RpcError> {
    let ws = p.workspace_path.as_deref().map(PathBuf::from);
    let snapshots = ctx
        .services
        .snapshots
        .list(p.project_id, p.task_id, ws.as_deref(), p.limit.unwrap_or(200))
        .await
        .map_err(snapshot_to_rpc)?;
    Ok(ListSnapshotsResult { snapshots })
}

async fn create(ctx: RpcContext, p: CreateSnapshotParams) -> Result<SnapshotResult, RpcError> {
    let dir = workspace_dir(&ctx, p.project_id, p.workspace_path.as_deref()).await?;
    let req = CreateSnapshot {
        project_id: p.project_id,
        workspace_path: dir,
        label: p.label,
        reason: p.reason.unwrap_or(SnapshotReason::Manual),
        task_id: p.task_id,
        agent_id: p.agent_id,
        task_phase: p.task_phase,
        created_by: p.agent_id.map_or(Actor::System, Actor::Agent),
    };
    let snapshot = ctx.services.snapshots.create(req).await.map_err(snapshot_to_rpc)?;
    announce(&ctx, snapshot.clone()).await?;
    Ok(SnapshotResult { snapshot })
}

/// Publishes the safety snapshot, `RollbackPerformed` and `FileChanged`.
async fn after_rollback(
    ctx: &RpcContext,
    project_id: ProjectId,
    scope: RollbackScope,
    res: &RollbackResult,
    task_id: Option<orkestr_domain::TaskId>,
) -> Result<(), RpcError> {
    if let Some(id) = res.pre_rollback_snapshot_id {
        match ctx.services.snapshots.get(id).await {
            Ok(s) => announce(ctx, s).await?,
            Err(e) => tracing::warn!(error = %e, "safety snapshot vanished"),
        }
    }
    if res.dry_run || (res.restored.is_empty() && res.deleted.is_empty()) {
        return Ok(());
    }
    let mut ev = NewEvent::new(EventKind::RollbackPerformed(RollbackPerformedPayload {
        snapshot_id: res.snapshot_id,
        scope,
        files_restored: u32::try_from(res.restored.len() + res.deleted.len()).unwrap_or(u32::MAX),
        pre_rollback_snapshot_id: res.pre_rollback_snapshot_id,
    }))
    .project(project_id);
    if let Some(t) = task_id {
        ev = ev.task(t);
    }
    publish(ctx, ev).await?;
    let changes = res
        .restored
        .iter()
        .map(|p| (p.clone(), FileChangeKind::Modified))
        .chain(res.deleted.iter().map(|p| (p.clone(), FileChangeKind::Deleted)))
        .collect::<Vec<_>>();
    publish_file_changes(ctx, project_id, changes).await;
    Ok(())
}

async fn project_of(ctx: &RpcContext, id: SnapshotId) -> Result<ProjectId, RpcError> {
    Ok(ctx.services.snapshots.get(id).await.map_err(snapshot_to_rpc)?.project_id)
}

async fn restore_file(ctx: RpcContext, p: RestoreFileParams) -> Result<RollbackResult, RpcError> {
    if !p.confirm {
        return Err(approval_required("restoring a file from a snapshot"));
    }
    let project_id = project_of(&ctx, p.snapshot_id).await?;
    let res = ctx.services.snapshots.restore_file(p.snapshot_id, &p.path).await.map_err(snapshot_to_rpc)?;
    after_rollback(&ctx, project_id, RollbackScope::Workspace, &res, None).await?;
    Ok(res)
}

async fn rollback_workspace(ctx: RpcContext, p: RollbackWorkspaceParams) -> Result<RollbackResult, RpcError> {
    if !p.confirm {
        return Err(approval_required("rolling back the workspace"));
    }
    let project_id = project_of(&ctx, p.snapshot_id).await?;
    let opts = WorkspaceRollback {
        delete_created: p.delete_created.unwrap_or(false),
        paths: p.paths,
        expected_fingerprint: p.expected_fingerprint,
    };
    let res = ctx.services.snapshots.rollback_workspace(p.snapshot_id, opts).await.map_err(snapshot_to_rpc)?;
    after_rollback(&ctx, project_id, RollbackScope::Workspace, &res, None).await?;
    Ok(res)
}

async fn rollback_task(ctx: RpcContext, p: RollbackTaskParams) -> Result<RollbackResult, RpcError> {
    let dry_run = p.dry_run.unwrap_or(false);
    if !dry_run && !p.confirm {
        return Err(approval_required("rolling back a task"));
    }
    let res = ctx
        .services
        .snapshots
        .rollback_task(p.project_id, p.task_id, p.before_snapshot_id, p.after_snapshot_id, dry_run)
        .await
        .map_err(snapshot_to_rpc)?;
    after_rollback(&ctx, p.project_id, RollbackScope::Task, &res, Some(p.task_id)).await?;
    Ok(res)
}
