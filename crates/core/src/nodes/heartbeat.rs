//! Pull heartbeat loop: the coordinator calls `node.heartbeat` on every worker
//! (`ORKESTR_NODE_HEARTBEAT_MS`, default 5 s), records samples / leases and
//! drives the health state machine (Phase 6). **Owner: WP-A.**
//!
//! Per tick and enrolled, non-revoked node: get (or open + `node.hello`) the
//! link, collect lease renewals from the [`HeartbeatHook`]s, call
//! `node.heartbeat`, record the result. A worker that refuses the handshake
//! (it revoked this coordinator) turns the node `Revoked`. Then every node's
//! health is re-evaluated with [`orkestr_node::registry::next_health`]; changes
//! are published (`NodeStatus`, `NodeOffline`) and passed to the hooks.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{JobId, NodeId, NodeStatus};
use orkestr_node::link::call_typed;
use orkestr_node::registry::{next_health, NodeRegistry};
use orkestr_node::NodeError;
use orkestr_protocol::methods::nodes::{self as m, LeaseRenewal, NodeHeartbeatParams, NodeHeartbeatResult};
use tokio_util::sync::CancellationToken;

use crate::services::Services;

/// Coordinator-side observer of the heartbeat loop (WP-C: leases, fencing,
/// migration on `Offline` / `Revoked` / `Draining`).
#[async_trait]
pub trait HeartbeatHook: Send + Sync {
    /// Lease renewals and revocations sent to `node` in the next heartbeat.
    async fn lease_params(&self, node: NodeId) -> (Vec<LeaseRenewal>, Vec<JobId>) {
        let _ = node;
        (Vec::new(), Vec::new())
    }
    /// After a successful heartbeat round trip.
    async fn on_heartbeat(&self, node: NodeId, result: &NodeHeartbeatResult) {
        let _ = (node, result);
    }
    /// After a health change (incl. explicit drain / revoke).
    async fn on_health(&self, node: NodeId, from: NodeStatus, to: NodeStatus) {
        let _ = (node, from, to);
    }
}

/// Spawns the loop (stops on `shutdown`).
pub fn spawn(services: Arc<Services>, shutdown: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let every = services.node_hub.health().heartbeat;
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = tick.tick() => run_once(&services).await,
            }
        }
    })
}

/// One heartbeat round over every enrolled node + a health sweep (public for tests).
pub async fn run_once(services: &Arc<Services>) {
    let hub = &services.node_hub;
    let enrollments = match hub.registry().all_stored().await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "node registry unavailable");
            return;
        }
    };
    let beats = enrollments
        .into_iter()
        .filter(|s| s.enrollment.revoked_at.is_none())
        .map(|s| beat_one(services, s.enrollment.node_id));
    futures::future::join_all(beats).await;
    sweep(services).await;
}

async fn beat_one(services: &Arc<Services>, node: NodeId) {
    let hub = &services.node_hub;
    let result = async {
        let link = hub.link(node).await?;
        let (renew, revoke) = hub.lease_params(node).await;
        let params = NodeHeartbeatParams { seq: hub.next_seq(), renew, revoke };
        let timeout = (hub.health().heartbeat * 2).max(Duration::from_secs(5));
        tokio::time::timeout(timeout, call_typed::<m::Heartbeat>(link.as_ref(), &params))
            .await
            .map_err(|_| NodeError::Transport("heartbeat timed out".into()))?
    }
    .await;
    match result {
        Ok(hb) => {
            let before = hub.registry().get(node).await.ok().flatten().map(|s| s.status);
            match hub.registry().record_heartbeat(node, hb.clone(), chrono::Utc::now()).await {
                Ok(snap) => {
                    if let Some(from) = before.filter(|f| *f != snap.status) {
                        hub.health_changed(services, node, from, snap.status).await;
                    }
                }
                Err(e) => tracing::warn!(%node, error = %e, "cannot record heartbeat"),
            }
            hub.on_heartbeat(node, &hb).await;
        }
        Err(NodeError::Revoked(_)) => {
            tracing::warn!(%node, "worker refused the coordinator (revoked); marking the node revoked");
            hub.drop_link(node);
            if let Err(e) = hub.mark_revoked(services, node, "worker_refused").await {
                tracing::warn!(%node, error = %e, "cannot mark node revoked");
            }
        }
        Err(e) => {
            tracing::debug!(%node, error = %e, "heartbeat failed");
            hub.drop_link(node);
        }
    }
}

/// Re-evaluates every node's health from its heartbeat silence.
async fn sweep(services: &Arc<Services>) {
    let hub = &services.node_hub;
    let Ok(snaps) = hub.registry().list().await else { return };
    let now = chrono::Utc::now();
    let enrolled = hub.registry().all_stored().await.unwrap_or_default();
    for s in snaps {
        let since = s
            .last_heartbeat_at
            .or_else(|| enrolled.iter().find(|e| e.enrollment.node_id == s.node.id).map(|e| e.enrollment.enrolled_at));
        let silence = since.map_or(Duration::MAX, |t| (now - t).to_std().unwrap_or_default());
        let next = next_health(s.status, silence, &hub.health());
        if next != s.status {
            if let Err(e) = hub.registry().set_status(s.node.id, next).await {
                tracing::warn!(node = %s.node.id, error = %e, "cannot update node health");
                continue;
            }
            hub.health_changed(services, s.node.id, s.status, next).await;
        }
    }
}
