//! `replay.*` handlers (§21, §68.50). Loads the stored records a timeline is
//! built around (Manager run, plan, the user request, or a task) and lets
//! [`orkestr_insight::replay`] reconstruct the timeline from the persisted
//! event log. Read-only; hidden chain-of-thought is never stored, so it can
//! never appear in a replay.

use orkestr_insight::replay::{self, ReplaySeeds};
use orkestr_protocol::methods::replay::{
    self as r, ReplayRunsParams, ReplayRunsResult, ReplayStepParams, ReplayStepResult, ReplaySubject, ReplayTimeline,
    ReplayTimelineParams, ReplayTimelineResult,
};
use orkestr_protocol::RpcError;

use super::architecture::insight_to_rpc;
use super::manager::manager_to_rpc;
use super::tasks::task_to_rpc;
use super::{RpcContext, RpcRouter};

/// Messages of a conversation searched for the request that led to a plan.
const REQUEST_LOOKBACK: u32 = 500;
/// Default / max runs of `replay.runs`.
const DEFAULT_RUNS: u32 = 50;
const MAX_RUNS: u32 = 200;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<r::Runs, _, _>(|ctx: RpcContext, p: ReplayRunsParams| async move {
            let limit = p.limit.unwrap_or(DEFAULT_RUNS).clamp(1, MAX_RUNS);
            let runs = ctx
                .services
                .manager
                .store()
                .runs(Some(p.project_id), None, limit)
                .await
                .map_err(manager_to_rpc)?;
            Ok(ReplayRunsResult { runs: runs.iter().map(replay::run_summary).collect() })
        })
        .method::<r::Timeline, _, _>(|ctx: RpcContext, p: ReplayTimelineParams| async move {
            Ok(ReplayTimelineResult { timeline: timeline(&ctx, &p).await? })
        })
        .method::<r::Step, _, _>(|ctx: RpcContext, p: ReplayStepParams| async move {
            let tl = timeline(&ctx, &p.timeline).await?;
            let res: ReplayStepResult =
                replay::step(&tl, p.index.unwrap_or(0), p.direction.unwrap_or_default(), p.phase);
            Ok(res)
        });
}

/// Loads the seeds of a timeline request.
async fn seeds(ctx: &RpcContext, p: &ReplayTimelineParams) -> Result<ReplaySeeds, RpcError> {
    match (p.run_id, p.task_id) {
        (Some(run_id), None) => {
            let store = ctx.services.manager.store();
            let run = store.run(run_id).await.map_err(manager_to_rpc)?;
            if run.project_id != p.project_id {
                return Err(RpcError::not_found(format!("run {run_id} not found in this project")));
            }
            let plan = match store.plan(run.plan_id).await {
                Ok(plan) => Some(plan),
                Err(orkestr_orchestrator::OrchestratorError::NotFound(_)) => None,
                Err(e) => return Err(manager_to_rpc(e)),
            };
            let request = match plan.as_ref().and_then(|pl| pl.conversation_id.map(|c| (pl, c))) {
                Some((pl, conv)) => {
                    let messages = store.messages(conv, REQUEST_LOOKBACK).await.map_err(manager_to_rpc)?;
                    replay::request_for_plan(pl, &messages)
                }
                None => None,
            };
            Ok(ReplaySeeds { project_id: p.project_id, subject: ReplaySubject::Run, run: Some(run), plan, request, task: None })
        }
        (None, Some(task_id)) => {
            let task = ctx.services.tasks.get(task_id).await.map_err(task_to_rpc)?;
            if task.project_id != p.project_id {
                return Err(RpcError::not_found(format!("task {task_id} not found in this project")));
            }
            Ok(ReplaySeeds {
                project_id: p.project_id,
                subject: ReplaySubject::Task,
                run: None,
                plan: None,
                request: None,
                task: Some(task),
            })
        }
        _ => Err(RpcError::bad_request("pass exactly one of runId / taskId")),
    }
}

/// Builds the (filtered) timeline of a request.
async fn timeline(ctx: &RpcContext, p: &ReplayTimelineParams) -> Result<ReplayTimeline, RpcError> {
    let seeds = seeds(ctx, p).await?;
    let gathered = replay::gather_default(&ctx.services.events, &seeds).await.map_err(insight_to_rpc)?;
    let full = replay::build(&seeds, &gathered);
    Ok(replay::filter(full, p.agent_id, p.phases.as_deref()))
}
