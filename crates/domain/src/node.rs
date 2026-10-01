//! Compute nodes (§38–39).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{Arch, OsKind, Timestamp};
use crate::ids::NodeId;

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
