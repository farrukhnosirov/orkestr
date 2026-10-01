//! `system.*` — health and information about the running core.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{Arch, NodeId, OsKind, Timestamp};

use super::{method, Empty};

/// Result of `system.health` (same data as `GET /health`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthResult {
    /// Always `"ok"` when the core answers.
    pub status: String,
    /// Core version.
    pub version: String,
    /// Uptime in milliseconds.
    pub uptime_ms: u64,
}

/// Result of `system.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemInfo {
    /// Core version.
    pub version: String,
    /// Wire protocol version.
    pub protocol_version: u32,
    /// Id of the local node.
    pub node_id: NodeId,
    /// OS family.
    pub os: OsKind,
    /// CPU architecture.
    pub arch: Arch,
    /// Host name.
    pub hostname: String,
    /// Core process id.
    pub pid: u32,
    /// Data directory.
    pub data_dir: String,
    /// Start time.
    pub started_at: Timestamp,
    /// Whether the previous session ended uncleanly (crash recovery ran).
    pub recovered_from_crash: bool,
    /// Detected shells (display names), for the terminal panel.
    pub shells: Vec<String>,
}

method!(
    /// `system.health`.
    Health, "system.health", Empty, HealthResult
);
method!(
    /// `system.info`.
    Info, "system.info", Empty, SystemInfo
);
