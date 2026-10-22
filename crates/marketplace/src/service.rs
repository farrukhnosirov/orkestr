//! [`Marketplace`]: facade used by `core/src/rpc/marketplace.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_events::EventBus;
use orkestr_plugins::PluginService;
use orkestr_security::Security;
use orkestr_storage::Db;

/// Services the marketplace needs.
#[derive(Clone)]
pub struct MarketplaceDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Vault (publisher key), audit.
    pub security: Arc<Security>,
    /// Plugin host (installs).
    pub plugins: Arc<PluginService>,
    /// `<data_dir>/marketplace` (cache).
    pub data_dir: PathBuf,
}

/// Marketplace service. Step-0 skeleton: every operation is
/// `unimplementedFeature` at the RPC layer.
pub struct Marketplace {
    deps: MarketplaceDeps,
}

impl std::fmt::Debug for Marketplace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Marketplace").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

impl Marketplace {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: MarketplaceDeps) -> Self {
        Self { deps }
    }
}
