//! # orkestr-node
//!
//! Nodes (MASTER_SPEC §38–39). Phase 0 implements [`describe_local_node`]
//! (OS, arch, resources via `orkestr-platform`) and fixes the
//! [`NodeRegistry`] interface; Phase 6 adds remote nodes, heartbeats,
//! scheduling by requirements and task migration via checkpoints.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{Node, NodeId, NodeResources, NodeStatus, Timestamp};
use orkestr_platform::PlatformAdapter;

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

/// Tools probed on PATH to describe a node.
pub const PROBED_TOOLS: &[&str] = &["git", "node", "npm", "cargo", "docker", "python", "ollama", "claude", "codex"];

/// Describes the local node from platform data.
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
}
