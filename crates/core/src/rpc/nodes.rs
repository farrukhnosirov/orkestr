//! `nodes.*` handlers (local node, Phase 1; Phase 6 coordinator view,
//! `nodes.enroll|revoke|drain|setPolicy` — **owner: WP-A**).

use std::sync::Arc;

use orkestr_domain::Actor;
use orkestr_protocol::methods::nodes::{
    self as m, DrainNodeParams, EnrollNodeParams, ListNodesResult, NodeIdParams, NodeInfo, NodeInfoParams,
    NodeResourcesResult, NodeSnapshotResult, PowerStatusView, SetNodePolicyParams,
};
use orkestr_protocol::RpcError;

use super::{Principal, RpcContext, RpcRouter};
use crate::nodes::rpc_error;

fn actor(ctx: &RpcContext) -> Actor {
    match &ctx.principal {
        Principal::Local => Actor::System,
        Principal::Remote(rp) => Actor::RemoteDevice(rp.device_id),
    }
}

/// Samples older than this are refreshed on `nodes.resources`.
const STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::seconds(5);

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _| async move {
            let snapshots = ctx.services.node_hub.snapshots().await.map_err(rpc_error)?;
            let nodes = snapshots.iter().map(|s| s.node.clone()).collect();
            Ok(ListNodesResult { nodes, snapshots })
        })
        .method::<m::Info, _, _>(info)
        .method::<m::Resources, _, _>(|ctx: RpcContext, _| resources(ctx))
        .method::<m::Power, _, _>(|ctx: RpcContext, _| power(ctx));
    // Phase 6: coordinator view
    router
        .method::<m::Enroll, _, _>(|ctx: RpcContext, p: EnrollNodeParams| async move {
            let a = actor(&ctx);
            ctx.services.node_hub.enroll(&ctx.services, a, p).await
        })
        .method::<m::Revoke, _, _>(|ctx: RpcContext, p: NodeIdParams| async move {
            let node = ctx.services.node_hub.mark_revoked(&ctx.services, p.node_id, "nodes.revoke").await?;
            Ok::<_, RpcError>(NodeSnapshotResult { node })
        })
        .method::<m::Drain, _, _>(|ctx: RpcContext, p: DrainNodeParams| async move {
            let node = ctx.services.node_hub.set_draining(&ctx.services, p.node_id, p.drain).await?;
            Ok::<_, RpcError>(NodeSnapshotResult { node })
        })
        .method::<m::SetPolicy, _, _>(|ctx: RpcContext, p: SetNodePolicyParams| async move {
            let node = ctx.services.node_hub.set_policy(p).await?;
            Ok::<_, RpcError>(NodeSnapshotResult { node })
        });
}

async fn info(ctx: RpcContext, p: NodeInfoParams) -> Result<NodeInfo, RpcError> {
    let monitor = Arc::clone(&ctx.services.nodes);
    let refresh = p.refresh.unwrap_or(false);
    let info = tokio::task::spawn_blocking(move || monitor.info(refresh))
        .await
        .map_err(|e| RpcError::internal(format!("node detection failed: {e}")))?;
    let mut info = (*info).clone();
    // models installed through the node model store (WP-D) next to Ollama's
    orkestr_node::models::merge_local_models(&mut info.local_models, &ctx.services.paths.data_dir);
    Ok(info)
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
