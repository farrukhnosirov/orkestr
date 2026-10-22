//! `nodes.*` — nodes and their resources (§27, §38).
//!
//! Phase 1 knows only the local node: `nodes.list` returns it, `nodes.info`
//! returns its hardware/runtime inventory (cached, `refresh` re-detects) and
//! `nodes.resources` the latest live sample. The core samples resources every
//! 2 s and streams them as ephemeral `NodeStatus` events on `node:<id>`
//! (`NodeStatusPayload::sample`). Phase 6 adds remote nodes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use std::collections::BTreeMap;

use orkestr_domain::{
    Arch, CheckpointId, Digest, JobId, LeaseEpoch, Node, NodeId, NodeStatus, OsKind, Timestamp, TrustLevel,
};

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
    /// Coordinator view of every node incl. health, trust, slots and leases
    /// (Phase 6; the local node first). Empty from pre-Phase-6 cores.
    #[serde(default)]
    pub snapshots: Vec<NodeSnapshot>,
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

// ---------------------------------------------------------------------------
// Phase 6: multi-node (node-protocol.md, ADR-0033)
// ---------------------------------------------------------------------------

/// Job slots of a node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeSlots {
    /// Maximum concurrent jobs.
    pub max: u32,
    /// Jobs running now.
    pub used: u32,
}

/// Phase of a job lease as reported by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LeasePhase {
    /// Accepted, materializing inputs.
    Starting,
    /// Executing.
    Running,
    /// Running to the next safe point for a checkpoint (drain).
    Draining,
    /// Finished (result pending pickup).
    Finished,
    /// Failed on the worker.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// One job lease held by a node (heartbeat payload, `NodeSnapshot.leases`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LeaseState {
    /// Job.
    pub job_id: JobId,
    /// Fencing epoch.
    pub epoch: LeaseEpoch,
    /// Phase.
    pub phase: LeasePhase,
    /// Progress 0..=1 when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<f32>,
    /// Latest checkpoint, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_checkpoint: Option<CheckpointId>,
    /// Lease valid until (coordinator clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lease_until: Option<Timestamp>,
}

/// Simulated / overridden node profile (`ORKESTR_NODE_PROFILE` JSON, Phase 6).
/// Every field is optional: unset fields keep the detected value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct NodeProfile {
    /// Display name.
    #[ts(optional)]
    pub name: Option<String>,
    /// OS family.
    #[ts(optional)]
    pub os: Option<OsKind>,
    /// Architecture.
    #[ts(optional)]
    pub arch: Option<Arch>,
    /// Logical CPU cores.
    #[ts(optional)]
    pub cpu_cores: Option<u32>,
    /// Total memory bytes.
    #[ts(optional)]
    pub memory_total_bytes: Option<u64>,
    /// GPUs.
    #[ts(optional)]
    pub gpus: Option<Vec<GpuDevice>>,
    /// Tools on PATH (replaces detection).
    #[ts(optional)]
    pub tools: Option<Vec<String>>,
    /// Free labels.
    #[ts(optional)]
    pub labels: Option<BTreeMap<String, String>>,
    /// Local models (replaces detection).
    #[ts(optional)]
    pub models: Option<Vec<String>>,
    /// Tool shims: tool name → command line run instead (`xcodebuild` →
    /// `node scripts/smoke/fixtures/fake-xcodebuild.mjs`).
    #[ts(optional)]
    pub tool_shims: Option<BTreeMap<String, String>>,
    /// Maximum concurrent jobs.
    #[ts(optional)]
    pub max_concurrent_jobs: Option<u32>,
}

/// Signed enrollment record of a worker, stored on the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeEnrollment {
    /// Node id (the worker core's node id).
    pub node_id: NodeId,
    /// Worker core id at the gateway (`/v1/connect` target).
    pub core_id: String,
    /// Display name.
    pub name: String,
    /// Worker core identity key (P-256 CIK, SPKI base64url) pinned at pairing.
    pub public_key: String,
    /// Trust level granted on the worker.
    pub trust_level: TrustLevel,
    /// Allowed projects (`[]` = none, `["*"]` = all).
    pub allowed_projects: Vec<String>,
    /// When enrolled.
    pub enrolled_at: Timestamp,
    /// When revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub revoked_at: Option<Timestamp>,
    /// Coordinator signature (CIK, base64url) over the canonical record without `sig`.
    pub sig: String,
}

/// Coordinator view of one node (`nodes.list`, scheduler input).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeSnapshot {
    /// The node (descriptor + latest resources).
    pub node: Node,
    /// Health (Online/Suspect/Offline/Draining/Revoked …).
    pub status: NodeStatus,
    /// Trust level (the local node is `trusted`).
    pub trust_level: TrustLevel,
    /// Allowed projects (`["*"]` = all).
    pub allowed_projects: Vec<String>,
    /// Job slots.
    pub slots: NodeSlots,
    /// Leases held.
    pub leases: Vec<LeaseState>,
    /// Last heartbeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_heartbeat_at: Option<Timestamp>,
    /// True for the coordinator itself.
    pub is_coordinator: bool,
    /// Free labels (profile).
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Digests of locally installed model manifests (model store).
    #[serde(default)]
    pub model_digests: Vec<Digest>,
}

/// Params of `nodes.enroll` (coordinator side; the worker created a `kind: "node"` pairing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnrollNodeParams {
    /// Worker core id at the gateway.
    pub core_id: String,
    /// Pairing id shown on the worker.
    pub pairing_id: String,
    /// One-time pairing code shown on the worker.
    pub code: String,
    /// Display name override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
}

/// Result of `nodes.enroll`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnrollNodeResult {
    /// The new enrollment (trust level as granted on the worker).
    pub enrollment: NodeEnrollment,
    /// Short authentication string to compare with the worker's screen.
    pub sas: String,
}

/// Params of `nodes.revoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeIdParams {
    /// Node.
    pub node_id: NodeId,
}

/// Params of `nodes.drain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DrainNodeParams {
    /// Node.
    pub node_id: NodeId,
    /// `true` drains (migrates jobs away, no new placements); `false` returns it to Online.
    pub drain: bool,
}

/// Params of `nodes.setPolicy` (unset fields unchanged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetNodePolicyParams {
    /// Node.
    pub node_id: NodeId,
    /// New trust level (never above what the worker granted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trust_level: Option<TrustLevel>,
    /// Allowed projects (`["*"]` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub allowed_projects: Option<Vec<String>>,
    /// Maximum concurrent jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_concurrent_jobs: Option<u32>,
}

/// Result of node mutations (`nodes.revoke`, `nodes.drain`, `nodes.setPolicy`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeSnapshotResult {
    /// The node after the change.
    pub node: NodeSnapshot,
}

/// Params of worker-side `node.hello` (coordinator → worker after connecting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeHelloParams {
    /// Coordinator node id.
    pub coordinator_node_id: NodeId,
    /// Node protocol version (1).
    pub protocol_version: u32,
}

/// Result of `node.hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeHelloResult {
    /// Worker node descriptor (profile overrides applied).
    pub node: Node,
    /// Trust level the worker granted the coordinator pairing.
    pub trust_level: TrustLevel,
    /// Job slots.
    pub slots: NodeSlots,
    /// Labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Node protocol version (1).
    pub protocol_version: u32,
    /// Digests of locally installed model manifests (model store, Phase 6).
    #[serde(default)]
    pub model_digests: Vec<Digest>,
}

/// Lease renewal carried by a heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LeaseRenewal {
    /// Job.
    pub job_id: JobId,
    /// Current epoch on the coordinator.
    pub epoch: LeaseEpoch,
    /// New lease end.
    pub lease_until: Timestamp,
}

/// Params of worker-side `node.heartbeat` (pull model: the coordinator polls every 5 s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeHeartbeatParams {
    /// Sequence number.
    pub seq: u64,
    /// Leases renewed in this round trip.
    #[serde(default)]
    pub renew: Vec<LeaseRenewal>,
    /// Jobs the coordinator no longer recognises (stale epoch / reassigned / revoked).
    #[serde(default)]
    pub revoke: Vec<JobId>,
}

/// Result of `node.heartbeat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeHeartbeatResult {
    /// Worker node with the latest resources.
    pub node: Node,
    /// Latest detailed sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sample: Option<ResourceSample>,
    /// Slots.
    pub slots: NodeSlots,
    /// Leases held by the worker.
    pub leases: Vec<LeaseState>,
    /// Digests of locally installed model manifests (model store, Phase 6).
    #[serde(default)]
    pub model_digests: Vec<Digest>,
}

/// `NodeRegistered` event payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeRegisteredPayload {
    /// The enrolled node.
    pub node: Box<NodeSnapshot>,
}

method!(
    /// `nodes.enroll` — enroll a worker core via its `kind: "node"` pairing (Phase 6).
    Enroll, "nodes.enroll", EnrollNodeParams, EnrollNodeResult
);
method!(
    /// `nodes.revoke` — revoke a node's enrollment; its jobs are cancelled (Phase 6).
    Revoke, "nodes.revoke", NodeIdParams, NodeSnapshotResult
);
method!(
    /// `nodes.drain` — stop placing jobs on a node and migrate its jobs away (Phase 6).
    Drain, "nodes.drain", DrainNodeParams, NodeSnapshotResult
);
method!(
    /// `nodes.setPolicy` — trust level / allowed projects / slots (Phase 6).
    SetPolicy, "nodes.setPolicy", SetNodePolicyParams, NodeSnapshotResult
);
method!(
    /// Worker side `node.hello` (called by the coordinator after connecting).
    Hello, "node.hello", NodeHelloParams, NodeHelloResult
);
method!(
    /// Worker side `node.heartbeat` (pull: sample + slots + leases; renews leases).
    Heartbeat, "node.heartbeat", NodeHeartbeatParams, NodeHeartbeatResult
);
