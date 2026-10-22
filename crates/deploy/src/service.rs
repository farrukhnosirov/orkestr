//! [`DeployService`]: facade used by `core/src/rpc/deploy.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_events::EventBus;
use orkestr_security::Security;
use orkestr_storage::{Db, FsObjectStore};

use crate::DeployAdapter;

/// Services deployments need.
#[derive(Clone)]
pub struct DeployDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Guard, approvals, vault, audit.
    pub security: Arc<Security>,
    /// Log storage.
    pub objects: Arc<FsObjectStore>,
    /// `<data_dir>/deploy` (static sites, process state).
    pub data_dir: PathBuf,
}

/// Deployment service.
pub struct DeployService {
    deps: DeployDeps,
    adapters: Vec<Arc<dyn DeployAdapter>>,
}

impl std::fmt::Debug for DeployService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeployService")
            .field("data_dir", &self.deps.data_dir)
            .field("adapters", &self.adapters.len())
            .finish_non_exhaustive()
    }
}

impl DeployService {
    /// Creates the service with the built-in adapters.
    #[must_use]
    pub fn new(deps: DeployDeps) -> Self {
        Self { deps, adapters: crate::adapters::builtin() }
    }

    /// Registered adapters.
    #[must_use]
    pub fn adapters(&self) -> &[Arc<dyn DeployAdapter>] {
        &self.adapters
    }

    /// Cancels every running deployment (emergency stop). Returns how many
    /// were cancelled; the step-0 skeleton never runs deployments (0).
    pub async fn cancel_all(&self) -> usize {
        0
    }
}
