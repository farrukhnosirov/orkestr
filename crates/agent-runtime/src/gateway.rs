//! The runtime's view of the provider hub (§60.5: no provider-specific logic
//! here). Production uses [`RegistryGateway`] over
//! [`orkestr_providers::ProviderRegistry`]; tests plug in scripted adapters.

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{ModelRef, ProviderId, ProviderKind};
use orkestr_providers::{ChatRequest, ChatStream, HandoffHook, ProviderAdapter, ProviderRegistry, RouteRequest};
use tokio_util::sync::CancellationToken;

/// Cost of one call on `model`, split by its provider's cost model: a
/// subscription / local instance spends $0 (its reported or list-price cost
/// is only an API-equivalent value, provider-adapter-spec §8).
#[must_use]
pub fn call_cost(
    models: &dyn ModelGateway,
    model: &ModelRef,
    usage: &orkestr_providers::Usage,
) -> orkestr_providers::CallCost {
    let kind = models.adapter(model.provider_id).map(|a| a.descriptor().cost_model);
    orkestr_providers::call_cost(kind.as_ref(), &model.model, usage)
}

/// List prices of `model` for pre-flight spend estimates: `None` when the
/// model has no known price or its instance is not metered in $
/// (subscription / local — the call spends nothing).
#[must_use]
pub fn metered_pricing(models: &dyn ModelGateway, model: &ModelRef) -> Option<orkestr_domain::ModelPricing> {
    let unmetered = models.adapter(model.provider_id).is_some_and(|a| {
        matches!(
            a.descriptor().cost_model,
            orkestr_providers::CostModelKind::Subscription | orkestr_providers::CostModelKind::Local
        )
    });
    if unmetered {
        return None;
    }
    orkestr_providers::pricing_for(&model.model)
}

/// What the runtime needs to know about a provider instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Adapter family.
    pub kind: ProviderKind,
    /// Runs on a machine the user controls (privacy routing).
    pub is_local: bool,
    /// Enabled in settings.
    pub enabled: bool,
}

impl ProviderInfo {
    /// Official CLI coding agents (Claude Code, Codex) run their own tool loop.
    #[must_use]
    pub fn is_cli_agent(&self) -> bool {
        matches!(self.kind, ProviderKind::ClaudeCode | ProviderKind::Codex)
    }
}

/// Model routing + streaming chat with fallback.
#[async_trait]
pub trait ModelGateway: Send + Sync {
    /// Ordered models to try for a request (head first); empty = nothing usable.
    async fn route(&self, req: &RouteRequest) -> Result<Vec<ModelRef>, String>;
    /// Streams a chat over `chain` with fallback; `ChatEvent::Switched` marks
    /// every model switch, `hook` builds the structured handoff.
    fn chat(
        &self,
        chain: Vec<ModelRef>,
        request: ChatRequest,
        hook: Option<Arc<dyn HandoffHook>>,
        cancel: CancellationToken,
    ) -> ChatStream;
    /// Provider facts.
    fn provider(&self, id: ProviderId) -> Option<ProviderInfo>;
    /// The adapter (CLI coding-agent runs).
    fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>>;
    /// Context window (tokens) of a model, when known (model-aware context
    /// budgeting, §20). Phase 2, additive.
    fn context_window(&self, _model: &ModelRef) -> Option<u32> {
        None
    }
    /// The model explicitly configured for a routing purpose (a routing rule),
    /// e.g. the cheap `Summarize` model for handoff compression. Phase 2, additive.
    async fn configured_model(&self, _purpose: &str) -> Option<ModelRef> {
        None
    }
}

/// Production gateway over the provider registry.
#[derive(Clone)]
pub struct RegistryGateway(pub Arc<ProviderRegistry>);

impl std::fmt::Debug for RegistryGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistryGateway")
    }
}

#[async_trait]
impl ModelGateway for RegistryGateway {
    async fn route(&self, req: &RouteRequest) -> Result<Vec<ModelRef>, String> {
        self.0.route(req).await.map_err(|e| e.to_string())
    }

    fn chat(
        &self,
        chain: Vec<ModelRef>,
        request: ChatRequest,
        hook: Option<Arc<dyn HandoffHook>>,
        cancel: CancellationToken,
    ) -> ChatStream {
        self.0.chat(chain, request, hook, cancel)
    }

    fn provider(&self, id: ProviderId) -> Option<ProviderInfo> {
        self.0.get(id).ok().map(|v| ProviderInfo {
            kind: v.provider.kind,
            is_local: v.provider.is_local,
            enabled: v.provider.enabled,
        })
    }

    fn adapter(&self, id: ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.0.adapter(id)
    }

    fn context_window(&self, model: &ModelRef) -> Option<u32> {
        self.0.model_context_window(model)
    }

    async fn configured_model(&self, purpose: &str) -> Option<ModelRef> {
        let cfg = self.0.routing_config().await.ok()?;
        cfg.rule_for(purpose).map(|r| r.model.clone())
    }
}
