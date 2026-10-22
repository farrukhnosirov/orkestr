//! `nodes.*` handlers (local node, Phase 1; Phase 6 coordinator view,
//! `nodes.enroll|revoke|drain|setPolicy` — **owner: WP-A**).

use std::sync::Arc;

use orkestr_protocol::methods::nodes::{
    self as m, ListNodesResult, NodeInfo, NodeInfoParams, NodeResourcesResult, PowerStatusView,
};
use orkestr_protocol::RpcError;

use super::{stub, RpcContext, RpcRouter};

/// Samples older than this are refreshed on `nodes.resources`.
const STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::seconds(5);

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _| async move {
            Ok(ListNodesResult { nodes: vec![ctx.services.nodes.local_node()], snapshots: Vec::new() })
        })
        .method::<m::Info, _, _>(info)
        .method::<m::Resources, _, _>(|ctx: RpcContext, _| resources(ctx))
        .method::<m::Power, _, _>(|ctx: RpcContext, _| power(ctx));
    // Phase 6 (WP-A replaces the stubs)
    stub::<m::Enroll>(router);
    stub::<m::Revoke>(router);
    stub::<m::Drain>(router);
    stub::<m::SetPolicy>(router);
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

/// `nodes.power` (Phase 3, §31): power / battery state of this host.
/// Unsupported hosts answer `supported: false` instead of an error.
async fn power(ctx: RpcContext) -> Result<PowerStatusView, RpcError> {
    let platform = Arc::clone(&ctx.services.platform);
    let status = tokio::task::spawn_blocking(move || platform.power().status())
        .await
        .map_err(|e| RpcError::internal(format!("power probe failed: {e}")))?;
    Ok(match status {
        Ok(s) => PowerStatusView {
            on_ac_power: s.on_ac_power,
            battery_percent: s.battery_percent,
            has_battery: s.has_battery,
            supported: true,
        },
        Err(e) => {
            tracing::debug!(error = %e, "power status unavailable");
            PowerStatusView { on_ac_power: None, battery_percent: None, has_battery: false, supported: false }
        }
    })
}
