//! `nodes.*` handlers (local node, Phase 1).

use std::sync::Arc;

use orkestr_protocol::methods::nodes::{self as m, ListNodesResult, NodeInfo, NodeInfoParams, NodeResourcesResult};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};

/// Samples older than this are refreshed on `nodes.resources`.
const STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::seconds(5);

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _| async move {
            Ok(ListNodesResult { nodes: vec![ctx.services.nodes.local_node()] })
        })
        .method::<m::Info, _, _>(info)
        .method::<m::Resources, _, _>(|ctx: RpcContext, _| resources(ctx));
}

async fn info(ctx: RpcContext, p: NodeInfoParams) -> Result<NodeInfo, RpcError> {
    let monitor = Arc::clone(&ctx.services.nodes);
    let refresh = p.refresh.unwrap_or(false);
    let info = tokio::task::spawn_blocking(move || monitor.info(refresh))
        .await
        .map_err(|e| RpcError::internal(format!("node detection failed: {e}")))?;
    Ok((*info).clone())
}

async fn resources(ctx: RpcContext) -> Result<NodeResourcesResult, RpcError> {
    let monitor = Arc::clone(&ctx.services.nodes);
    let fresh = monitor.latest().filter(|s| chrono::Utc::now() - s.ts < STALE_AFTER);
    let sample = match fresh {
        Some(s) => s,
        None => tokio::task::spawn_blocking(move || monitor.sample())
            .await
            .map_err(|e| RpcError::internal(format!("resource sampling failed: {e}")))?,
    };
    let interval_ms = u64::try_from(ctx.services.node_sample_interval.as_millis()).unwrap_or(u64::MAX);
    Ok(NodeResourcesResult { sample, interval_ms })
}
