//! # orkestr-node
//!
//! Nodes (MASTER_SPEC §27, §38–39). Phase 1 implements the **local** node:
//!
//! * [`inventory::detect_node_info`] — OS, CPU model/cores, RAM, GPUs
//!   (`nvidia-smi` / WMI / `system_profiler` / `lspci` through
//!   `orkestr-platform`), disks, developer runtimes with versions
//!   ([`runtimes`]) and local Ollama models ([`ollama`]);
//! * [`sampler::ResourceSampler`] — live CPU (global + per core), RAM, swap,
//!   data volume, the core process and the busiest processes;
//! * [`NodeMonitor`] — caches the inventory and the latest sample; the core
//!   drives it every 2 s and streams `NodeStatus` events.
//!
//! The [`NodeRegistry`] interface is fixed for Phase 6 (remote nodes,
//! heartbeats, scheduling by requirements, task migration).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod inventory;
pub mod ollama;
pub mod runtimes;
pub mod sampler;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{GpuInfo, Node, NodeId, NodeResources, NodeStatus, Timestamp};
use orkestr_platform::PlatformAdapter;
use orkestr_protocol::methods::nodes::{NodeInfo, ResourceSample};
use parking_lot::{Mutex, RwLock};

pub use inventory::{detect_node_info, DetectOptions};
pub use sampler::ResourceSampler;

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Unknown node.
    #[error("node {0} not found")]
    NotFound(NodeId),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Registry of known nodes.
#[async_trait]
pub trait NodeRegistry: Send + Sync {
    /// All nodes.
    async fn list(&self) -> Result<Vec<Node>, NodeError>;
    /// Records a heartbeat.
    async fn heartbeat(&self, node: Node) -> Result<(), NodeError>;
}

/// Tools probed on PATH to describe a node quickly (without running them).
pub const PROBED_TOOLS: &[&str] = &["git", "node", "npm", "cargo", "docker", "python", "ollama", "claude", "codex"];

/// Describes the local node from platform data (cheap: no external commands).
#[must_use]
pub fn describe_local_node(id: NodeId, platform: &dyn PlatformAdapter, core_version: &str, now: Timestamp) -> Node {
    let info = platform.info();
    let r = platform.resources().snapshot();
    let tools =
        PROBED_TOOLS.iter().filter(|t| platform.process().which(t).is_some()).map(|t| (*t).to_owned()).collect();
    Node {
        id,
        name: info.hostname.clone(),
        hostname: info.hostname.clone(),
        os: info.os,
        os_version: info.os_version.clone(),
        arch: info.arch,
        core_version: core_version.to_owned(),
        status: NodeStatus::Online,
        resources: NodeResources {
            cpu_cores: r.cpu_cores,
            cpu_usage_percent: r.cpu_usage_percent,
            memory_total_bytes: r.memory_total_bytes,
            memory_used_bytes: r.memory_used_bytes,
            disk_total_bytes: r.disk_total_bytes,
            disk_free_bytes: r.disk_free_bytes,
            gpus: Vec::new(),
        },
        tools,
        local_models: Vec::new(),
        is_local: true,
        last_heartbeat_at: Some(now),
    }
}

/// Builds the domain [`Node`] from the cached inventory and a live sample.
#[must_use]
pub fn node_from(info: Option<&NodeInfo>, base: &Node, sample: &ResourceSample) -> Node {
    let mut node = base.clone();
    node.status = NodeStatus::Online;
    node.last_heartbeat_at = Some(sample.ts);
    node.resources.cpu_cores = u32::try_from(sample.per_core_percent.len()).unwrap_or(u32::MAX).max(1);
    node.resources.cpu_usage_percent = sample.cpu_percent;
    node.resources.memory_total_bytes = sample.memory_total_bytes;
    node.resources.memory_used_bytes = sample.memory_used_bytes;
    node.resources.disk_total_bytes = sample.disk_total_bytes;
    node.resources.disk_free_bytes = sample.disk_free_bytes;
    if let Some(info) = info {
        node.resources.gpus =
            info.gpus.iter().map(|g| GpuInfo { name: g.name.clone(), vram_bytes: g.vram_bytes }).collect();
        node.tools = info.runtimes.iter().filter(|r| r.available).map(|r| r.id.clone()).collect();
        node.local_models = info.local_models.models.iter().map(|m| m.name.clone()).collect();
    }
    node
}

/// The local node: cached inventory + live sampler.
pub struct NodeMonitor {
    node_id: NodeId,
    core_version: String,
    platform: Arc<dyn PlatformAdapter>,
    options: DetectOptions,
    base: Node,
    info: RwLock<Option<Arc<NodeInfo>>>,
    /// Serializes detections so concurrent `nodes.info` calls share one run.
    detecting: Mutex<()>,
    /// Incremented after every finished detection.
    generation: AtomicU64,
    sampler: Mutex<ResourceSampler>,
    latest: RwLock<Option<ResourceSample>>,
}

impl std::fmt::Debug for NodeMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeMonitor").field("node_id", &self.node_id).finish_non_exhaustive()
    }
}

impl NodeMonitor {
    /// Creates the monitor (cheap; no external commands run yet).
    #[must_use]
    pub fn new(
        node_id: NodeId,
        platform: Arc<dyn PlatformAdapter>,
        core_version: &str,
        options: DetectOptions,
    ) -> Self {
        let base = describe_local_node(node_id, platform.as_ref(), core_version, orkestr_domain::now());
        let sampler = ResourceSampler::new(node_id, platform.paths().data_dir.clone());
        Self {
            node_id,
            core_version: core_version.to_owned(),
            platform,
            options,
            base,
            info: RwLock::new(None),
            detecting: Mutex::new(()),
            generation: AtomicU64::new(0),
            sampler: Mutex::new(sampler),
            latest: RwLock::new(None),
        }
    }

    /// Local node id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Cached inventory, if detected.
    #[must_use]
    pub fn cached_info(&self) -> Option<Arc<NodeInfo>> {
        self.info.read().clone()
    }

    /// Returns the inventory, detecting it when missing or when `refresh`.
    /// Blocking (seconds on a cold call) — run on a blocking thread.
    pub fn info(&self, refresh: bool) -> Arc<NodeInfo> {
        if !refresh {
            if let Some(i) = self.cached_info() {
                return i;
            }
        }
        let generation_before = self.generation.load(Ordering::Acquire);
        let _guard = self.detecting.lock();
        // Another caller finished a detection while we waited: share it.
        if self.generation.load(Ordering::Acquire) != generation_before {
            if let Some(i) = self.cached_info() {
                return i;
            }
        }
        let info = Arc::new(detect_node_info(self.node_id, self.platform.as_ref(), &self.core_version, &self.options));
        *self.info.write() = Some(Arc::clone(&info));
        self.generation.fetch_add(1, Ordering::AcqRel);
        info
    }

    /// Takes a live sample and caches it. Blocking (tens of ms).
    pub fn sample(&self) -> ResourceSample {
        let sample = self.sampler.lock().sample();
        *self.latest.write() = Some(sample.clone());
        sample
    }

    /// Latest cached sample.
    #[must_use]
    pub fn latest(&self) -> Option<ResourceSample> {
        self.latest.read().clone()
    }

    /// The domain node built from the latest sample (or the static description).
    #[must_use]
    pub fn local_node(&self) -> Node {
        match self.latest() {
            Some(s) => node_from(self.cached_info().as_deref(), &self.base, &s),
            None => self.base.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::{NativePlatform, Paths};

    #[test]
    fn local_node_description() {
        let dir = tempfile::tempdir().unwrap();
        let p = NativePlatform::new(Paths::from_root(dir.path()));
        let n = describe_local_node(NodeId::new(), &p, "0.1.0", orkestr_domain::now());
        assert!(n.is_local);
        assert!(n.resources.cpu_cores > 0);
        assert!(!n.hostname.is_empty());
    }

    #[test]
    fn monitor_samples_and_detects() {
        let dir = tempfile::tempdir().unwrap();
        let p: Arc<dyn PlatformAdapter> = Arc::new(NativePlatform::new(Paths::from_root(dir.path())));
        let opts = DetectOptions { ollama_endpoint: "127.0.0.1:9".into(), ..DetectOptions::default() };
        let m = NodeMonitor::new(NodeId::new(), p, "0.1.0", opts);
        assert!(m.latest().is_none());
        let s = m.sample();
        assert_eq!(m.latest().unwrap().ts, s.ts);
        let node = m.local_node();
        assert_eq!(node.resources.memory_total_bytes, s.memory_total_bytes);
        let info = m.info(false);
        assert!(info.cpu.logical_cores >= 1);
        assert!(!info.disks.is_empty());
        assert!(info.disks.iter().any(|d| d.is_data_volume));
        assert!(!info.local_models.ollama_reachable);
        let again = m.info(false);
        assert!(Arc::ptr_eq(&info, &again), "cached");
        let node = m.local_node();
        assert!(node.tools.contains(&"cargo".to_owned()), "{:?}", node.tools);
    }
}
