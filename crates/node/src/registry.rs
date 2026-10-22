//! Node registry + health state machine (Phase 6, node-protocol.md §3–4).
//! **Owner: WP-A.** The SQLite implementation is
//! `orkestr_core::nodes::registry_store::SqlNodeRegistry`.

use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{NodeId, NodeStatus, Timestamp};
use orkestr_protocol::methods::nodes::{NodeEnrollment, NodeHeartbeatResult, NodeSnapshot};

use crate::profile::{EnvSource, HEARTBEAT_ENV, LEASE_TTL_ENV};
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

impl HealthConfig {
    /// Derives the timings from a heartbeat interval: `Suspect` after 3 missed
    /// heartbeats, `Offline` after 6, lease TTL 6 heartbeats.
    #[must_use]
    pub fn from_heartbeat(heartbeat: Duration) -> Self {
        Self { heartbeat, suspect_after: heartbeat * 3, offline_after: heartbeat * 6, lease_ttl: heartbeat * 6 }
    }

    /// Reads `ORKESTR_NODE_HEARTBEAT_MS` / `ORKESTR_NODE_LEASE_TTL_MS`
    /// (defaults: 5 s heartbeat, 30 s lease). Invalid / zero values are ignored.
    #[must_use]
    pub fn from_env(env: &EnvSource) -> Self {
        let ms = |k: &str| env.get(k).and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v > 0);
        let mut cfg = ms(HEARTBEAT_ENV).map_or_else(Self::default, |hb| Self::from_heartbeat(Duration::from_millis(hb)));
        if let Some(ttl) = ms(LEASE_TTL_ENV) {
            cfg.lease_ttl = Duration::from_millis(ttl);
        }
        cfg
    }
}

/// Next health state from the current one and the time since the last
/// heartbeat (node-protocol.md §4):
///
/// * `Revoked` and `Draining` are sticky (changed only by explicit actions);
/// * silence ≥ `offline_after` → `Offline`;
/// * silence ≥ `suspect_after` → `Suspect` (an `Offline` node stays `Offline`);
/// * a fresh heartbeat brings `Suspect` / `Offline` back to `Online`;
///   `Online` / `Busy` / `Degraded` keep their value.
#[must_use]
pub fn next_health(current: NodeStatus, silence: Duration, cfg: &HealthConfig) -> NodeStatus {
    match current {
        NodeStatus::Revoked | NodeStatus::Draining => current,
        _ if silence >= cfg.offline_after => NodeStatus::Offline,
        NodeStatus::Offline if silence >= cfg.suspect_after => NodeStatus::Offline,
        _ if silence >= cfg.suspect_after => NodeStatus::Suspect,
        NodeStatus::Suspect | NodeStatus::Offline => NodeStatus::Online,
        other => other,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake clock: a heartbeat at `t=0`, then the state machine is evaluated
    /// at increasing instants.
    #[test]
    fn health_transitions_with_fake_clock() {
        let cfg = HealthConfig::from_heartbeat(Duration::from_millis(500));
        assert_eq!(cfg.suspect_after, Duration::from_millis(1500));
        assert_eq!(cfg.offline_after, Duration::from_secs(3));
        let mut status = NodeStatus::Online;
        let mut seen = Vec::new();
        for t_ms in (0..=4000).step_by(250) {
            let next = next_health(status, Duration::from_millis(t_ms), &cfg);
            if next != status {
                seen.push((t_ms, next));
            }
            status = next;
        }
        assert_eq!(seen, vec![(1500, NodeStatus::Suspect), (3000, NodeStatus::Offline)]);
        // a fresh heartbeat (silence 0) brings it back
        assert_eq!(next_health(NodeStatus::Offline, Duration::ZERO, &cfg), NodeStatus::Online);
        assert_eq!(next_health(NodeStatus::Suspect, Duration::from_millis(100), &cfg), NodeStatus::Online);
        // busy/degraded keep their value while fresh
        assert_eq!(next_health(NodeStatus::Busy, Duration::ZERO, &cfg), NodeStatus::Busy);
        assert_eq!(next_health(NodeStatus::Degraded, Duration::from_secs(2), &cfg), NodeStatus::Suspect);
    }

    #[test]
    fn revoked_and_draining_are_sticky() {
        let cfg = HealthConfig::default();
        for s in [NodeStatus::Revoked, NodeStatus::Draining] {
            assert_eq!(next_health(s, Duration::ZERO, &cfg), s);
            assert_eq!(next_health(s, Duration::from_secs(3600), &cfg), s);
        }
    }

    #[test]
    fn env_tuning() {
        let cfg = HealthConfig::from_env(&EnvSource::from_pairs([(HEARTBEAT_ENV, "500"), (LEASE_TTL_ENV, "3000")]));
        assert_eq!(cfg.heartbeat, Duration::from_millis(500));
        assert_eq!(cfg.lease_ttl, Duration::from_secs(3));
        assert_eq!(HealthConfig::from_env(&EnvSource::from_pairs([(HEARTBEAT_ENV, "x")])), HealthConfig::default());
        assert_eq!(HealthConfig::from_env(&EnvSource::from_pairs(Vec::<(String, String)>::new())), HealthConfig::default());
    }
}
