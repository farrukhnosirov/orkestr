//! Marketplace (`marketplace.*`, ADR-0040). **Owner: WP2.**
//!
//! Thin layer over [`orkestr_marketplace::Marketplace`]. Agent templates are
//! installed through the existing templates import, plugins and widget
//! packages through `plugins.install` (pending grants go back to the
//! permission prompt).
//!
//! Trust is unified with the plugin host (ADR-0040): when a plugin package
//! comes from the marketplace and its inner `signature.json` signer is a
//! publisher key the marketplace trusts (official or explicitly trusted),
//! that key is pinned in the plugin trust store (source `marketplace`) so the
//! plugin host accepts it; any other signer is left to the plugin host
//! (refused unless dev mode). `marketplace.untrustKey` removes such a
//! marketplace-sourced plugin trust again.

use async_trait::async_trait;
use std::sync::Arc;

use orkestr_marketplace::{Installers, Marketplace, MarketplaceError, PluginSink, TemplateSink};
use orkestr_plugins::PluginService;
use orkestr_protocol::methods::marketplace::{
    self as m, AddSourceParams, GetEntryParams, InstallEntryParams, InstalledResult, KeyIdParams, KeyResult, KeyTrust,
    KeysResult, MarketplaceSearchParams, MarketplaceSearchResult, PublishPackageParams, PublishPackageResult,
    RefreshSourcesParams, SourceIdParams, SourceResult, SourcesResult, TrustKeyParams, UninstallEntryParams,
};
use orkestr_protocol::methods::Empty;

use super::{RpcContext, RpcRouter};

/// `templates.import` / `templates.delete` for marketplace installs.
struct CoreTemplates(orkestr_agent_runtime::AgentRuntime);

fn agent_err(e: orkestr_agent_runtime::AgentError) -> MarketplaceError {
    use orkestr_agent_runtime::AgentError as A;
    match e {
        A::NotFound(m) => MarketplaceError::NotFound(m),
        A::Invalid(m) => MarketplaceError::Invalid(m),
        A::Forbidden(m) => MarketplaceError::Denied(m),
        other => MarketplaceError::Internal(other.to_string()),
    }
}

#[async_trait]
impl TemplateSink for CoreTemplates {
    async fn import(&self, json: &str, accept_permissions: bool) -> Result<String, MarketplaceError> {
        let (t, _dropped) = self.0.templates().import(json, accept_permissions).await.map_err(agent_err)?;
        Ok(t.id.to_string())
    }

    async fn remove(&self, template_id: &str) -> Result<(), MarketplaceError> {
        let id = orkestr_domain::AgentTemplateId::parse(template_id)
            .map_err(|e| MarketplaceError::Invalid(e.to_string()))?;
        self.0.templates().delete(id).await.map(|_| ()).map_err(agent_err)
    }
}

/// Source tag of plugin trust entries created by marketplace installs.
const MARKETPLACE_TRUST_SOURCE: &str = "marketplace";

/// Plugin installs from the marketplace: pins the package signer in the
/// plugin trust store when the marketplace trusts that publisher key.
struct MarketplacePlugins {
    plugins: Arc<PluginService>,
    marketplace: Arc<Marketplace>,
}

impl MarketplacePlugins {
    async fn pin_signer(&self, bytes: &[u8]) -> Result<(), MarketplaceError> {
        let pkg = orkestr_plugins::package::parse(bytes).map_err(|e| MarketplaceError::Invalid(e.to_string()))?;
        let Some(sig) = pkg.signature.as_ref() else {
            // Unsigned: the plugin host decides (refused unless dev mode).
            return Ok(());
        };
        let store = self.plugins.trust_store();
        if store.get(&sig.key_id).await.ok().flatten().is_some() {
            return Ok(());
        }
        let (keys, _) = self.marketplace.keys().await?;
        // Pin only keys the marketplace trusts; anything else is left to the
        // plugin host (refused unless dev mode).
        let trusted = keys.into_iter().find(|k| {
            k.key_id == sig.key_id
                && k.public_key == sig.public_key
                && matches!(k.trust, KeyTrust::Official | KeyTrust::Trusted)
        });
        if let Some(key) = trusted {
            store
                .trust(&key.public_key, &key.publisher, MARKETPLACE_TRUST_SOURCE)
                .await
                .map_err(|e| MarketplaceError::Internal(e.to_string()))?;
        }
        Ok(())
    }
}

#[async_trait]
impl PluginSink for MarketplacePlugins {
    async fn install(
        &self,
        bytes: &[u8],
        source: String,
    ) -> Result<orkestr_protocol::methods::plugins::InstallPluginResult, MarketplaceError> {
        self.pin_signer(bytes).await?;
        PluginSink::install(self.plugins.as_ref(), bytes, source).await
    }

    async fn uninstall(&self, plugin_id: &str) -> Result<(), MarketplaceError> {
        PluginSink::uninstall(self.plugins.as_ref(), plugin_id).await
    }
}

fn plugin_sink(services: &crate::services::Services) -> MarketplacePlugins {
    MarketplacePlugins { plugins: Arc::clone(&services.plugins), marketplace: Arc::clone(&services.marketplace) }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Sources, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(SourcesResult { sources: ctx.services.marketplace.sources().await? })
        })
        .method::<m::AddSource, _, _>(|ctx: RpcContext, p: AddSourceParams| async move {
            Ok(SourceResult { source: ctx.services.marketplace.add_source(p).await? })
        })
        .method::<m::RemoveSource, _, _>(|ctx: RpcContext, p: SourceIdParams| async move {
            ctx.services.marketplace.remove_source(&p.source_id).await?;
            Ok(Empty {})
        })
        .method::<m::Refresh, _, _>(|ctx: RpcContext, p: RefreshSourcesParams| async move {
            Ok(SourcesResult { sources: ctx.services.marketplace.refresh(p.source_id.as_deref()).await? })
        })
        .method::<m::Search, _, _>(|ctx: RpcContext, p: MarketplaceSearchParams| async move {
            Ok(MarketplaceSearchResult { entries: ctx.services.marketplace.search(p).await? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: GetEntryParams| async move {
            Ok(ctx.services.marketplace.get(&p.id, p.source_id.as_deref()).await?)
        })
        .method::<m::Install, _, _>(|ctx: RpcContext, p: InstallEntryParams| async move {
            let templates = CoreTemplates(ctx.services.agents.clone());
            let plugins = plugin_sink(&ctx.services);
            let to = Installers { templates: &templates, plugins: &plugins };
            Ok(ctx.services.marketplace.install(p, to).await?)
        })
        .method::<m::Installed, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(InstalledResult { packages: ctx.services.marketplace.installed().await? })
        })
        .method::<m::Uninstall, _, _>(|ctx: RpcContext, p: UninstallEntryParams| async move {
            let templates = CoreTemplates(ctx.services.agents.clone());
            let plugins = plugin_sink(&ctx.services);
            let to = Installers { templates: &templates, plugins: &plugins };
            ctx.services.marketplace.uninstall(&p.id, to).await?;
            Ok(Empty {})
        })
        .method::<m::Publish, _, _>(|ctx: RpcContext, p: PublishPackageParams| async move {
            Ok(PublishPackageResult { entry: ctx.services.marketplace.publish(p).await? })
        })
        .method::<m::Keys, _, _>(|ctx: RpcContext, _: Empty| async move {
            let (keys, local_publisher) = ctx.services.marketplace.keys().await?;
            Ok(KeysResult { keys, local_publisher })
        })
        .method::<m::TrustKey, _, _>(|ctx: RpcContext, p: TrustKeyParams| async move {
            Ok(KeyResult { key: ctx.services.marketplace.trust_key(&p.key_id, &p.fingerprint).await? })
        })
        .method::<m::UntrustKey, _, _>(|ctx: RpcContext, p: KeyIdParams| async move {
            let key = ctx.services.marketplace.untrust_key(&p.key_id).await?;
            let store = ctx.services.plugins.trust_store();
            if let Ok(Some(k)) = store.get(&p.key_id).await {
                if k.source == MARKETPLACE_TRUST_SOURCE && !k.builtin {
                    if let Err(e) = store.untrust(&p.key_id).await {
                        tracing::warn!(error = %e, "could not remove the marketplace key from the plugin trust store");
                    }
                }
            }
            Ok(KeyResult { key })
        });
}
