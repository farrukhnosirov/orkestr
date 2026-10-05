//! `quality.*` handlers (§17, §58): toolchain detection, quality profiles,
//! check runs (project root, worktree or task worktree — every command passes
//! the security command gate via the terminal run API) and quality gates.
//!
//! Runs publish `BuildStarted` / `BuildSucceeded` / `BuildFailed` /
//! `TestRunFinished` (scoped to project/task/agent) from the quality service.

use std::path::{Path, PathBuf};

use orkestr_domain::{ProjectId, Task, TaskId};
use orkestr_protocol::methods::quality::{
    self as m, CancelRunResult, CheckKind, EvaluateParams, ListRunsParams, ListRunsResult, QualityProjectParams,
    RunChecksParams, RunIdParams, RunTarget, SetGatesParams, SetProfileParams,
};
use orkestr_protocol::RpcError;
use orkestr_quality::{QualityError, StartRun};

use super::tasks::task_to_rpc;
use super::vcs::{project_root, vcs_to_rpc};
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Detect, _, _>(|ctx: RpcContext, p: QualityProjectParams| async move {
            let root = project_root(&ctx, p.project_id).await?;
            Ok(ctx.services.quality.detect(&root).await)
        })
        .method::<m::GetProfile, _, _>(|ctx: RpcContext, p: QualityProjectParams| async move {
            let root = optional_root(&ctx, p.project_id).await?;
            ctx.services.quality.profile(p.project_id, root.as_deref()).await.map_err(quality_to_rpc)
        })
        .method::<m::SetProfile, _, _>(|ctx: RpcContext, p: SetProfileParams| async move {
            let root = optional_root(&ctx, p.project_id).await?;
            ctx.services
                .quality
                .set_profile(p.project_id, p.checks, p.reset.unwrap_or(false), root.as_deref())
                .await
                .map_err(quality_to_rpc)
        })
        .method::<m::Run, _, _>(run)
        .method::<m::Runs, _, _>(|ctx: RpcContext, p: ListRunsParams| async move {
            let runs = ctx
                .services
                .quality
                .runs(p.project_id, p.task_id, p.limit.unwrap_or(50))
                .await
                .map_err(quality_to_rpc)?;
            Ok(ListRunsResult { runs })
        })
        .method::<m::GetRun, _, _>(|ctx: RpcContext, p: RunIdParams| async move {
            ctx.services.quality.run(&p.run_id).await.map_err(quality_to_rpc)
        })
        .method::<m::Cancel, _, _>(|ctx: RpcContext, p: RunIdParams| async move {
            Ok::<_, RpcError>(CancelRunResult { cancelled: ctx.services.quality.cancel(&p.run_id) })
        })
        .method::<m::Gates, _, _>(|ctx: RpcContext, p: QualityProjectParams| async move {
            ensure_project(&ctx, p.project_id).await?;
            ctx.services.quality.gates(p.project_id).await.map_err(quality_to_rpc)
        })
        .method::<m::SetGates, _, _>(|ctx: RpcContext, p: SetGatesParams| async move {
            ensure_project(&ctx, p.project_id).await?;
            ctx.services
                .quality
                .set_gates(p.project_id, &p.gates, &p.dod, p.reset.unwrap_or(false))
                .await
                .map_err(quality_to_rpc)
        })
        .method::<m::Evaluate, _, _>(evaluate);
}

/// Maps quality errors to RPC errors.
pub(crate) fn quality_to_rpc(err: QualityError) -> RpcError {
    match err {
        QualityError::NotFound(m) => RpcError::not_found(m),
        QualityError::Invalid(_) | QualityError::NothingToRun(_) => RpcError::bad_request(err.to_string()),
        QualityError::Busy(_) => RpcError::conflict(err.to_string()),
        QualityError::Storage(e) => storage_to_rpc(e),
        QualityError::ShuttingDown | QualityError::Json(_) => {
            tracing::error!(error = %err, "quality failure");
            RpcError::internal(err.to_string())
        }
    }
}

async fn ensure_project(ctx: &RpcContext, project_id: ProjectId) -> Result<(), RpcError> {
    ctx.services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .map(|_| ())
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))
}

/// The project root when it has a usable one (profiles work without a root).
async fn optional_root(ctx: &RpcContext, project_id: ProjectId) -> Result<Option<PathBuf>, RpcError> {
    ensure_project(ctx, project_id).await?;
    Ok(project_root(ctx, project_id).await.ok())
}

async fn task_of(ctx: &RpcContext, project_id: ProjectId, task_id: TaskId) -> Result<Task, RpcError> {
    let task = ctx.services.tasks.get(task_id).await.map_err(task_to_rpc)?;
    if task.project_id != project_id {
        return Err(RpcError::bad_request(format!("task {task_id} belongs to another project")));
    }
    Ok(task)
}

/// A worktree of the project's repository (or the root itself).
async fn worktree_dir(ctx: &RpcContext, root: &Path, path: &str) -> Result<PathBuf, RpcError> {
    if orkestr_vcs::naming::path_key(Path::new(path)) == orkestr_vcs::naming::path_key(root) {
        return Ok(root.to_path_buf());
    }
    ctx.services.vcs.resolve_dir(root, Some(path)).await.map_err(vcs_to_rpc)
}

async fn run(ctx: RpcContext, p: RunChecksParams) -> Result<m::QualityRun, RpcError> {
    let root = project_root(&ctx, p.project_id).await?;
    let target = p.target.unwrap_or(RunTarget::Project);
    let (workspace, task_id) = match &target {
        RunTarget::Project => (root.clone(), None),
        RunTarget::Worktree { path } => (worktree_dir(&ctx, &root, path).await?, None),
        RunTarget::Task { task_id } => {
            let task = task_of(&ctx, p.project_id, *task_id).await?;
            let dir = match task.worktree.as_ref().map(|w| w.path.trim()).filter(|w| !w.is_empty()) {
                Some(path) => worktree_dir(&ctx, &root, path).await?,
                None => root.clone(),
            };
            (dir, Some(task.id))
        }
    };
    let quality = &ctx.services.quality;
    let profile = quality.profile(p.project_id, Some(&root)).await.map_err(quality_to_rpc)?;
    let budget = p.budget_secs.map_or_else(orkestr_quality::QualityService::default_budget, |s| {
        std::time::Duration::from_secs(u64::from(s.max(1)))
    });
    let started = quality
        .start(StartRun {
            project_id: p.project_id,
            task_id,
            agent_id: p.agent_id,
            target,
            workspace,
            checks: profile.checks,
            kinds: p.kinds.unwrap_or_default(),
            check_ids: p.check_ids.unwrap_or_default(),
            budget,
        })
        .await
        .map_err(quality_to_rpc)?;
    if p.wait.unwrap_or(false) {
        return quality.wait(&started.id).await.map_err(quality_to_rpc);
    }
    Ok(started)
}

async fn evaluate(ctx: RpcContext, p: EvaluateParams) -> Result<m::GateReport, RpcError> {
    ensure_project(&ctx, p.project_id).await?;
    let task = match p.task_id {
        Some(t) => Some(task_of(&ctx, p.project_id, t).await?),
        None => None,
    };
    let root = project_root(&ctx, p.project_id).await.ok();
    let quality = &ctx.services.quality;
    let profile = quality.profile(p.project_id, root.as_deref()).await.map_err(quality_to_rpc)?;
    let mut kinds: Vec<CheckKind> = profile.checks.iter().filter(|c| c.enabled).map(|c| c.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let snapshots = match &task {
        Some(t) => ctx.services.snapshots.list(p.project_id, Some(t.id), None, 1000).await.map(|s| s.len()).unwrap_or(0),
        None => 0,
    };
    quality.evaluate(p.project_id, task.as_ref(), &kinds, snapshots).await.map_err(quality_to_rpc)
}
