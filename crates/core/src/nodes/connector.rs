//! Gateway connector: coordinator → worker E2E links over `/v1/connect`
//! with a core bearer token (same account), signed with the coordinator CIK
//! (Phase 6, ADR-0033). **Owner: WP-A.**

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::NodeId;
use orkestr_node::{NodeConnector, NodeError, NodeLink};
use orkestr_protocol::methods::nodes::NodeEnrollment;
use orkestr_protocol::{topic, Event};
use tokio::sync::broadcast;

use super::registry_store::SqlNodeRegistry;
use crate::remote::client::{ClientError, E2eClient};
use crate::remote::{e2e, RemoteService};

/// Maps a client error of the link to `node_id` onto a [`NodeError`]: the
/// worker refusing the handshake because it revoked (or forgot) this
/// coordinator is [`NodeError::Revoked`].
#[must_use]
pub fn node_error(node_id: NodeId, e: ClientError) -> NodeError {
    match e {
        ClientError::Rpc(r) => NodeError::Rpc(r),
        ClientError::Refused(code) if code == "revoked" || code == "unknown_device" => NodeError::Revoked(node_id),
        other => NodeError::Transport(other.to_string()),
    }
}

/// One coordinator → worker E2E session.
#[derive(Debug)]
pub struct E2eLink {
    node_id: NodeId,
    client: E2eClient,
}

impl E2eLink {
    /// Wraps an authenticated client (already subscribed to topic `jobs`).
    #[must_use]
    pub fn new(node_id: NodeId, client: E2eClient) -> Self {
        Self { node_id, client }
    }

    /// Whether the underlying session closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }
}

#[async_trait]
impl NodeLink for E2eLink {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, NodeError> {
        self.client.call(method, params).await.map_err(|e| node_error(self.node_id, e))
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.client.events()
    }
}

/// [`NodeConnector`] over the gateway.
pub struct GatewayConnector {
    remote: Arc<RemoteService>,
    registry: Arc<SqlNodeRegistry>,
}

impl std::fmt::Debug for GatewayConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayConnector").finish_non_exhaustive()
    }
}

impl GatewayConnector {
    /// Connector using this core's gateway enrollment + CIK.
    #[must_use]
    pub fn new(remote: Arc<RemoteService>, registry: Arc<SqlNodeRegistry>) -> Self {
        Self { remote, registry }
    }

    /// Opens an authenticated E2E session to `core_id` as `device_id`,
    /// pinning `core_public_key` (SEC1), subscribed to topic `jobs`.
    ///
    /// # Errors
    /// Remote access not enrolled; gateway / handshake failure.
    pub async fn open(&self, core_id: &str, device_id: &str, core_public_key: &[u8]) -> Result<E2eClient, ClientError> {
        let (gateway, token) = self
            .remote
            .sync_credentials()
            .ok_or_else(|| ClientError::Transport("remote access is not enrolled at a gateway".into()))?;
        let cik = self.remote.cik().ok_or_else(|| ClientError::Transport("no core identity key".into()))?.clone();
        let client = E2eClient::connect(&gateway, &token, core_id, device_id, &cik, core_public_key).await?;
        client.subscribe(&[topic::JOBS]).await?;
        Ok(client)
    }
}

#[async_trait]
impl NodeConnector for GatewayConnector {
    async fn connect(&self, enr: &NodeEnrollment) -> Result<Arc<dyn NodeLink>, NodeError> {
        if enr.revoked_at.is_some() {
            return Err(NodeError::Revoked(enr.node_id));
        }
        let stored = self.registry.stored(enr.node_id).await?.ok_or(NodeError::NotFound(enr.node_id))?;
        let core_pub = e2e::unb64(&enr.public_key).map_err(|e| NodeError::Internal(format!("worker key: {e}")))?;
        let client =
            self.open(&enr.core_id, &stored.device_id, &core_pub).await.map_err(|e| node_error(enr.node_id, e))?;
        Ok(Arc::new(E2eLink::new(enr.node_id, client)))
    }
}
