//! `cost.*`, `budgets.*` handlers (§22).
//!
//! Usage records are ingested by engines (providers / agent runtime) through
//! `Services::cost.ingest`, which also fires `BudgetThreshold` events; these
//! handlers expose aggregates and budget management.

use orkestr_cost::{BudgetContext, CostError};
use orkestr_protocol::methods::cost::{
    self as m, BudgetListResult, BudgetResult, CostByDimensionResult, CostGranularity, CostRecordsResult,
    CostTimeseriesResult, CostWindow, DeleteBudgetResult,
};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Maps engine errors to RPC errors without leaking internals.
pub fn cost_to_rpc(err: CostError) -> RpcError {
    match err {
        CostError::NotFound(m) => RpcError::not_found(format!("{m} not found")),
        CostError::Invalid(m) => RpcError::bad_request(m),
        CostError::Storage(e) => storage_to_rpc(e),
        CostError::Events(m) => {
            tracing::error!(error = %m, "cost event publication failed");
            RpcError::internal("change stored but event publication failed")
        }
    }
}

/// Registers the areas.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Summary, _, _>(|ctx: RpcContext, p| async move {
            ctx.services.cost.summary(p.project_id).await.map_err(cost_to_rpc)
        })
        .method::<m::ByDimension, _, _>(|ctx: RpcContext, p| async move {
            let rows = ctx
                .services
                .cost
                .by_dimension(
                    p.dimension,
                    p.window.unwrap_or(CostWindow::Month),
                    p.project_id,
                    p.limit.unwrap_or(50).clamp(1, 500),
                )
                .await
                .map_err(cost_to_rpc)?;
            Ok(CostByDimensionResult { rows })
        })
        .method::<m::Timeseries, _, _>(|ctx: RpcContext, p| async move {
            let default = match p.granularity {
                CostGranularity::Day => 30,
                CostGranularity::Month => 12,
            };
            let buckets = p.buckets.unwrap_or(default).clamp(1, 366);
            let points =
                ctx.services.cost.timeseries(p.granularity, buckets, p.project_id).await.map_err(cost_to_rpc)?;
            Ok(CostTimeseriesResult { points })
        })
        .method::<m::Records, _, _>(|ctx: RpcContext, p| async move {
            let records = ctx.services.cost.records(p).await.map_err(cost_to_rpc)?;
            Ok(CostRecordsResult { records })
        })
        .method::<m::BudgetsList, _, _>(|ctx: RpcContext, p| async move {
            let budgets = ctx.services.cost.list_budgets(p.project_id).await.map_err(cost_to_rpc)?;
            Ok(BudgetListResult { budgets })
        })
        .method::<m::BudgetsSet, _, _>(|ctx: RpcContext, p| async move {
            let budget = ctx.services.cost.set_budget(p).await.map_err(cost_to_rpc)?;
            Ok(BudgetResult { budget })
        })
        .method::<m::BudgetsDelete, _, _>(|ctx: RpcContext, p| async move {
            ctx.services.cost.delete_budget(&p.id).await.map_err(cost_to_rpc)?;
            Ok(DeleteBudgetResult { id: p.id })
        })
        .method::<m::BudgetsCheck, _, _>(|ctx: RpcContext, p| async move {
            let bctx = BudgetContext {
                project_id: p.project_id,
                task_id: p.task_id,
                provider_id: p.provider_id,
                agent_id: p.agent_id,
            };
            ctx.services.cost.check(bctx, p.estimated_cost_usd.unwrap_or(0.0)).await.map_err(cost_to_rpc)
        });
}
