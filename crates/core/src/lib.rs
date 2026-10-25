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
pub mod benchmarks;
pub mod checks;
pub mod claude_code;
pub mod claude_engine;
pub mod codex;
pub mod config;
pub mod cursor_models;
pub mod dashboards_store;
pub mod discovery;
pub mod ecosystem;
pub mod engines;
pub mod error;
pub mod gates;
pub mod housekeeping;
pub mod integration;
pub mod layouts_store;
pub mod logging;
pub mod migrations;
pub mod nodes;
pub mod plugin_tools;
pub mod recovery;
pub mod redact;
pub mod remote;
pub mod resume;
pub mod rpc;
pub mod server;
pub mod services;
pub mod settings_store;
pub mod ui_store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use orkestr_events::{EventBus, SqliteEventLog};
use orkestr_platform::{NativePlatform, Paths, PlatformAdapter};
use orkestr_storage::{Compression, Db, FsObjectStore, SqliteMetadataStore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use config::{AutostartConfig, CoreConfig};
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
    housekeeping: Option<JoinHandle<()>>,
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
        // Manager supervisors first (runs become Interrupted, resumable), then
        // agent runs (their commands are killed with the terminals).
        self.services.manager.shutdown().await;
        self.services.agents.shutdown().await;
        self.services.quality.shutdown().await;
        // Kill terminal process trees before the server goes away.
        self.services.terminals.shutdown().await;
        // The VS Code server runs extension code: never outlive the core.
        self.services.vscode.shutdown().await;
        // Language servers (ADR-0025).
        self.services.lsp.shutdown(std::time::Duration::from_secs(5)).await;
        // Claude Code chat sessions (kill their process trees).
        self.services.claude.manager.shutdown(std::time::Duration::from_secs(5)).await;
        // Phase 7: detective runs become resumable/failed, monitor loops stop.
        self.services.detective.shutdown().await;
        self.services.monitor.shutdown();
        if let Some(server) = self.server.take() {
            if tokio::time::timeout(std::time::Duration::from_secs(5), server).await.is_err() {
                tracing::warn!("RPC server did not stop within 5s");
            }
        }
        if let Some(hk) = self.housekeeping.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), hk).await;
        }
        let last_seq = self.services.events.last_seq();
        if let Err(e) = self.services.seq_high_water.finish(last_seq).await {
            tracing::error!(error = %e, "failed to record the event seq high-water mark");
        }
        if self.discovery_written {
            discovery::remove_if_ours(&self.services.paths);
            self.discovery_written = false;
        }
        // Graceful shutdown: in-flight tasks become Interrupted (ADR-0022).
        if let Err(e) = self.services.tasks.recover_interrupted("shutdown").await {
            tracing::error!(error = %e, "failed to interrupt running tasks on shutdown");
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
    // Seq high-water mark: the bus resumes after max(log max, reservation).
    let seq_high_water = Arc::new(housekeeping::SeqHighWater::load(db.clone()).await?);
    let event_log: Arc<dyn orkestr_events::EventLog> =
        Arc::new(housekeeping::SeqFloorLog::new(Arc::new(SqliteEventLog::new(db.clone())), seq_high_water.reserved()));
    let events = EventBus::with_log(event_log, config.event_capacity).await?;
    seq_high_water.reserve_from(events.last_seq()).await?;
    let node_id = migrations::load_or_create_node_id(&db).await?;
    // Bind early so the guard knows the core port (H10: never expose it).
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;

    // Security subsystem (permissions, approvals, secrets, audit, emergency).
    let credentials: Option<Arc<dyn orkestr_platform::CredentialStore>> = match config.secret_backend {
        orkestr_security::SecretBackendChoice::Memory => {
            Some(Arc::new(orkestr_platform::MemoryCredentialStore::default()))
        }
        orkestr_security::SecretBackendChoice::VaultFileKey => None,
        _ => Some(Arc::new(orkestr_platform::KeyringCredentialStore::new())),
    };
    let security = Arc::new(
        orkestr_security::Security::open(orkestr_security::SecurityConfig {
            db: db.clone(),
            events: events.clone(),
            data_dir: paths.data_dir.clone(),
            credentials,
            secret_backend: config.secret_backend,
            guard: orkestr_security::GuardConfig {
                protected_paths: vec![paths.data_dir.clone()],
                core_port: Some(addr.port()),
            },
        })
        .await?,
    );
    // Terminal: every command (user `run`s and agent lines) passes the
    // security gate (permission engine + dangerous-action guard + approvals).
    let terminals = orkestr_terminal::TerminalService::builder()
        .sink(Arc::new(rpc::terminal::EventTerminalSink::new(events.clone())))
        .log(Arc::new(orkestr_terminal::SqliteCommandLog::new(db.clone(), redact::redact)))
        .gate(Arc::new(gates::SecurityCommandGate::new(Arc::clone(&security), Arc::clone(&metadata))))
        .build();

    // Provider hub: first start registers the Mock and detects Ollama / CLIs.
    let providers = Arc::new(orkestr_providers::ProviderRegistry::new(
        db.clone(),
        Arc::new(rpc::providers::SecretServiceCredentials::new(Arc::clone(&security.secrets))),
        Arc::new(rpc::providers::BusProviderSink::new(events.clone())),
    ));
    if let Err(e) = providers.init().await {
        tracing::error!(error = %e, "provider registry failed to initialise");
    }

    let tasks = Arc::new(orkestr_task_engine::TaskService::new(db.clone(), events.clone()));
    // VCS + snapshots (Phase 1). Git is optional: without it vcs.* methods
    // report a clear error and snapshots carry no git metadata.
    let git = Arc::new(orkestr_vcs::GitCli::new(orkestr_vcs::GitRunner::new(platform.process().which("git"))));
    if !git.is_available() {
        tracing::warn!("git not found on PATH — Git features are disabled");
    }
    let vcs = orkestr_vcs::Vcs::new(Arc::clone(&git), db.clone(), paths.data_dir.join("worktrees"));
    let git_state: Arc<dyn orkestr_snapshots::GitStateProvider> = git;
    let snapshots = Arc::new(orkestr_snapshots::SnapshotService::new(
        db.clone(),
        Arc::clone(&objects) as Arc<dyn orkestr_storage::ObjectStore>,
        Some(git_state),
        orkestr_snapshots::ScanOptions::default(),
    ));
    let files = Arc::new(orkestr_files::FilesManager::new(
        orkestr_files::FilesConfig::default(),
        Arc::new(orkestr_files::StorageTrash::new(db.clone(), objects.clone())),
        Arc::new(orkestr_files::SystemOpener::new(Arc::new(orkestr_platform::process::which))),
        Arc::new(rpc::files::EventBusChangeSink::new(events.clone())),
    ));

    let memory = orkestr_memory::MemoryEngine::new(db.clone()).with_events(events.clone());
    let cost = orkestr_cost::CostEngine::new(db.clone()).with_events(events.clone());
    let previous_session_started_at = housekeeping::rotate_session(&db, chrono::Utc::now()).await?;
    let settings = Arc::new(settings_store::SettingsStore::load(db.clone()).await?);
    let nodes = Arc::new(orkestr_node::NodeMonitor::new(
        node_id,
        Arc::clone(&platform),
        VERSION,
        orkestr_node::DetectOptions::default(),
    ));

    let ui_language = orkestr_agent_runtime::UiLanguage::default();
    // An explicit preference, else the language the UI last resolved `system`
    // to (persisted by `settings.setUiLanguage`), so restart notices are not
    // English for a Russian UI before the UI reconnects.
    if let Some(lang) = settings.startup_ui_language() {
        ui_language.set(&lang);
    }
    // Global parallel scheduler (§11): admission for every Manager run and the
    // API-concurrency gate around the model gateway (agents + Manager).
    let parallel = Arc::new(orkestr_orchestrator::ParallelScheduler::new(orkestr_orchestrator::ParallelDeps {
        db: db.clone(),
        events: events.clone(),
        tasks: Arc::clone(&tasks),
        hub: Arc::new(orkestr_orchestrator::RegistryHub(Arc::clone(&providers))),
        resources: Arc::new(rpc::manager::NodeResourceProbe(Arc::clone(&nodes))),
        vcs: vcs.clone(),
    }));
    let models: Arc<dyn orkestr_agent_runtime::ModelGateway> = Arc::new(orkestr_orchestrator::GatedGateway::new(
        Arc::new(orkestr_agent_runtime::RegistryGateway(Arc::clone(&providers))),
        Arc::clone(&parallel),
    ));
    let agents = orkestr_agent_runtime::AgentRuntime::new(orkestr_agent_runtime::RuntimeDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        tasks: Arc::clone(&tasks),
        projects: Arc::clone(&metadata) as Arc<dyn orkestr_storage::ProjectRepository>,
        vcs: vcs.clone(),
        snapshots: Arc::clone(&snapshots),
        terminals: terminals.clone(),
        memory: memory.clone(),
        cost: cost.clone(),
        models: Arc::clone(&models),
        config: orkestr_agent_runtime::RuntimeConfig {
            ui_language: ui_language.clone(),
            ..orkestr_agent_runtime::RuntimeConfig::default()
        },
    });
    if let Err(e) = agents.init().await {
        tracing::error!(error = %e, "agent runtime failed to initialise");
    }
    let manager = orkestr_orchestrator::Manager::with_parallel(
        orkestr_orchestrator::OrchestratorDeps {
            db: db.clone(),
            events: events.clone(),
            security: Arc::clone(&security),
            tasks: Arc::clone(&tasks),
            projects: Arc::clone(&metadata) as Arc<dyn orkestr_storage::ProjectRepository>,
            vcs: vcs.clone(),
            snapshots: Arc::clone(&snapshots),
            memory: memory.clone(),
            cost: cost.clone(),
            agents: agents.clone(),
            models,
            hub: Arc::new(orkestr_orchestrator::RegistryHub(Arc::clone(&providers))),
            resources: Arc::new(rpc::manager::NodeResourceProbe(Arc::clone(&nodes))),
            config: orkestr_orchestrator::ManagerTunables {
                state_root: paths.data_dir.clone(),
                ui_language: ui_language.clone(),
                ..orkestr_orchestrator::ManagerTunables::default()
            },
        },
        parallel,
    );
    if let Err(e) = manager.init().await {
        tracing::error!(error = %e, "AI Manager failed to initialise");
    }
    if let Err(e) = manager.recover_reviews().await {
        tracing::error!(error = %e, "failed to recover reviews / consensus runs of the previous session");
    }

    let quality = orkestr_quality::QualityService::new(orkestr_quality::QualityDeps {
        db: db.clone(),
        events: events.clone(),
        executor: Arc::new(orkestr_quality::TerminalExecutor::new(terminals.clone())),
        redact: Arc::new(|s: &str| redact::redact(s)),
        config: orkestr_quality::QualityConfig {
            detect: orkestr_quality::DetectOptions {
                windows: platform.info().os == orkestr_domain::OsKind::Windows,
                ..orkestr_quality::DetectOptions::default()
            },
            ..orkestr_quality::QualityConfig::default()
        },
    });
    if let Err(e) = quality.init().await {
        tracing::error!(error = %e, "quality service failed to initialise");
    }
    // Quality gates read the code reviews (reviewer approved / no critical
    // security finding) instead of only the task's review field.
    let review_gates = Arc::new(integration::ReviewGates::new(manager.clone()));
    quality.set_review_source(Arc::clone(&review_gates) as Arc<dyn orkestr_quality::ReviewStatusSource>);
    quality.set_security_source(review_gates as Arc<dyn orkestr_quality::SecurityFindingsSource>);

    let architecture = orkestr_insight::ArchitectureService::new(
        db.clone(),
        Some(events.clone()),
        orkestr_insight::ScanOptions::default(),
    );
    // Merge Manager (Phase 2, §16): shares the Manager's engines; its review
    // and tests gates come from the code reviews and the quality runs, and
    // the integration branch is checked by the quality runner before the
    // target moves.
    let merge = orkestr_orchestrator::MergeManager::new(
        orkestr_orchestrator::MergeDeps::from_manager(&manager)
            .with_gates(Arc::new(integration::MergeGates::new(manager.clone(), quality.clone())))
            .with_quality(Arc::new(integration::MergeQuality::new(quality.clone()))),
    );
    if let Err(e) = merge.init().await {
        tracing::error!(error = %e, "Merge Manager failed to initialise");
    }
    // Quality pipeline (Phase 2, §17): Build / Test stages and gates through
    // the quality runner; the Merge stage through the Merge Manager.
    manager.set_pipeline_checks(Arc::new(integration::PipelineQuality::new(quality.clone(), Arc::clone(&snapshots))));
    manager.set_merge_manager(merge.clone());

    // Embedded VS Code (VSCodium REH web): nothing is downloaded or started
    // until the user asks for it (`vscode.install` / `vscode.start`).
    let vscode = orkestr_vscode_host::VsCodeHost::new(
        orkestr_vscode_host::HostOptions::new(
            paths.data_dir.join("vscode-server"),
            platform.info().os,
            platform.info().arch,
            Arc::new(orkestr_platform::NativeProcessControl),
        )
        .with_events(events.clone()),
    )
    .map_err(|e| CoreError::Config(format!("VS Code host: {e}")))?;
    vscode.spawn_background();
    // VS Code extensions (ADR-0025): Open VSX registry, `<data_dir>/extensions`.
    let extensions = Arc::new(
        orkestr_extensions::ExtensionService::open(
            db.clone(),
            paths.data_dir.join("extensions"),
            orkestr_extensions::OpenVsxClient::new(None)?,
            Arc::new(rpc::extensions::BusExtensionEvents::new(events.clone())),
        )
        .await?,
    );
    let (lsp_tx, lsp_rx) = tokio::sync::mpsc::channel(4096);
    let lsp = orkestr_extensions::lsp::LspManager::new(
        lsp_tx,
        orkestr_extensions::lsp::LspConfig::default(),
        Arc::new(orkestr_platform::NativeProcessControl),
    );

    // Claude Code panel: one official `claude` process per chat session.
    let (claude_tx, claude_rx) = tokio::sync::mpsc::channel(4096);
    let claude = claude_code::ClaudeCodeService::new(
        orkestr_providers::claude_code_session::ClaudeSessionManager::new(
            claude_tx,
            Arc::new(claude_code::GuardPolicy::new(Arc::clone(&security))),
            Arc::new(orkestr_platform::NativeProcessControl),
        ),
        &paths.data_dir,
        vscode.layout().extensions_dir(),
    );
    // Codex (ChatGPT subscription): one shared `codex app-server` for the panel.
    let (codex_tx, codex_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut codex = codex::CodexService::new(&paths.data_dir, codex_tx, Arc::clone(&security));
    codex.open_browser = config.claude_auto_register;

    // Encrypted sync engine (Phase 4, ADR-0031): `<data_dir>/sync`.
    let sync_engine = match orkestr_sync::DefaultSyncEngine::open(&paths.data_dir.join("sync")) {
        Ok(e) => Some(e),
        Err(e) => {
            tracing::warn!(error = %e, "sync engine unavailable");
            None
        }
    };
    let remote = Arc::new(remote::RemoteService::new());
    let node_hub = Arc::new(nodes::NodeHub::new(
        db.clone(),
        Arc::clone(&nodes),
        Arc::clone(&remote),
        &orkestr_node::EnvSource::process(),
    ));
    // Ecosystem (Phase 7, ADR-0034 / 0040 / 0041 / 0042): constructed idle;
    // `ecosystem::wire` starts them once the RPC router exists.
    let plugins = Arc::new(orkestr_plugins::PluginService::new(orkestr_plugins::PluginDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: paths.data_dir.join("plugins"),
    }));
    let marketplace = Arc::new(orkestr_marketplace::Marketplace::new(orkestr_marketplace::MarketplaceDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        plugins: Arc::clone(&plugins),
        data_dir: paths.data_dir.join("marketplace"),
    }));
    let deploy = Arc::new(orkestr_deploy::DeployService::new(orkestr_deploy::DeployDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        objects: Arc::clone(&objects),
        data_dir: paths.data_dir.join("deploy"),
    }));
    let monitor = Arc::new(orkestr_monitor::MonitorService::new(orkestr_monitor::MonitorDeps {
        db: db.clone(),
        events: events.clone(),
        nodes: Arc::clone(&nodes),
        data_dir: paths.data_dir.join("monitor"),
    }));
    let detective = Arc::new(orkestr_detective::Detective::new(orkestr_detective::DetectiveDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: paths.data_dir.join("detective"),
    }));
    // Computer assistant (ADR-0035). ORKESTR_COMPUTER_BACKEND=mock|unsupported
    // swaps the OS backend (tests / CI never drive the real desktop).
    let computer_backend: Arc<dyn orkestr_computer::ComputerBackend> =
        match std::env::var("ORKESTR_COMPUTER_BACKEND").unwrap_or_default().as_str() {
            "mock" => Arc::new(orkestr_computer::MockBackend::default()),
            "unsupported" => Arc::new(orkestr_computer::UnsupportedBackend),
            _ => orkestr_computer::default_backend(),
        };
    let computer = orkestr_computer::ComputerService::new(orkestr_computer::ComputerDeps {
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: paths.data_dir.join("computer"),
        backend: computer_backend,
    });
    // Cursor capture mode (ADR-0050): same switch — `mock` never touches the real desktop.
    let cursor_screen: Arc<dyn orkestr_computer::cursor::screen::CursorBackend> =
        match std::env::var("ORKESTR_COMPUTER_BACKEND").unwrap_or_default().as_str() {
            "mock" => Arc::new(orkestr_computer::cursor::MockScreen::default()),
            "unsupported" => Arc::new(orkestr_computer::cursor::UnsupportedScreen),
            _ => orkestr_computer::cursor::default_cursor_backend(),
        };
    let cursor_factory = Arc::new(cursor_models::CoreCursorFactory::new(paths.data_dir.join("computer").join("cursor-work")));
    let cursor = orkestr_computer::cursor::CursorService::new(orkestr_computer::cursor::CursorDeps {
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: paths.data_dir.join("computer"),
        computer: Arc::clone(&computer),
        screen: cursor_screen,
        factory: cursor_factory.clone(),
    });
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
        memory,
        cost,
        objects,
        providers,
        tasks,
        files,
        recovered_from_crash: previous_session.crashed,
        security,
        vcs,
        snapshots,
        terminals,
        previous_session_started_at,
        settings,
        nodes,
        autostart: config.autostart.clone(),
        node_sample_interval: config.node_sample_interval,
        seq_high_water,
        agents,
        manager,
        merge,
        ui_language,
        quality,
        architecture,
        benchmarks: Arc::new(benchmarks::BenchmarkStats::new()),
        vscode,
        extensions,
        lsp,
        claude,
        codex,
        remote,
        node_hub,
        plugins,
        marketplace,
        deploy,
        monitor,
        detective,
        computer,
        cursor,
        cursor_factory,
        voice: Arc::new(orkestr_voice::VoiceService::new(paths.data_dir.join("voice"))),
        sync: sync_engine.clone().map(|e| Arc::new(e) as Arc<dyn orkestr_sync::SyncEngine>),
    });
    logging::attach_event_bus(services.events.clone());

    // Crash recovery hooks (task-engine etc. register theirs here later).
    recovery::run(&services, &previous_session, &recovery::default_hooks()).await;
    // Background agents (§12): interrupted work resumes on its own when the
    // scheduler's auto-resume policy allows; otherwise it waits for the user.
    {
        let manager = services.manager.clone();
        tokio::spawn(async move {
            let report = manager.auto_resume_interrupted().await;
            tracing::debug!(?report, "auto-resume after start");
        });
    }

    // RPC server.
    let token = config.token.clone().unwrap_or_else(auth::generate_token);
    redact::register_secret(&token);
    let url = format!("ws://{addr}/rpc");
    let http_url = format!("http://{addr}");
    let shutdown = CancellationToken::new();
    tokio::spawn(
        Arc::clone(&services.security.approvals).run_expiry_loop(std::time::Duration::from_secs(30), shutdown.clone()),
    );
    {
        let secrets = Arc::clone(&services.security.secrets);
        tokio::spawn(async move {
            match secrets.preload_redaction().await {
                Ok(n) if n > 0 => tracing::info!(count = n, "secret values registered for redaction"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not preload secret values for redaction"),
            }
        });
    }
    services.providers.spawn_health_loop(shutdown.clone());
    if let Some(engine) = sync_engine {
        // Offline queue (§8): retry pending uploads / failed rounds.
        let stop = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = engine.run_retry_loop(std::time::Duration::from_secs(30)) => {}
                () = stop.cancelled() => {}
            }
        });
    }
    rpc::lsp::spawn_bridge(services.lsp.clone(), lsp_rx, services.events.clone(), shutdown.clone());
    claude_code::spawn_bridge(Arc::clone(&services), claude_rx);
    codex::spawn_bridge(Arc::clone(&services), codex_rx);
    engines::spawn_balancer(Arc::clone(&services));
    if config.claude_auto_register {
        claude_engine::spawn_auto_register(Arc::clone(&services));
        codex::spawn_auto_register(Arc::clone(&services));
    }
    // Architecture Map: debounced incremental refresh after file changes (§21).
    orkestr_insight::arch::spawn_auto_refresh(
        services.architecture.clone(),
        services.events.clone(),
        Arc::clone(&services.projects),
        orkestr_insight::arch::AutoRefreshConfig::default(),
        shutdown.clone(),
    );
    // Benchmarks + router statistics + context-index invalidation (Phase 2).
    benchmarks::spawn_collector(Arc::clone(&services), shutdown.clone());
    let router = Arc::new(rpc::build_router());
    let app_state = server::AppState::new(Arc::clone(&services), token.clone(), Arc::clone(&router), shutdown.clone())
        .with_port(addr.port());
    // Remote access (Phase 4): load device registry + CIK, re-dial the tunnel when enabled.
    if let Err(e) = services.remote.attach(
        &paths.data_dir,
        services.events.clone(),
        Arc::clone(&services.security.audit),
        server::connection::remote_runner(&app_state),
        shutdown.clone(),
        services.version.clone(),
    ) {
        tracing::error!(error = %e, "remote access unavailable: cannot load the core identity key");
    }
    // Multi-node (Phase 6): wire registry ↔ jobs ↔ blobs ↔ models (needs the
    // CIK loaded above), then pull heartbeats to enrolled workers.
    nodes::wiring::wire(&services, Arc::clone(&router), &shutdown);
    nodes::heartbeat::spawn(Arc::clone(&services), shutdown.clone());
    // Web Push (Phase 4, ADR-0036): event → push loop (stops on shutdown).
    remote::push::spawn_loop(&services, shutdown.clone());
    // Ecosystem (Phase 7): plugin tools, infra monitor, Bug Detective.
    ecosystem::wire(&services, Arc::clone(&router)).await;
    // Computer assistant: AI planner over the provider stack + Start-menu index.
    rpc::computer::install(&services);
    services.cursor_factory.attach(&services);
    let app = server::app(app_state);
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await;
        if let Err(e) = result {
            tracing::error!(error = %e, "RPC server failed");
        }
    });

    let housekeeping = housekeeping::spawn(
        Arc::clone(&services),
        shutdown.clone(),
        config.node_sample_interval,
        config.node_info_prefetch_after,
    );

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
        housekeeping: Some(housekeeping),
        discovery_written,
    })
}
