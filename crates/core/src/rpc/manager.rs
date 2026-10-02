//! `manager.*` handlers (§1, §5, §9, §11, §57). Thin layer over
//! [`orkestr_orchestrator::Manager`]. `manager.chat` returns at once; the
//! reply streams as `ManagerMessage` events and is stored
//! (`ManagerMessageAdded`). `manager.cancel` is handled locally with
//! cancellation tokens (no provider round-trip).

use orkestr_orchestrator::OrchestratorError;
use orkestr_protocol::methods::manager::{
    self as m, ApprovePlanParams, ManagerChatParams, ManagerHistoryParams, ManagerPlanParams, ManagerPlanResult,
    ManagerProjectParams, ManagerRunIdParams, ManagerRunParams, ManagerRunResult, ManagerStatusParams,
    RejectPlanParams,
};
use orkestr_protocol::{RpcError, RpcErrorCode};
use orkestr_task_engine::ResourceSnapshot;

use super::agents::agent_to_rpc;
use super::tasks::task_to_rpc;
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Chat, _, _>(|ctx: RpcContext, p: ManagerChatParams| async move {
            ctx.services.manager.chat(p).await.map_err(manager_to_rpc)
        })
        .method::<m::History, _, _>(|ctx: RpcContext, p: ManagerHistoryParams| async move {
            ctx.services.manager.history(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Clear, _, _>(|ctx: RpcContext, p: ManagerProjectParams| async move {
            ctx.services.manager.clear(p.project_id).await.map_err(manager_to_rpc)
        })
        .method::<m::PlanMethod, _, _>(|ctx: RpcContext, p: ManagerPlanParams| async move {
            Ok(ManagerPlanResult { plan: ctx.services.manager.plan(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::ApprovePlan, _, _>(|ctx: RpcContext, p: ApprovePlanParams| async move {
            ctx.services.manager.approve_plan(p).await.map_err(manager_to_rpc)
        })
        .method::<m::RejectPlan, _, _>(|ctx: RpcContext, p: RejectPlanParams| async move {
            Ok(ManagerPlanResult { plan: ctx.services.manager.reject_plan(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Run, _, _>(|ctx: RpcContext, p: ManagerRunParams| async move {
            Ok(ManagerRunResult { run: ctx.services.manager.start_run(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Pause, _, _>(|ctx: RpcContext, p: ManagerRunIdParams| async move {
            Ok(ManagerRunResult { run: ctx.services.manager.pause(p.run_id).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Resume, _, _>(|ctx: RpcContext, p: ManagerRunIdParams| async move {
            Ok(ManagerRunResult { run: ctx.services.manager.resume(p.run_id).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Cancel, _, _>(|ctx: RpcContext, p: ManagerRunIdParams| async move {
            Ok(ManagerRunResult { run: ctx.services.manager.cancel(p.run_id).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Status, _, _>(|ctx: RpcContext, p: ManagerStatusParams| async move {
            ctx.services.manager.status(p).await.map_err(manager_to_rpc)
        })
        .method::<m::ResumeSummaryMethod, _, _>(|ctx: RpcContext, p: ManagerProjectParams| async move {
            ctx.services.manager.resume_summary(p.project_id).await.map_err(manager_to_rpc)
        });
}

/// Maps orchestrator errors to RPC errors (contract §4 codes).
pub fn manager_to_rpc(err: OrchestratorError) -> RpcError {
    match err {
        OrchestratorError::NotFound(m) => RpcError::not_found(format!("{m} not found")),
        OrchestratorError::Invalid(m) => RpcError::bad_request(m),
        OrchestratorError::Conflict(m) => RpcError::conflict(m),
        OrchestratorError::Forbidden(m) => RpcError::forbidden(m),
        OrchestratorError::NoModel(m) | OrchestratorError::Planning(m) => RpcError::new(RpcErrorCode::ProviderError, m),
        OrchestratorError::Unimplemented(f) => RpcError::unimplemented_feature(&f),
        OrchestratorError::Agent(e) => agent_to_rpc(e),
        OrchestratorError::Task(e) => task_to_rpc(e),
        OrchestratorError::Storage(e) => storage_to_rpc(e),
        OrchestratorError::Internal(m) => {
            tracing::error!(error = %m, "manager failure while handling request");
            RpcError::internal("internal AI Manager error")
        }
    }
}

/// Host resources for the Manager's admission control, from the node
/// monitor's latest sample (sampled by housekeeping; unconstrained until the
/// first sample exists).
pub struct NodeResourceProbe(pub std::sync::Arc<orkestr_node::NodeMonitor>);

impl std::fmt::Debug for NodeResourceProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NodeResourceProbe")
    }
}

impl orkestr_orchestrator::ResourceProbe for NodeResourceProbe {
    fn snapshot(&self) -> ResourceSnapshot {
        match self.0.latest() {
            Some(s) => ResourceSnapshot {
                cpu_usage_percent: s.cpu_percent,
                logical_cores: u32::try_from(s.per_core_percent.len()).unwrap_or(1).max(1),
                ram_total_mb: s.memory_total_bytes / (1024 * 1024),
                ram_available_mb: s.memory_total_bytes.saturating_sub(s.memory_used_bytes) / (1024 * 1024),
            },
            None => ResourceSnapshot::unconstrained(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(manager_to_rpc(OrchestratorError::NotFound("plan x".into())).code.as_str(), "not_found");
        assert_eq!(manager_to_rpc(OrchestratorError::Conflict("x".into())).code.as_str(), "conflict");
        assert_eq!(manager_to_rpc(OrchestratorError::NoModel("x".into())).code.as_str(), "provider_error");
        assert_eq!(manager_to_rpc(OrchestratorError::Forbidden("x".into())).code.as_str(), "forbidden");
        let e = manager_to_rpc(OrchestratorError::Internal("secret path".into()));
        assert!(!e.message.contains("secret"));
    }
}
