//! `benchmarks.*` handlers (§22, §68.38–39): model benchmark table, agent
//! performance analytics and raw execution records. Data is collected by
//! [`crate::benchmarks`] from events and joined with usage records by the
//! cost engine.

use orkestr_protocol::methods::benchmarks::{
    self as m, AgentPerformanceResult, BenchmarkRecordsResult, ModelBenchmarksResult,
};
use orkestr_cost::benchmarks::{DEFAULT_DAYS, MAX_DAYS};

use super::cost::cost_to_rpc;
use super::{RpcContext, RpcRouter};

fn days(d: Option<u32>) -> u32 {
    d.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS)
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Models, _, _>(|ctx: RpcContext, p| async move {
            let window = days(p.days);
            let mut rows = ctx
                .services
                .cost
                .model_benchmark_rows(m::ModelBenchmarksParams { days: Some(window), ..p })
                .await
                .map_err(cost_to_rpc)?;
            for r in &mut rows {
                r.routing_score = crate::benchmarks::routing_score(r);
            }
            Ok(ModelBenchmarksResult { rows, days: window })
        })
        .method::<m::Agents, _, _>(|ctx: RpcContext, p| async move {
            let window = days(p.days);
            let rows = ctx
                .services
                .cost
                .agent_performance_rows(m::AgentPerformanceParams { days: Some(window), ..p })
                .await
                .map_err(cost_to_rpc)?;
            Ok(AgentPerformanceResult { rows, days: window })
        })
        .method::<m::Records, _, _>(|ctx: RpcContext, p| async move {
            let records = ctx.services.cost.benchmark_records(p).await.map_err(cost_to_rpc)?;
            Ok(BenchmarkRecordsResult { records })
        });
}
