//! Services the runtime is wired with (by the core's composition root).

use std::sync::Arc;
use std::time::Duration;

use orkestr_events::EventBus;
use orkestr_protocol::NewEvent;
use orkestr_security::Security;
use orkestr_storage::{Db, ProjectRepository};

use crate::gateway::ModelGateway;

/// Tunables.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// How long an approval may stay pending before the agent gives up.
    pub approval_timeout: Duration,
    /// Wall-clock limit of a run when the budget sets none (spec §7: 60 min).
    pub default_max_wall_clock: Duration,
    /// Max characters of one tool result sent to the model.
    pub tool_result_chars: usize,
    /// Token budget of the initial context bundle.
    pub context_token_budget: u32,
    /// Timeout of one `run_command`.
    pub command_timeout: Duration,
    /// Max output tokens per model turn.
    pub max_output_tokens: u32,
    /// Max concurrently running agents (all projects).
    pub max_running_agents: usize,
    /// The user's UI language: agents write messages for the user in it
    /// (shared with the core, which updates it from settings / the UI).
    pub ui_language: crate::language::UiLanguage,
    /// Late-bound provider of plugin tools (Phase 7, ADR-0034; shared like `ui_language`).
    pub external_tools: crate::tools::external::ExternalToolsSlot,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            approval_timeout: Duration::from_secs(24 * 3600),
            default_max_wall_clock: Duration::from_secs(60 * 60),
            tool_result_chars: 16_000,
            context_token_budget: 6_000,
            command_timeout: Duration::from_secs(10 * 60),
            max_output_tokens: 8_192,
            max_running_agents: 16,
            ui_language: crate::language::UiLanguage::default(),
            external_tools: crate::tools::external::ExternalToolsSlot::default(),
        }
    }
}

/// Everything an agent run touches. Engines only — never the core.
pub struct RuntimeDeps {
    /// Database (agent tables).
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Security subsystem (permission engine, approvals, emergency tokens, rules).
    pub security: Arc<Security>,
    /// Task engine.
    pub tasks: Arc<orkestr_task_engine::TaskService>,
    /// Projects.
    pub projects: Arc<dyn ProjectRepository>,
    /// Git / worktrees.
    pub vcs: orkestr_vcs::Vcs,
    /// Snapshots.
    pub snapshots: Arc<orkestr_snapshots::SnapshotService>,
    /// Terminal (commands go through its security gate).
    pub terminals: orkestr_terminal::TerminalService,
    /// Memory / decisions.
    pub memory: orkestr_memory::MemoryEngine,
    /// Usage + budgets.
    pub cost: orkestr_cost::CostEngine,
    /// Provider hub.
    pub models: Arc<dyn ModelGateway>,
    /// Tunables.
    pub config: RuntimeConfig,
}

impl std::fmt::Debug for RuntimeDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeps").field("config", &self.config).finish_non_exhaustive()
    }
}

impl RuntimeDeps {
    /// Publishes an event; failures are logged (state is already committed).
    pub async fn publish(&self, ev: NewEvent) {
        if let Err(e) = self.events.publish(ev).await {
            tracing::warn!(error = %e, "failed to publish agent event");
        }
    }
}
