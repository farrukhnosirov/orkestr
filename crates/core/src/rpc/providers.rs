//! `providers.*` handlers (§4, §5, §7, §55) plus the core-side wiring of the
//! provider hub: the event-bus sink and the credential bridge to the
//! security area's secret vault.
//!
//! API keys are accepted only inbound, handed to the vault through
//! [`CredentialResolver`] and never returned, logged or emitted.

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{Actor, ProviderId, SecretRefId, SecretScope};
use orkestr_events::EventBus;
use orkestr_protocol::event::{
    EventKind, ProviderFallbackPayload, ProviderRateLimitedPayload, ProviderStatusChangedPayload,
};
use orkestr_protocol::methods::providers::{
    self as m, CreateProviderParams, DeleteProviderResult, ListProvidersResult, ManagerConfigPayload,
    ProviderCostModel, ProviderIdParams, ProviderKindInfo, ProviderKindsResult, ProviderLocality, ProviderModelsParams,
    ProviderModelsResult, ProviderStatusResult, ProviderView, ProviderViewResult, RoutingConfigPayload,
    SetCredentialParams, TestProviderParams, TestProviderResult, UpdateProviderParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError, RpcErrorCode};
use orkestr_providers::adapters::cli::resolve_binary;
use orkestr_providers::{
    kind_info, CostModelKind, CredentialError, CredentialResolver, Locality, NewProvider, ProviderError, ProviderEvent,
    ProviderEventSink, ProviderPatch, RegistryError, PHASE1_KINDS, PRICING_TABLE_DATE,
};

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _p: Empty| async move {
            Ok(ListProvidersResult { providers: ctx.services.providers.list().into_iter().map(view).collect() })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: ProviderIdParams| async move {
            Ok(ProviderViewResult { provider: view(ctx.services.providers.get(p.provider_id).map_err(to_rpc)?) })
        })
        .method::<m::Create, _, _>(create)
        .method::<m::Update, _, _>(update)
        .method::<m::Delete, _, _>(|ctx: RpcContext, p: ProviderIdParams| async move {
            ctx.services.providers.delete(p.provider_id).await.map_err(to_rpc)?;
            Ok(DeleteProviderResult { provider_id: p.provider_id })
        })
        .method::<m::SetCredential, _, _>(|ctx: RpcContext, p: SetCredentialParams| async move {
            let v = ctx.services.providers.set_credential(p.provider_id, &p.api_key).await.map_err(to_rpc)?;
            Ok(ProviderViewResult { provider: view(v) })
        })
        .method::<m::ClearCredential, _, _>(|ctx: RpcContext, p: ProviderIdParams| async move {
            let v = ctx.services.providers.clear_credential(p.provider_id).await.map_err(to_rpc)?;
            Ok(ProviderViewResult { provider: view(v) })
        })
        .method::<m::Health, _, _>(|ctx: RpcContext, p: ProviderIdParams| async move {
            let status = ctx.services.providers.health_check(p.provider_id).await.map_err(to_rpc)?;
            Ok(ProviderStatusResult { provider_id: p.provider_id, status })
        })
        .method::<m::Models, _, _>(|ctx: RpcContext, p: ProviderModelsParams| async move {
            let o = ctx.services.providers.models(p.provider_id, p.refresh.unwrap_or(false)).await.map_err(to_rpc)?;
            Ok(ProviderModelsResult {
                provider_id: p.provider_id,
                models: o.models,
                cached: o.cached,
                fetched_at: o.fetched_at,
            })
        })
        .method::<m::Test, _, _>(test)
        .method::<m::Kinds, _, _>(|_ctx: RpcContext, _p: Empty| async move { Ok(kinds()) })
        .method::<m::GetManager, _, _>(|ctx: RpcContext, _p: Empty| async move {
            Ok(ManagerConfigPayload { config: ctx.services.providers.manager_config().await.map_err(to_rpc)? })
        })
        .method::<m::SetManager, _, _>(|ctx: RpcContext, p: ManagerConfigPayload| async move {
            Ok(ManagerConfigPayload {
                config: ctx.services.providers.set_manager_config(p.config).await.map_err(to_rpc)?,
            })
        })
        .method::<m::GetRouting, _, _>(|ctx: RpcContext, _p: Empty| async move {
            Ok(RoutingConfigPayload { config: ctx.services.providers.routing_config().await.map_err(to_rpc)? })
        })
        .method::<m::SetRouting, _, _>(|ctx: RpcContext, p: RoutingConfigPayload| async move {
            Ok(RoutingConfigPayload {
                config: ctx.services.providers.set_routing_config(p.config).await.map_err(to_rpc)?,
            })
        });
}

/// Maps a provider error onto the wire (spec §4: `RateLimited` → `rate_limited`,
/// everything else → `provider_error` with `data.class`).
#[must_use]
pub fn provider_error_to_rpc(e: &ProviderError) -> RpcError {
    let data = serde_json::json!({ "class": e.class() });
    match e {
        ProviderError::RateLimited { retry_after_secs } => RpcError::new(RpcErrorCode::RateLimited, e.to_string())
            .with_data(serde_json::json!({ "class": e.class(), "retryAfterSecs": retry_after_secs })),
        _ => RpcError::new(RpcErrorCode::ProviderError, e.to_string()).with_data(data),
    }
}

fn to_rpc(e: RegistryError) -> RpcError {
    match e {
        RegistryError::NotFound(m) => RpcError::not_found(m),
        RegistryError::Invalid(m) => RpcError::bad_request(m),
        RegistryError::Conflict(m) => RpcError::conflict(m),
        RegistryError::Storage(s) => storage_to_rpc(s),
        RegistryError::Provider(p) => provider_error_to_rpc(&p),
        RegistryError::Credential(c) => {
            tracing::error!(error = %c, "secret vault failure");
            RpcError::internal(format!("secret vault: {c}"))
        }
    }
}

fn view(v: orkestr_providers::ProviderView) -> ProviderView {
    let d = v.descriptor;
    ProviderView {
        provider: v.provider,
        status: v.status,
        has_credential: v.has_credential,
        capabilities: d.capabilities,
        max_context_tokens: d.max_context_tokens,
        locality: match d.locality {
            Locality::Local => ProviderLocality::Local,
            Locality::SelfHosted => ProviderLocality::SelfHosted,
            Locality::Cloud => ProviderLocality::Cloud,
        },
        cost_model: match d.cost_model {
            CostModelKind::PerToken => ProviderCostModel::PerToken,
            CostModelKind::Subscription => ProviderCostModel::Subscription,
            CostModelKind::Local => ProviderCostModel::Local,
            CostModelKind::Unknown => ProviderCostModel::Unknown,
        },
        coding_agent: d.coding_agent,
    }
}

fn kinds() -> ProviderKindsResult {
    let kinds = PHASE1_KINDS
        .iter()
        .map(|k| {
            let i = kind_info(*k);
            ProviderKindInfo {
                kind: i.kind,
                display_name: i.display_name.to_owned(),
                description: i.description.to_owned(),
                auth_method: i.auth_method,
                requires_api_key: i.requires_api_key,
                api_key_optional: i.api_key_optional,
                supports_base_url: i.supports_base_url,
                default_base_url: i.default_base_url.map(str::to_owned),
                supports_custom_headers: i.supports_custom_headers,
                is_local: i.is_local,
                coding_agent: i.coding_agent,
                binary: i.binary.map(str::to_owned),
                detected: i.binary.map(|b| resolve_binary(b, None).is_some()),
                sign_in_note: i.sign_in_note.map(str::to_owned),
                login_instructions: i.login_instructions.map(str::to_owned),
                default_model: i.default_model.map(str::to_owned),
            }
        })
        .collect();
    ProviderKindsResult { kinds, pricing_table_date: PRICING_TABLE_DATE.to_owned() }
}

async fn create(ctx: RpcContext, p: CreateProviderParams) -> Result<ProviderViewResult, RpcError> {
    let v = ctx
        .services
        .providers
        .create(NewProvider {
            kind: Some(p.kind),
            name: p.name,
            base_url: p.base_url,
            headers: p.headers,
            default_model: p.default_model,
            enabled: p.enabled,
            api_key: p.api_key,
            binary_path: p.binary_path,
        })
        .await
        .map_err(to_rpc)?;
    Ok(ProviderViewResult { provider: view(v) })
}

async fn update(ctx: RpcContext, p: UpdateProviderParams) -> Result<ProviderViewResult, RpcError> {
    let patch = ProviderPatch {
        name: p.patch.name,
        base_url: p.patch.base_url,
        headers: p.patch.headers,
        default_model: p.patch.default_model,
        enabled: p.patch.enabled,
        binary_path: p.patch.binary_path,
    };
    let v = ctx.services.providers.update(p.provider_id, patch).await.map_err(to_rpc)?;
    Ok(ProviderViewResult { provider: view(v) })
}

async fn test(ctx: RpcContext, p: TestProviderParams) -> Result<TestProviderResult, RpcError> {
    let o = ctx.services.providers.test(p.provider_id, p.model).await.map_err(to_rpc)?;
    if let Some(u) = o.usage {
        let kind = ctx.services.providers.adapter(p.provider_id).map(|a| a.descriptor().cost_model);
        let record =
            usage_record(p.provider_id, kind.as_ref(), &o.model, &u, UsageScope::default(), Some(o.latency_ms), o.ok);
        record_usage(&ctx.services, &record).await;
    }
    Ok(TestProviderResult {
        ok: o.ok,
        model: o.model,
        latency_ms: o.latency_ms,
        sample: o.sample,
        error_class: o.error.as_ref().map(|e| e.class().to_owned()),
        error_message: o.error.as_ref().map(|e| crate::redact::redact(&e.to_string())),
        input_tokens: o.usage.map(|u| u.input_tokens),
        output_tokens: o.usage.map(|u| u.output_tokens),
        cost_usd: o.cost_usd,
    })
}

/// Who a provider call was made for (all optional).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageScope {
    /// Project.
    pub project_id: Option<orkestr_domain::ProjectId>,
    /// Task.
    pub task_id: Option<orkestr_domain::TaskId>,
    /// Agent.
    pub agent_id: Option<orkestr_domain::AgentId>,
}

/// Builds the cost-engine record of one provider call. The cost is the
/// provider/CLI-reported amount when present, otherwise a list-price
/// estimate (`estimated = true`), or 0 when the model has no known price.
/// On a subscription / local instance (`cost_model`) that amount is only
/// API-equivalent: the spend is 0.
#[must_use]
pub fn usage_record(
    provider_id: ProviderId,
    cost_model: Option<&orkestr_providers::CostModelKind>,
    model: &str,
    usage: &orkestr_providers::Usage,
    scope: UsageScope,
    latency_ms: Option<u64>,
    success: bool,
) -> orkestr_domain::UsageRecord {
    let reported = usage.reported_cost_usd.is_some();
    let priced = orkestr_providers::call_cost(cost_model, model, usage);
    orkestr_domain::UsageRecord {
        id: orkestr_domain::UsageRecordId::new(),
        ts: chrono::Utc::now(),
        provider_id,
        model: model.to_owned(),
        project_id: scope.project_id,
        task_id: scope.task_id,
        agent_id: scope.agent_id,
        execution_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cost_usd: priced.spend_usd,
        api_equivalent_usd: priced.api_equivalent_usd,
        estimated: !reported || usage.estimated,
        latency_ms,
        success,
    }
}

/// Feeds a provider call into the cost engine (usage + budget thresholds).
/// Failures are logged: accounting must never break the call itself.
pub async fn record_usage(services: &crate::services::Services, record: &orkestr_domain::UsageRecord) {
    if let Err(e) = services.cost.ingest(record).await {
        tracing::warn!(error = %e, provider = %record.provider_id, "failed to record provider usage");
    }
}

/// Publishes provider events on the bus (`ProviderStatusChanged`,
/// `ProviderRateLimited`, `ProviderFallback`).
#[derive(Clone)]
pub struct BusProviderSink {
    events: EventBus,
}

impl BusProviderSink {
    /// Wraps the bus.
    #[must_use]
    pub fn new(events: EventBus) -> Self {
        Self { events }
    }
}

impl ProviderEventSink for BusProviderSink {
    fn emit(&self, event: ProviderEvent) {
        let kind = match event {
            ProviderEvent::StatusChanged { provider_id, health, auth_state, detail } => {
                EventKind::ProviderStatusChanged(ProviderStatusChangedPayload {
                    provider_id,
                    health,
                    auth_state,
                    detail: detail.map(|d| crate::redact::redact(&d)),
                })
            }
            ProviderEvent::RateLimited { provider_id, model, retry_after_secs } => {
                EventKind::ProviderRateLimited(ProviderRateLimitedPayload { provider_id, model, retry_after_secs })
            }
            ProviderEvent::Fallback { from, to, reason } => {
                EventKind::ProviderFallback(ProviderFallbackPayload { from, to, reason })
            }
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("provider event dropped: no tokio runtime");
            return;
        };
        let bus = self.events.clone();
        handle.spawn(async move {
            if let Err(e) = bus.publish(NewEvent::new(kind)).await {
                tracing::warn!(error = %e, "failed to publish provider event");
            }
        });
    }
}

/// Credential bridge over the security area's [`SecretService`]: provider
/// API keys live in the OS keyring (encrypted-file vault fallback), every
/// access is audited, values are registered with the redactor, and the vault
/// lock / emergency controls apply. Only the `SecretRefId` is stored by the
/// provider registry.
///
/// Each stored key gets a unique secret name (`provider.<id>.<nonce>`) so a
/// rotation (store new → delete old) never deletes the freshly stored value.
pub struct SecretServiceCredentials {
    secrets: Arc<orkestr_security::SecretService>,
}

impl SecretServiceCredentials {
    /// Wraps the secret service.
    #[must_use]
    pub fn new(secrets: Arc<orkestr_security::SecretService>) -> Self {
        Self { secrets }
    }
}

fn credential_error(e: orkestr_security::SecurityError) -> CredentialError {
    match e {
        orkestr_security::SecurityError::VaultLocked => CredentialError::Locked,
        orkestr_security::SecurityError::SecretNotFound(_) | orkestr_security::SecurityError::NotFound(_) => {
            CredentialError::NotFound
        }
        other => CredentialError::Backend(other.to_string()),
    }
}

#[async_trait]
impl CredentialResolver for SecretServiceCredentials {
    async fn store(&self, provider: ProviderId, _label: &str, secret: &str) -> Result<SecretRefId, CredentialError> {
        let name = format!("provider.{provider}.{}", SecretRefId::new());
        let reference = self
            .secrets
            .set(&name, SecretScope::Global, zeroize::Zeroizing::new(secret.to_owned()), Actor::System)
            .await
            .map_err(credential_error)?;
        tracing::info!(%provider, secret_ref = %reference.id, "provider secret stored in the secret vault");
        Ok(reference.id)
    }

    async fn resolve(&self, reference: SecretRefId) -> Result<String, CredentialError> {
        let value = self
            .secrets
            .resolve(reference, Actor::System, orkestr_security::AccessContext::default())
            .await
            .map_err(credential_error)?;
        Ok(value.as_str().to_owned())
    }

    async fn delete(&self, reference: SecretRefId) -> Result<(), CredentialError> {
        self.secrets.delete(reference, Actor::System).await.map(|_| ()).map_err(credential_error)
    }
}
