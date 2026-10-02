//! Services the Manager is wired with (by the core's composition root), plus
//! the two small seams the core implements: [`ManagerHub`] (AI Manager
//! settings and provider facts) and [`ResourceProbe`] (host resources).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_agent_runtime::{AgentRuntime, ModelGateway};
use orkestr_domain::{ManagerConfig, ModelRef, ProviderHealth};
use orkestr_events::EventBus;
use orkestr_protocol::NewEvent;
use orkestr_providers::ProviderRegistry;
use orkestr_security::Security;
use orkestr_storage::{Db, ProjectRepository};
use orkestr_task_engine::ResourceSnapshot;

/// One provider as the Manager's `provider_status` tool reports it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBrief {
    /// Provider id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Kind (`OpenAi`, `Mock`, …).
    pub kind: String,
    /// Enabled.
    pub enabled: bool,
    /// Runs locally.
    pub is_local: bool,
    /// Health.
    pub health: ProviderHealth,
    /// Auth state.
    pub auth: String,
    /// Default model.
    pub default_model: Option<String>,
    /// Max concurrent requests.
    pub max_concurrency: u32,
}

/// AI Manager settings + provider facts (implemented over the provider registry).
#[async_trait]
pub trait ManagerHub: Send + Sync {
    /// Current AI Manager settings (`providers.getManager`).
    async fn manager_config(&self) -> Result<ManagerConfig, String>;
    /// Configured providers.
    fn providers(&self) -> Vec<ProviderBrief>;
    /// Whether a model supports provider-enforced structured output.
    fn supports_structured_output(&self, model: &ModelRef) -> bool;
    /// Concurrency slots per provider id (scheduler `provider_slots`).
    fn provider_slots(&self) -> BTreeMap<String, usize>;
}

/// Production hub over [`ProviderRegistry`].
#[derive(Clone)]
pub struct RegistryHub(pub Arc<ProviderRegistry>);

impl std::fmt::Debug for RegistryHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistryHub")
    }
}

#[async_trait]
impl ManagerHub for RegistryHub {
    async fn manager_config(&self) -> Result<ManagerConfig, String> {
        self.0.manager_config().await.map_err(|e| e.to_string())
    }

    fn providers(&self) -> Vec<ProviderBrief> {
        self.0
            .list()
            .into_iter()
            .map(|v| ProviderBrief {
                id: v.provider.id.to_string(),
                name: v.provider.name.clone(),
                kind: format!("{:?}", v.provider.kind),
                enabled: v.provider.enabled,
                is_local: v.provider.is_local,
                health: v.status.health,
                auth: format!("{:?}", v.status.auth_state),
                default_model: v.provider.default_model.clone(),
                max_concurrency: v.descriptor.max_concurrency,
            })
            .collect()
    }

    fn supports_structured_output(&self, model: &ModelRef) -> bool {
        self.0
            .candidates(std::slice::from_ref(model))
            .iter()
            .find(|c| &c.model == model)
            .is_some_and(|c| c.capabilities.structured_output)
    }

    fn provider_slots(&self) -> BTreeMap<String, usize> {
        self.0
            .list()
            .into_iter()
            .filter(|v| v.descriptor.max_concurrency > 0)
            .map(|v| (v.provider.id.to_string(), v.descriptor.max_concurrency as usize))
            .collect()
    }
}

/// Host resources for admission control (§11).
pub trait ResourceProbe: Send + Sync {
    /// Latest snapshot.
    fn snapshot(&self) -> ResourceSnapshot;
}

/// A probe that never limits (tests, unknown hosts).
#[derive(Debug, Clone, Copy, Default)]
pub struct Unconstrained;

impl ResourceProbe for Unconstrained {
    fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot::unconstrained()
    }
}

/// Tunables.
#[derive(Debug, Clone)]
pub struct ManagerTunables {
    /// Confinement root for permission checks of Orkestr state reads (the
    /// data directory): state tools read metadata, not project files.
    pub state_root: PathBuf,
    /// Supervisor wake-up interval when no event arrives.
    pub tick: Duration,
    /// Max model turns of one chat reply (tool loop bound).
    pub chat_max_turns: u32,
    /// Conversation messages sent to the model (older ones are summarised).
    pub history_messages: usize,
    /// How long a Manager approval request (Ask policy) may stay pending.
    pub approval_timeout: Duration,
    /// Default total agent cap of a run.
    pub default_max_agents: u32,
    /// Default consecutive failure cap.
    pub default_max_consecutive_failures: u32,
    /// Default attempts per task.
    pub default_max_attempts: u32,
    /// Max output tokens of a Manager turn.
    pub max_output_tokens: u32,
    /// Max characters of one tool result given to the model.
    pub tool_result_chars: usize,
    /// Files listed in the planning context.
    pub planning_files: usize,
    /// The user's UI language (shared with the agent runtime and the core):
    /// the Manager writes for the user in it.
    pub ui_language: orkestr_agent_runtime::UiLanguage,
}

impl Default for ManagerTunables {
    fn default() -> Self {
        Self {
            state_root: std::env::temp_dir(),
            tick: Duration::from_millis(1000),
            chat_max_turns: 8,
            history_messages: 30,
            approval_timeout: Duration::from_secs(30 * 60),
            default_max_agents: 32,
            default_max_consecutive_failures: 3,
            default_max_attempts: 2,
            max_output_tokens: 8_192,
            tool_result_chars: 12_000,
            planning_files: 60,
            ui_language: orkestr_agent_runtime::UiLanguage::default(),
        }
    }
}

/// Everything the Manager touches. Engines only — never the core.
pub struct OrchestratorDeps {
    /// Database (manager tables).
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Security (permission engine, approvals, emergency tokens).
    pub security: Arc<Security>,
    /// Task engine.
    pub tasks: Arc<orkestr_task_engine::TaskService>,
    /// Projects.
    pub projects: Arc<dyn ProjectRepository>,
    /// Git.
    pub vcs: orkestr_vcs::Vcs,
    /// Snapshots.
    pub snapshots: Arc<orkestr_snapshots::SnapshotService>,
    /// Memory / decisions.
    pub memory: orkestr_memory::MemoryEngine,
    /// Usage + budgets.
    pub cost: orkestr_cost::CostEngine,
    /// Agent runtime (spawn / stop / pause).
    pub agents: AgentRuntime,
    /// Model routing + streaming chat with fallback.
    pub models: Arc<dyn ModelGateway>,
    /// Manager settings + provider facts.
    pub hub: Arc<dyn ManagerHub>,
    /// Host resources.
    pub resources: Arc<dyn ResourceProbe>,
    /// Tunables.
    pub config: ManagerTunables,
}

impl std::fmt::Debug for OrchestratorDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorDeps").field("config", &self.config).finish_non_exhaustive()
    }
}

impl OrchestratorDeps {
    /// Publishes an event; failures are logged (state is already committed).
    pub async fn publish(&self, ev: NewEvent) {
        if let Err(e) = self.events.publish(ev).await {
            tracing::warn!(error = %e, "failed to publish manager event");
        }
    }
}
