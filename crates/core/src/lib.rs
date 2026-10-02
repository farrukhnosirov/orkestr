//! # orkestr-core
//!
//! Composition root of the Orkestr AI Core (contract §3):
//!
//! * [`start`] boots everything — paths, SQLite + migrations, event bus,
//!   services, crash recovery — and binds the RPC server on
//!   `127.0.0.1:<ephemeral>` with a random 256-bit bearer token.
//! * [`CoreHandle`] exposes `{url, token}` and graceful shutdown.
//! * [`server`] — axum: `GET /health` (no auth) and `GET /rpc` (WebSocket).
//! * [`rpc`] — method router; one module per area (`rpc/<area>.rs`).
//! * [`discovery`] — `<data_dir>/core.json` so clients find a running core.
//! * [`recovery`] — unclean-shutdown detection and recovery hooks (§62.13).
//! * `orkestrd` — the headless binary (§12).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod config;
pub mod discovery;
pub mod error;
pub mod logging;
pub mod migrations;
pub mod recovery;
pub mod redact;
pub mod rpc;
pub mod server;
pub mod services;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use orkestr_events::{EventBus, SqliteEventLog};
use orkestr_platform::{NativePlatform, Paths, PlatformAdapter};
use orkestr_storage::{Compression, Db, FsObjectStore, SqliteMetadataStore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use config::CoreConfig;
pub use error::CoreError;
pub use services::Services;

/// Version of this core build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A running core.
pub struct CoreHandle {
    /// WebSocket RPC URL, e.g. `ws://127.0.0.1:53211/rpc`.
    pub url: String,
    /// HTTP base URL, e.g. `http://127.0.0.1:53211`.
    pub http_url: String,
    /// Bearer token (256-bit, hex).
    pub token: String,
    /// Bound address.
    pub addr: SocketAddr,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Shared services (for embedding hosts and tests).
    pub services: Arc<Services>,
    shutdown: CancellationToken,
    server: Option<JoinHandle<()>>,
    discovery_written: bool,
}

impl std::fmt::Debug for CoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreHandle")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl CoreHandle {
    /// A token that is cancelled when the core shuts down.
    #[must_use]
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Gracefully stops the server, removes the discovery file and marks the
    /// session as cleanly finished.
    pub async fn shutdown(mut self) {
        self.shutdown_inner().await;
    }

    async fn shutdown_inner(&mut self) {
        self.shutdown.cancel();
        if let Some(server) = self.server.take() {
            if tokio::time::timeout(std::time::Duration::from_secs(5), server).await.is_err() {
                tracing::warn!("RPC server did not stop within 5s");
            }
        }
        if self.discovery_written {
            discovery::remove_if_ours(&self.services.paths);
            self.discovery_written = false;
        }
        recovery::end_session(&self.services.paths);
        logging::detach_event_bus();
        tracing::info!("orkestr core stopped");
        redact::unregister_secret(&self.token);
    }
}

impl Drop for CoreHandle {
    fn drop(&mut self) {
        // Best effort when the host forgot to call `shutdown()`: stop serving.
        // Files are left behind and the next start treats it as unclean.
        self.shutdown.cancel();
    }
}

/// Starts the core.
pub async fn start(config: CoreConfig) -> Result<CoreHandle, CoreError> {
    let started = Instant::now();
    config.validate()?;
    let paths = match &config.data_dir {
        Some(dir) => Paths::from_root(dir),
        None => Paths::resolve()?,
    };
    paths.ensure()?;
    let platform: Arc<dyn PlatformAdapter> = Arc::new(NativePlatform::new(paths.clone()));

    // Refuse to share a data dir with another live core.
    if let Some(running) = discovery::find_running(&paths).await {
        return Err(CoreError::AlreadyRunning { url: running.url, pid: running.pid });
    }
    let previous_session = recovery::begin_session(&paths, platform.process())?;

    // Storage.
    let db = Db::open(paths.database_file())?;
    db.migrate(migrations::all()).await?;
    let objects = Arc::new(FsObjectStore::open(paths.objects_dir(), Compression::Zstd { level: 3 })?);
    let metadata = Arc::new(SqliteMetadataStore::new(db.clone()));
    let event_log = Arc::new(SqliteEventLog::new(db.clone()));
    let events = EventBus::with_log(event_log, config.event_capacity).await?;
    let node_id = migrations::load_or_create_node_id(&db).await?;
    let files = Arc::new(orkestr_files::FilesManager::new(
        orkestr_files::FilesConfig::default(),
        Arc::new(orkestr_files::StorageTrash::new(db.clone(), objects.clone())),
        Arc::new(orkestr_files::SystemOpener::new(Arc::new(orkestr_platform::process::which))),
        Arc::new(rpc::files::EventBusChangeSink::new(events.clone())),
    ));

    let services = Arc::new(Services {
        version: VERSION.to_owned(),
        started_at: chrono::Utc::now(),
        started_instant: started,
        node_id,
        paths: paths.clone(),
        platform,
        db,
        events,
        projects: metadata,
        objects,
        files,
        recovered_from_crash: previous_session.crashed,
    });
    logging::attach_event_bus(services.events.clone());

    // Crash recovery hooks (task-engine etc. register theirs here later).
    recovery::run(&services, &previous_session, &recovery::default_hooks()).await;

    // RPC server.
    let token = config.token.clone().unwrap_or_else(auth::generate_token);
    redact::register_secret(&token);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;
    let url = format!("ws://{addr}/rpc");
    let http_url = format!("http://{addr}");
    let shutdown = CancellationToken::new();
    let router = Arc::new(rpc::build_router());
    let app = server::app(server::AppState::new(Arc::clone(&services), token.clone(), router, shutdown.clone()));
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        let result = axum::serve(listener, app).with_graceful_shutdown(server_shutdown.cancelled_owned()).await;
        if let Err(e) = result {
            tracing::error!(error = %e, "RPC server failed");
        }
    });

    let mut discovery_written = false;
    if config.write_discovery {
        discovery::write(&paths, &url, &token)?;
        discovery_written = true;
    }
    tracing::info!(%url, data_dir = %paths.data_dir.display(), startup_ms = started.elapsed().as_millis() as u64, "orkestr core started");

    Ok(CoreHandle {
        url,
        http_url,
        token,
        addr,
        data_dir: paths.data_dir.clone(),
        services,
        shutdown,
        server: Some(server),
        discovery_written,
    })
}
