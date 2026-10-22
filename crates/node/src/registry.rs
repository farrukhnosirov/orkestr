//! Node registry + health state machine (Phase 6, node-protocol.md §3–4).
//! **Owner: WP-A.** The SQLite implementation is
//! `orkestr_core::nodes::registry_store::SqlNodeRegistry`.

use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{NodeId, NodeStatus, Timestamp};
use orkestr_protocol::methods::nodes::{NodeEnrollment, NodeHeartbeatResult, NodeSnapshot};

use crate::NodeError;

/// Health timing (env: `ORKESTR_NODE_HEARTBEAT_MS`, `ORKESTR_NODE_LEASE_TTL_MS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthConfig {
    /// Heartbeat interval (default 5 s).
    pub heartbeat: Duration,
    /// Silence after which a node is `Suspect` (default 3 heartbeats).
    pub suspect_after: Duration,
    /// Silence after which a node is `Offline` (default 30 s).
    pub offline_after: Duration,
    /// Lease TTL (default 30 s).
    pub lease_ttl: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(5),
            suspect_after: Duration::from_secs(15),
            offline_after: Duration::from_secs(30),
            lease_ttl: Duration::from_secs(30),
        }
    }
}

/// Next health state from the current one and the time since the last heartbeat.
/// `Revoked` and `Draining` are sticky (changed only by explicit actions).
#[must_use]
pub fn next_health(current: NodeStatus, silence: Duration, cfg: &HealthConfig) -> NodeStatus {
    // Step 0 skeleton: WP-A implements the full state machine (node-protocol.md §4).
    let _ = (silence, cfg);
    current
}

/// Registry of known nodes (coordinator view).
#[async_trait]
pub trait NodeRegistry: Send + Sync {
    /// All nodes (the local/coordinator node first).
    async fn list(&self) -> Result<Vec<NodeSnapshot>, NodeError>;
    /// One node.
    async fn get(&self, id: NodeId) -> Result<Option<NodeSnapshot>, NodeError>;
    /// Enrollment of a node.
    async fn enrollment(&self, id: NodeId) -> Result<Option<NodeEnrollment>, NodeError>;
    /// All enrollments (incl. revoked).
    async fn enrollments(&self) -> Result<Vec<NodeEnrollment>, NodeError>;
    /// Stores / replaces an enrollment.
    async fn upsert_enrollment(&self, enrollment: NodeEnrollment) -> Result<(), NodeError>;
    /// Records a heartbeat result (→ `Online` unless sticky).
    async fn record_heartbeat(
        &self,
        id: NodeId,
        hb: NodeHeartbeatResult,
        at: Timestamp,
    ) -> Result<NodeSnapshot, NodeError>;
    /// Sets the health status explicitly (drain, revoke, offline).
    async fn set_status(&self, id: NodeId, status: NodeStatus) -> Result<NodeSnapshot, NodeError>;
}
