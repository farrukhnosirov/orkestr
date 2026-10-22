//! Marketplace (`marketplace.*`, ADR-0040). **Owner: WP2.**
//!
//! Thin layer over [`orkestr_marketplace::Marketplace`]. Agent templates are
//! installed through the existing templates import, plugins and widget
//! packages through `plugins.install` (pending grants go back to the
//! permission prompt).

use async_trait::async_trait;
use orkestr_marketplace::{Installers, MarketplaceError, TemplateSink};
use orkestr_protocol::methods::marketplace::{
    self as m, AddSourceParams, GetEntryParams, InstallEntryParams, InstalledResult, KeyIdParams, KeyResult,
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
            let to = Installers { templates: &templates, plugins: ctx.services.plugins.as_ref() };
            Ok(ctx.services.marketplace.install(p, to).await?)
        })
        .method::<m::Installed, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(InstalledResult { packages: ctx.services.marketplace.installed().await? })
        })
        .method::<m::Uninstall, _, _>(|ctx: RpcContext, p: UninstallEntryParams| async move {
            let templates = CoreTemplates(ctx.services.agents.clone());
            let to = Installers { templates: &templates, plugins: ctx.services.plugins.as_ref() };
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
            Ok(KeyResult { key: ctx.services.marketplace.untrust_key(&p.key_id).await? })
        });
}
