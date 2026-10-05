//! `scheduler.*` handlers (§11, §12, §68.24–26): the global parallel
//! scheduler of the AI Manager ([`orkestr_orchestrator::ParallelScheduler`]).

use orkestr_protocol::methods::scheduler::{
    self as s, SchedulerExplainParams, SchedulerStatusParams, SetSchedulerLimitsParams, SetSchedulerLimitsResult,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<s::Status, _, _>(|ctx: RpcContext, p: SchedulerStatusParams| async move {
            ctx.services.manager.parallel().status(p.project_id).await.map_err(manager_to_rpc)
        })
        .method::<s::SetLimits, _, _>(|ctx: RpcContext, p: SetSchedulerLimitsParams| async move {
            let settings = ctx.services.manager.parallel().set_settings(p.settings).await.map_err(manager_to_rpc)?;
            Ok(SetSchedulerLimitsResult { settings })
        })
        .method::<s::Explain, _, _>(|ctx: RpcContext, p: SchedulerExplainParams| async move {
            ctx.services.manager.parallel().explain(p.task_id).await.map_err(manager_to_rpc)
        });
}
