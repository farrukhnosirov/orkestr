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
use orkestr_providers::ProviderRegistry;
use orkestr_storage::{Db, FsObjectStore, ProjectRepository};
use orkestr_task_engine::TaskService;

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
    /// AI provider hub (registry, router, fallback).
    pub providers: Arc<ProviderRegistry>,
    /// Task engine (§10–11).
    pub tasks: Arc<TaskService>,
    /// Workspace file service (`files.*`).
    pub files: Arc<orkestr_files::FilesManager>,
    /// Memory engine: memory items, decision log, knowledge base (§20–21).
    pub memory: orkestr_memory::MemoryEngine,
    /// Cost engine: usage, budgets, thresholds (§22).
    pub cost: orkestr_cost::CostEngine,
    /// Whether the previous session ended uncleanly.
    pub recovered_from_crash: bool,
    /// Security subsystem: permissions, approvals, secrets, audit, emergency (Phase 1).
    pub security: Arc<orkestr_security::Security>,
    /// Git / worktrees (Phase 1, `orkestr-vcs`).
    pub vcs: orkestr_vcs::Vcs,
    /// Snapshots and rollback (Phase 1, `orkestr-snapshots`).
    pub snapshots: Arc<orkestr_snapshots::SnapshotService>,
    /// Terminal sessions and command execution.
    pub terminals: orkestr_terminal::TerminalService,
    /// Start of the previous core session (for `system.resumeSummary`).
    pub previous_session_started_at: Option<Timestamp>,
    /// Application settings.
    pub settings: Arc<crate::settings_store::SettingsStore>,
    /// Local node inventory + live resources.
    pub nodes: Arc<orkestr_node::NodeMonitor>,
    /// Autostart registration target.
    pub autostart: crate::config::AutostartConfig,
    /// Interval of the `NodeStatus` heartbeat.
    pub node_sample_interval: std::time::Duration,
    /// Agent runtime: templates, instances, tool loop (§6).
    pub agents: orkestr_agent_runtime::AgentRuntime,
    /// AI Manager: conversations, plans, supervised runs (§1, §5).
    pub manager: orkestr_orchestrator::Manager,
    /// AI Merge Manager: proposals, approval-gated merges, conflicts (§16).
    pub merge: orkestr_orchestrator::MergeManager,
    /// The user's UI language, shared with the Manager and the agent runtime
    /// (they reply in it). Seeded from `AppSettings.language` unless it is
    /// `system` (then the UI reports the resolved language with each Manager
    /// request).
    pub ui_language: orkestr_agent_runtime::UiLanguage,
    /// Quality runner + gates (§17, §58).
    pub quality: orkestr_quality::QualityService,
    /// Architecture Map: scans, snapshots, diffs (§21, Phase 2).
    pub architecture: orkestr_insight::ArchitectureService,
    /// Persistent event seq reservation.
    pub seq_high_water: Arc<crate::housekeeping::SeqHighWater>,
    /// Historical model statistics for router auto-scoring (Phase 2, §22).
    pub benchmarks: Arc<crate::benchmarks::BenchmarkStats>,
    /// Embedded VS Code server (VSCodium REH web, `vscode.*`).
    pub vscode: orkestr_vscode_host::VsCodeHost,
    /// VS Code extensions for the native editor (ADR-0025).
    pub extensions: Arc<orkestr_extensions::ExtensionService>,
    /// Language-server sessions (ADR-0025).
    pub lsp: orkestr_extensions::lsp::LspManager,
    /// Claude Code panel: chat sessions over the official `claude` CLI.
    pub claude: crate::claude_code::ClaudeCodeService,
    /// Remote access via the gateway (Phase 4; skeleton until wired).
    pub remote: Arc<crate::remote::RemoteService>,
    /// Multi-node registry, links, enrollment, worker state (Phase 6).
    pub node_hub: Arc<crate::nodes::NodeHub>,
    /// Encrypted sync engine (Phase 4); `None` until the sync package wires it.
    pub sync: Option<Arc<dyn orkestr_sync::SyncEngine>>,
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

    /// The autostart backend (OS registry/LaunchAgent/XDG, or the configured override).
    #[must_use]
    pub fn autostart_backend(&self) -> &dyn orkestr_platform::Autostart {
        match &self.autostart.backend {
            Some(b) => b.as_ref(),
            None => self.platform.autostart(),
        }
    }
}
