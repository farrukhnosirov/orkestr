//! Worker-side `node.hello` / `node.heartbeat` handlers (Phase 6, called by the
//! coordinator; policy: coordinator principals only). **Owner: WP-A.**
//!
//! Remote callers reach these only as a coordinator principal (`authz`
//! enforces `coordinatorOnly`); local calls (tests, `LoopbackLink` with
//! `Principal::Local`) are treated as a fully trusted coordinator.

use orkestr_domain::TrustLevel;
use orkestr_protocol::methods::nodes::{self as m, NodeHeartbeatParams, NodeHelloParams};
use orkestr_protocol::RpcError;

use super::{Principal, RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Hello, _, _>(|ctx: RpcContext, p: NodeHelloParams| async move {
            if p.protocol_version != crate::nodes::NODE_PROTOCOL_VERSION {
                return Err(RpcError::bad_request(format!(
                    "node protocol {} is not supported (this core speaks {})",
                    p.protocol_version,
                    crate::nodes::NODE_PROTOCOL_VERSION
                )));
            }
            let trust = match &ctx.principal {
                Principal::Remote(rp) => {
                    // the coordinator signs policy snapshots with the CIK it
                    // paired with: trust it for that coordinator node's offers
                    if rp.coordinator {
                        trust_coordinator_key(&ctx, rp.device_id, p.coordinator_node_id);
                    }
                    rp.trust_level.unwrap_or_default()
                }
                Principal::Local => TrustLevel::Trusted,
            };
            tracing::debug!(coordinator = %p.coordinator_node_id, ?trust, "node.hello");
            Ok::<_, RpcError>(ctx.services.node_hub.hello(trust).await)
        })
        .method::<m::Heartbeat, _, _>(|ctx: RpcContext, p: NodeHeartbeatParams| async move {
            let coordinator = match &ctx.principal {
                Principal::Remote(rp) => Some(rp.device_id),
                Principal::Local => None,
            };
            Ok::<_, RpcError>(ctx.services.node_hub.worker_heartbeat(coordinator, &p).await)
        });
}

fn trust_coordinator_key(ctx: &RpcContext, device: orkestr_domain::DeviceId, coordinator: orkestr_domain::NodeId) {
    let Some(record) = ctx.services.remote.device(device) else {
        tracing::warn!(%device, "node.hello from an unknown device");
        return;
    };
    match crate::remote::e2e::parse_verifying_b64(&record.public_key) {
        Ok(key) => {
            let host = crate::nodes::worker::host(&ctx.services);
            host.trust_coordinator(coordinator, key);
            host.bind_coordinator_device(device, coordinator);
        }
        Err(e) => tracing::warn!(%device, error = %e, "coordinator public key unusable"),
    }
}
