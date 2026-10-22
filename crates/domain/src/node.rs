//! Compute nodes (§38–39).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{Arch, OsKind, Timestamp};
use crate::ids::{DeviceId, NodeId};

/// Availability of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NodeStatus {
    /// Healthy and accepting work.
    Online,
    /// Healthy but at capacity.
    Busy,
    /// Reachable but unhealthy (low disk, high load, provider issues).
    Degraded,
    /// Not reachable.
    Offline,
    /// Missed heartbeats (Phase 6): not offered new jobs, leases still valid.
    Suspect,
    /// Finishing / migrating its jobs; excluded from placement (Phase 6).
    Draining,
    /// Enrollment revoked; refused at the next heartbeat (Phase 6).
    Revoked,
}

/// Trust level of an enrolled node (node-protocol.md §3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TrustLevel {
    /// `Command` jobs without network/secrets.
    #[default]
    Restricted,
    /// + `AgentTask` jobs for allowed projects.
    Standard,
    /// + Deploy/Ssh/Production-scoped jobs (still subject to approvals).
    Trusted,
}

/// Who executes / owns a task (status authority for sync and scheduling, Phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ExecutorRef {
    /// A compute node (the coordinator itself or a worker).
    #[serde(rename_all = "camelCase")]
    Node {
        /// Node.
        node_id: NodeId,
    },
    /// A paired device.
    #[serde(rename_all = "camelCase")]
    Device {
        /// Device.
        device_id: DeviceId,
    },
}

/// Lease epoch (fencing token) of a job placement (node-protocol.md §6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LeaseEpoch(pub u64);

impl LeaseEpoch {
    /// The next epoch.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl std::fmt::Display for LeaseEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Content address of a blob: `sha256:<64 lowercase hex>` (ADR-0039).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Digest(pub String);

impl Digest {
    /// Prefix of every digest.
    pub const PREFIX: &'static str = "sha256:";

    /// Wraps a 32-byte SHA-256 hash.
    #[must_use]
    pub fn from_sha256(hash: &[u8; 32]) -> Self {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(71);
        s.push_str(Self::PREFIX);
        for b in hash {
            let _ = write!(s, "{b:02x}");
        }
        Self(s)
    }

    /// Validates the `sha256:<64 hex>` shape.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.0
            .strip_prefix(Self::PREFIX)
            .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    }

    /// The hex part (empty when malformed).
    #[must_use]
    pub fn hex(&self) -> &str {
        self.0.strip_prefix(Self::PREFIX).unwrap_or("")
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A GPU on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GpuInfo {
    /// Marketing name.
    pub name: String,
    /// VRAM in bytes, if known.
    pub vram_bytes: Option<u64>,
}

/// Static + slowly changing resource description of a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeResources {
    /// Logical CPU cores.
    pub cpu_cores: u32,
    /// Current CPU usage 0..100.
    pub cpu_usage_percent: f32,
    /// Total RAM in bytes.
    pub memory_total_bytes: u64,
    /// Used RAM in bytes.
    pub memory_used_bytes: u64,
    /// Total disk bytes of the data volume.
    pub disk_total_bytes: u64,
    /// Free disk bytes of the data volume.
    pub disk_free_bytes: u64,
    /// GPUs.
    pub gpus: Vec<GpuInfo>,
}

/// A machine that can run agents (§38). Every desktop core is a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Node {
    /// Id.
    pub id: NodeId,
    /// Display name.
    pub name: String,
    /// Host name.
    pub hostname: String,
    /// OS family.
    pub os: OsKind,
    /// OS version string.
    pub os_version: String,
    /// CPU architecture.
    pub arch: Arch,
    /// Orkestr core version running on the node.
    pub core_version: String,
    /// Status.
    pub status: NodeStatus,
    /// Resources.
    pub resources: NodeResources,
    /// Detected tools/runtimes (git, docker, node, cargo, …).
    pub tools: Vec<String>,
    /// Locally available models (e.g. Ollama tags).
    pub local_models: Vec<String>,
    /// True for the node this core runs on.
    pub is_local: bool,
    /// Last heartbeat.
    pub last_heartbeat_at: Option<Timestamp>,
}
