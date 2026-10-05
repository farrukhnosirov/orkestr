//! `nodes.*` — nodes and their resources (§27, §38).
//!
//! Phase 1 knows only the local node: `nodes.list` returns it, `nodes.info`
//! returns its hardware/runtime inventory (cached, `refresh` re-detects) and
//! `nodes.resources` the latest live sample. The core samples resources every
//! 2 s and streams them as ephemeral `NodeStatus` events on `node:<id>`
//! (`NodeStatusPayload::sample`). Phase 6 adds remote nodes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{Arch, Node, NodeId, OsKind, Timestamp};

use super::method;

/// CPU description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CpuInfo {
    /// Brand string ("AMD Ryzen 9 7950X 16-Core Processor").
    pub model: String,
    /// Vendor id ("AuthenticAMD", "GenuineIntel", "Apple").
    pub vendor: String,
    /// Logical cores.
    pub logical_cores: u32,
    /// Physical cores, when known.
    pub physical_cores: Option<u32>,
    /// Base frequency in MHz (0 when unknown).
    pub frequency_mhz: u64,
}

/// A graphics adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GpuDevice {
    /// Marketing name.
    pub name: String,
    /// Vendor when recognisable.
    pub vendor: Option<String>,
    /// Dedicated memory in bytes, when known.
    pub vram_bytes: Option<u64>,
    /// Driver version, when known.
    pub driver_version: Option<String>,
    /// Detection source (`nvidia-smi`, `wmi`, `system_profiler`, `lspci`).
    pub source: String,
}

/// A mounted volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiskInfo {
    /// Device / volume name.
    pub name: String,
    /// Mount point (`C:\`, `/`).
    pub mount_point: String,
    /// File system (`NTFS`, `apfs`, `ext4`).
    pub file_system: String,
    /// `ssd`, `hdd` or `unknown`.
    pub kind: String,
    /// Removable media.
    pub removable: bool,
    /// Total bytes.
    pub total_bytes: u64,
    /// Available bytes.
    pub available_bytes: u64,
    /// True for the volume holding the Orkestr data directory.
    pub is_data_volume: bool,
}

/// A developer runtime / tool found (or not) on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeInfo {
    /// Stable id (`git`, `node`, `npm`, `python`, `rustc`, `cargo`, `go`, `dotnet`, `java`, `docker`, `wsl`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Found and answered its version probe.
    pub available: bool,
    /// Parsed version ("2.45.1"), when available.
    pub version: Option<String>,
    /// Executable path, when found.
    pub path: Option<String>,
    /// Why it is unavailable (not found, timeout, error output).
    pub detail: Option<String>,
}

/// A locally available model (Ollama).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalModel {
    /// Model tag (`llama3.1:8b`).
    pub name: String,
    /// Size on disk in bytes.
    pub size_bytes: Option<u64>,
    /// Model family (`llama`).
    pub family: Option<String>,
    /// Parameter size (`8.0B`).
    pub parameter_size: Option<String>,
    /// Quantization (`Q4_K_M`).
    pub quantization: Option<String>,
}

/// Local model servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalModelsInfo {
    /// Ollama endpoint probed (`127.0.0.1:11434`).
    pub ollama_endpoint: String,
    /// Whether Ollama answered `/api/tags`.
    pub ollama_reachable: bool,
    /// Models reported by Ollama.
    pub models: Vec<LocalModel>,
}

/// Hardware and runtime inventory of a node (result of `nodes.info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeInfo {
    /// Node id.
    pub node_id: NodeId,
    /// Host name.
    pub hostname: String,
    /// OS family.
    pub os: OsKind,
    /// OS version string.
    pub os_version: String,
    /// Kernel version, when known.
    pub kernel_version: Option<String>,
    /// CPU architecture.
    pub arch: Arch,
    /// Core version.
    pub core_version: String,
    /// CPU.
    pub cpu: CpuInfo,
    /// Total RAM bytes.
    pub memory_total_bytes: u64,
    /// Total swap / page file bytes.
    pub swap_total_bytes: u64,
    /// GPUs.
    pub gpus: Vec<GpuDevice>,
    /// Volumes.
    pub disks: Vec<DiskInfo>,
    /// Developer runtimes.
    pub runtimes: Vec<RuntimeInfo>,
    /// Local models.
    pub local_models: LocalModelsInfo,
    /// When the inventory was taken.
    pub detected_at: Timestamp,
    /// How long detection took.
    pub detection_ms: u64,
}

/// CPU/RAM usage of one process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessUsage {
    /// Pid.
    pub pid: u32,
    /// Process name.
    pub name: String,
    /// CPU usage relative to the whole machine (0..=100).
    pub cpu_percent: f32,
    /// CPU usage in cores (1.0 = one core fully busy).
    pub cpu_cores: f32,
    /// Resident memory bytes.
    pub memory_bytes: u64,
}

/// A live resource sample (result of `nodes.resources`, payload of `NodeStatus.sample`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResourceSample {
    /// Node.
    pub node_id: NodeId,
    /// Sample time.
    pub ts: Timestamp,
    /// Global CPU usage 0..=100.
    pub cpu_percent: f32,
    /// Usage per logical core 0..=100.
    pub per_core_percent: Vec<f32>,
    /// Total RAM bytes.
    pub memory_total_bytes: u64,
    /// Used RAM bytes.
    pub memory_used_bytes: u64,
    /// Total swap bytes.
    pub swap_total_bytes: u64,
    /// Used swap bytes.
    pub swap_used_bytes: u64,
    /// Total bytes of the data volume.
    pub disk_total_bytes: u64,
    /// Free bytes of the data volume.
    pub disk_free_bytes: u64,
    /// The Orkestr core process itself.
    pub core_process: Option<ProcessUsage>,
    /// Busiest processes by CPU (at most 5).
    pub top_processes: Vec<ProcessUsage>,
}

/// Result of `nodes.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListNodesResult {
    /// Known nodes (Phase 1: the local node only).
    pub nodes: Vec<Node>,
}

/// Params of `nodes.info`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeInfoParams {
    /// Re-detect instead of returning the cached inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `nodes.resources`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeResourcesResult {
    /// Latest sample.
    pub sample: ResourceSample,
    /// Sampling interval of the streamed `NodeStatus` events, in ms.
    pub interval_ms: u64,
}

method!(
    /// `nodes.list`.
    List, "nodes.list", super::Empty, ListNodesResult
);
method!(
    /// `nodes.info`.
    Info, "nodes.info", NodeInfoParams, NodeInfo
);
method!(
    /// `nodes.resources`.
    Resources, "nodes.resources", super::Empty, NodeResourcesResult
);

/// Host power state (result of `nodes.power`; §31 Performance Mode — the UI
/// switches to Battery Saver on battery when the user allows it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PowerStatusView {
    /// `true` on AC power, `false` on battery, `None` unknown.
    pub on_ac_power: Option<bool>,
    /// Battery charge 0..=100, when known.
    pub battery_percent: Option<f32>,
    /// Whether a battery is present.
    pub has_battery: bool,
    /// Whether this host can report its power state at all.
    pub supported: bool,
}

method!(
    /// `nodes.power` — power / battery state of the local node (Phase 3).
    Power, "nodes.power", super::Empty, PowerStatusView
);
