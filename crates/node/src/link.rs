//! Coordinator → worker links (Phase 6, ADR-0033). **Owner: WP-A.**
//!
//! A [`NodeLink`] is one coordinator's authenticated E2E session to one worker
//! core (gateway `/v1/connect`, coordinator signs with its P-256 CIK). The
//! coordinator *pulls*: it calls worker-side methods (`node.heartbeat`,
//! `jobs.offer`, `node.blobPut`, …) and receives worker events (`JobProgress`,
//! `JobCheckpointed`) on [`NodeLink::events`]. There is no worker → coordinator
//! RPC. `orkestr_core::nodes::loopback::LoopbackLink` wraps a worker router
//! in-process for tests.

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::NodeId;
use orkestr_protocol::methods::nodes::NodeEnrollment;
use orkestr_protocol::methods::Method;

use crate::NodeError;

/// Coordinator → one worker.
#[async_trait]
pub trait NodeLink: Send + Sync {
    /// Worker node id.
    fn node_id(&self) -> NodeId;
    /// Calls a worker-side method (untyped).
    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, NodeError>;
    /// Worker events (topic `jobs`) forwarded over the session.
    fn events(&self) -> tokio::sync::broadcast::Receiver<orkestr_protocol::Event>;
}

/// Opens links to enrolled workers.
#[async_trait]
pub trait NodeConnector: Send + Sync {
    /// Connects (gateway + E2E handshake) to the worker of `enr`.
    async fn connect(&self, enr: &NodeEnrollment) -> Result<Arc<dyn NodeLink>, NodeError>;
}

/// Typed call helper over any [`NodeLink`].
///
/// # Errors
/// Transport / RPC errors of the link; `Internal` when (de)serialization fails.
pub async fn call_typed<M: Method>(link: &dyn NodeLink, params: &M::Params) -> Result<M::Result, NodeError> {
    let value = serde_json::to_value(params).map_err(|e| NodeError::Internal(e.to_string()))?;
    let out = link.call(M::NAME, value).await?;
    serde_json::from_value(out).map_err(|e| NodeError::Internal(format!("bad result of {}: {e}", M::NAME)))
}
