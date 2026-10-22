//! [`Detective`]: facade used by `core/src/rpc/detective.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_events::EventBus;
use orkestr_security::Security;
use orkestr_storage::Db;

/// Services the detective needs (agent runtime, quality, reviews, VCS are
/// injected by WP6 through `core/src/rpc/detective.rs`).
#[derive(Clone)]
pub struct DetectiveDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Approvals, audit.
    pub security: Arc<Security>,
    /// `<data_dir>/detective`.
    pub data_dir: PathBuf,
}

/// Bug Detective.
pub struct Detective {
    deps: DetectiveDeps,
}

impl std::fmt::Debug for Detective {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detective").field("data_dir", &self.deps.data_dir).finish_non_exhaustive()
    }
}

impl Detective {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: DetectiveDeps) -> Self {
        Self { deps }
    }

    /// Cancels every running investigation (emergency stop). Returns how many
    /// were cancelled; the step-0 skeleton never runs one (0).
    pub async fn cancel_all(&self) -> usize {
        0
    }
}
