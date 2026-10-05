//! `pipeline.*` handlers (§17, §68.15–16, Phase 2): the quality pipeline of
//! a task and its Self-Healing loop. Thin layer over
//! [`orkestr_orchestrator::Manager`]; progress streams as `PipelineUpdated`,
//! `PipelineStageChanged` and `HealingAttemptStarted` events.

use orkestr_protocol::methods::pipeline::{
    self as m, GetPipelineParams, ListPipelinesParams, ListPipelinesResult, PipelineConfigParams,
    SetPipelineConfigParams, StartPipelineParams,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Get, _, _>(|ctx: RpcContext, p: GetPipelineParams| async move {
            ctx.services.manager.pipeline(p.task_id).await.map_err(manager_to_rpc)
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListPipelinesParams| async move {
            Ok(ListPipelinesResult { pipelines: ctx.services.manager.list_pipelines(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Start, _, _>(|ctx: RpcContext, p: StartPipelineParams| async move {
            ctx.services.manager.start_pipeline(p.task_id).await.map_err(manager_to_rpc)
        })
        .method::<m::GetConfig, _, _>(|ctx: RpcContext, p: PipelineConfigParams| async move {
            ctx.services.manager.pipeline_config(p.project_id).await.map_err(manager_to_rpc)
        })
        .method::<m::SetConfig, _, _>(|ctx: RpcContext, p: SetPipelineConfigParams| async move {
            ctx.services.manager.set_pipeline_config(p).await.map_err(manager_to_rpc)
        });
}
