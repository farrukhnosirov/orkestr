//! Sync server (ref / chunk store) mounted by the gateway at `/v1/sync`.
//!
//! Step 0 skeleton: [`server_router`] answers every request with
//! `501 Not Implemented` until the sync package lands the store.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::Router;

/// Bearer-token → account id resolver supplied by the gateway.
pub type AuthFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Shared state of the sync server.
#[derive(Clone)]
pub struct SyncServerState {
    dir: PathBuf,
    #[allow(dead_code)] // used once the store lands
    auth: AuthFn,
}

impl std::fmt::Debug for SyncServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncServerState").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl SyncServerState {
    /// Opens (creating if needed) the store under `dir`.
    ///
    /// # Errors
    /// Fails when `dir` cannot be created.
    pub fn open(dir: &Path, auth: AuthFn) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self { dir: dir.to_path_buf(), auth })
    }

    /// Store directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The `/v1/sync` router (mount with `Router::nest("/v1/sync", …)`).
pub fn server_router(state: SyncServerState) -> Router {
    Router::new()
        .fallback(|| async { (StatusCode::NOT_IMPLEMENTED, "sync server: unimplemented-feature") })
        .with_state(state)
}
