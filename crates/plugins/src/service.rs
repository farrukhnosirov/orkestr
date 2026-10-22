//! [`PluginService`]: the plugin host facade used by `core/src/rpc/plugins.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_domain::PluginId;
use orkestr_events::EventBus;
use orkestr_protocol::methods::plugins::{
    GrantPluginParams, InstallPluginParams, InstallPluginResult, InvokePluginParams, InvokePluginResult,
    KillAllPluginsResult, PluginInfo, PluginLogLine,
};
use orkestr_security::Security;
use orkestr_storage::Db;

use crate::PluginError;

/// Services the plugin host needs.
#[derive(Clone)]
pub struct PluginDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Permission engine, audit, vault.
    pub security: Arc<Security>,
    /// `<data_dir>/plugins`.
    pub data_dir: PathBuf,
}

/// Plugin host.
pub struct PluginService {
    deps: PluginDeps,
}

impl std::fmt::Debug for PluginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginService").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

type R<T> = Result<T, PluginError>;

impl PluginService {
    /// Creates the service (no plugin code is loaded here).
    #[must_use]
    pub fn new(deps: PluginDeps) -> Self {
        Self { deps }
    }

    /// Installed plugins.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn list(&self) -> R<Vec<PluginInfo>> {
        Err(PluginError::Unimplemented("plugins.list"))
    }

    /// One plugin.
    ///
    /// # Errors
    /// Not found.
    pub async fn get(&self, _id: PluginId) -> R<PluginInfo> {
        Err(PluginError::Unimplemented("plugins.get"))
    }

    /// Installs a package.
    ///
    /// # Errors
    /// Invalid / unsigned / untrusted package.
    pub async fn install(&self, _params: InstallPluginParams) -> R<InstallPluginResult> {
        Err(PluginError::Unimplemented("plugins.install"))
    }

    /// Decides permission requests.
    ///
    /// # Errors
    /// Not found / scope mismatch.
    pub async fn grant(&self, _params: GrantPluginParams) -> R<PluginInfo> {
        Err(PluginError::Unimplemented("plugins.grant"))
    }

    /// Uninstalls a plugin.
    ///
    /// # Errors
    /// Not found.
    pub async fn uninstall(&self, _id: PluginId) -> R<()> {
        Err(PluginError::Unimplemented("plugins.uninstall"))
    }

    /// Enables (`true`) / disables (`false`) a plugin.
    ///
    /// # Errors
    /// Not found / pending grants / safe mode.
    pub async fn set_enabled(&self, _id: PluginId, _enabled: bool, _reason: Option<String>) -> R<PluginInfo> {
        Err(PluginError::Unimplemented("plugins.enable"))
    }

    /// Kill switch: interrupts running calls and moves the plugin (`Some`) or
    /// every plugin (`None`) to `Killed`. Used by `plugins.killAll` and the
    /// emergency stop; never fails (nothing to kill = zero counts).
    pub async fn kill(&self, _id: Option<PluginId>, _reason: &str) -> KillAllPluginsResult {
        // Step-0 skeleton: no plugin code can run yet, so there is nothing to kill.
        KillAllPluginsResult::default()
    }

    /// Current safe mode.
    #[must_use]
    pub fn safe_mode(&self) -> bool {
        false
    }

    /// Sets safe mode (persisted).
    ///
    /// # Errors
    /// Storage failure.
    pub async fn set_safe_mode(&self, _enabled: bool) -> R<bool> {
        Err(PluginError::Unimplemented("plugins.setSafeMode"))
    }

    /// Invokes a tool of a code plugin.
    ///
    /// # Errors
    /// Not enabled / denied / trapped.
    pub async fn invoke(&self, _params: InvokePluginParams) -> R<InvokePluginResult> {
        Err(PluginError::Unimplemented("plugins.invoke"))
    }

    /// Log lines.
    ///
    /// # Errors
    /// Not found.
    pub async fn logs(&self, _id: PluginId, _limit: u32) -> R<Vec<PluginLogLine>> {
        Err(PluginError::Unimplemented("plugins.logs"))
    }
}
