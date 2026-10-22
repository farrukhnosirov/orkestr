//! [`MonitorService`]: facade used by `core/src/rpc/monitor.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_events::EventBus;
use orkestr_storage::Db;

/// Services the monitor needs.
#[derive(Clone)]
pub struct MonitorDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Local resource sampler.
    pub nodes: Arc<orkestr_node::NodeMonitor>,
    /// `<data_dir>/monitor`.
    pub data_dir: PathBuf,
}

/// Infrastructure monitor.
pub struct MonitorService {
    deps: MonitorDeps,
}

impl std::fmt::Debug for MonitorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorService").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

impl MonitorService {
    /// Creates the service (no background loops are started here).
    #[must_use]
    pub fn new(deps: MonitorDeps) -> Self {
        Self { deps }
    }
}
