//! Phase 7 ecosystem wiring (ADR-0034 / 0040 / 0041 / 0042): connects the
//! plugin host, infrastructure monitor and Bug Detective to the rest of the
//! core once `Services` and the RPC router exist.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use orkestr_domain::{DeploymentId, Node};
use orkestr_monitor::{DeploymentLogFeed, DeploymentRef, NodeMetricsSource};
use orkestr_protocol::methods::deploy::DeployLogsResult;

use crate::rpc::RpcRouter;
use crate::services::Services;

/// Remote node metrics for the monitor: the node hub's snapshots.
struct HubNodes(Weak<Services>);

#[async_trait]
impl NodeMetricsSource for HubNodes {
    async fn nodes(&self) -> Vec<Node> {
        let Some(s) = self.0.upgrade() else { return Vec::new() };
        match s.node_hub.snapshots().await {
            Ok(snaps) => snaps.into_iter().map(|n| n.node).collect(),
            Err(e) => {
                tracing::debug!(error = %e, "node snapshots unavailable for the monitor");
                Vec::new()
            }
        }
    }
}

/// Deployment logs for the monitor (`deploy:<id>` log sources).
struct DeployFeed(Weak<Services>);

#[async_trait]
impl DeploymentLogFeed for DeployFeed {
    async fn recent(&self, limit: usize) -> Vec<DeploymentRef> {
        let Some(s) = self.0.upgrade() else { return Vec::new() };
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        match s.deploy.recent(limit).await {
            Ok(refs) => refs
                .into_iter()
                .map(|r| DeploymentRef { deployment_id: r.deployment_id, project_id: Some(r.project_id), name: r.name })
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "recent deployments unavailable for the monitor");
                Vec::new()
            }
        }
    }

    async fn read(&self, deployment_id: DeploymentId, offset: u64) -> Result<DeployLogsResult, String> {
        let s = self.0.upgrade().ok_or_else(|| "core is shutting down".to_owned())?;
        s.deploy.logs(deployment_id, offset).await.map_err(|e| e.to_string())
    }
}

/// Wires the ecosystem services (call once at boot, after the router exists).
pub async fn wire(services: &Arc<Services>, router: Arc<RpcRouter>) {
    // Plugins: agent tools, RPC reader for host calls, persisted state.
    crate::plugin_tools::wire(services, router).await;
    // Infrastructure monitor: remote nodes + deployment logs, then loops.
    services.monitor.set_node_source(Arc::new(HubNodes(Arc::downgrade(services))));
    services.monitor.set_deployment_logs(Arc::new(DeployFeed(Arc::downgrade(services))));
    if let Err(e) = services.monitor.start().await {
        tracing::error!(error = %e, "infrastructure monitor could not start");
    }
    // Bug Detective: ports over providers, agents, quality, reviews, VCS.
    crate::rpc::detective::install(services).await;
}
