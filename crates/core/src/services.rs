//! The service container (composition root output).
//!
//! Holds `Arc`s to every engine. Engines are added here as their crates land
//! (task engine, agent runtime, providers, …); handlers reach them through
//! `RpcContext::services`. Keep this a plain struct of handles — no logic
//! (§60.3 "no god services").

use std::sync::Arc;
use std::time::Instant;

use orkestr_domain::{NodeId, Timestamp};
use orkestr_events::EventBus;
use orkestr_platform::{Paths, PlatformAdapter};
use orkestr_storage::{Db, FsObjectStore, ProjectRepository};

/// Shared services.
pub struct Services {
    /// Core version.
    pub version: String,
    /// Wall-clock start time.
    pub started_at: Timestamp,
    /// Monotonic start time (uptime).
    pub started_instant: Instant,
    /// Id of this node (persistent per data dir).
    pub node_id: NodeId,
    /// Resolved paths.
    pub paths: Paths,
    /// Platform adapter.
    pub platform: Arc<dyn PlatformAdapter>,
    /// Database.
    pub db: Db,
    /// Event bus (persistent).
    pub events: EventBus,
    /// Project repository.
    pub projects: Arc<dyn ProjectRepository>,
    /// Object store.
    pub objects: Arc<FsObjectStore>,
    /// Memory engine: memory items, decision log, knowledge base (§20–21).
    pub memory: orkestr_memory::MemoryEngine,
    /// Cost engine: usage, budgets, thresholds (§22).
    pub cost: orkestr_cost::CostEngine,
    /// Whether the previous session ended uncleanly.
    pub recovered_from_crash: bool,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("version", &self.version)
            .field("node_id", &self.node_id)
            .field("data_dir", &self.paths.data_dir)
            .finish_non_exhaustive()
    }
}

impl Services {
    /// Uptime in milliseconds.
    #[must_use]
    pub fn uptime_ms(&self) -> u64 {
        u64::try_from(self.started_instant.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
