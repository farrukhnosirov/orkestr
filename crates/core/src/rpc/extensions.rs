//! `extensions.*` handlers (ADR-0025) and the event bridge of the extension
//! service.

use std::path::PathBuf;

use orkestr_events::EventBus;
use orkestr_extensions::{ExtError, ExtensionEvents};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::extensions::{
    self as m, ExtensionFileParams, ExtensionIconParams, ExtensionIconResult, ExtensionIdParams,
    ExtensionInstallProgressPayload, ExtensionReadmeResult, ExtensionsChangedPayload, InstallExtensionParams,
    InstallVsixParams, InstalledExtensionResult, LanguageConfigResult, ListExtensionsResult, MarketplaceRef,
    ReadExtensionAssetParams, SearchExtensionsParams, SearchExtensionsResult, SetExtensionEnabledParams,
    SnippetsResult,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError, RpcErrorCode};

use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Search, _, _>(|ctx: RpcContext, p: SearchExtensionsParams| async move {
            if p.query.chars().count() > 200 {
                return Err(RpcError::bad_request("search text is too long"));
            }
            let offset = p.offset.unwrap_or(0);
            let (total, items) = ctx
                .services
                .extensions
                .search(&p.query, offset, p.size.unwrap_or(20), p.sort.unwrap_or_default(), p.category.as_deref())
                .await
                .map_err(to_rpc)?;
            Ok(SearchExtensionsResult { total, offset, items })
        })
        .method::<m::Details, _, _>(|ctx: RpcContext, p: MarketplaceRef| async move {
            ctx.services.extensions.details(&p.namespace, &p.name).await.map_err(to_rpc)
        })
        .method::<m::Icon, _, _>(|ctx: RpcContext, p: ExtensionIconParams| async move {
            let data_url = ctx.services.extensions.registry().icon(&p.url).await.map_err(to_rpc)?;
            Ok(ExtensionIconResult { data_url })
        })
        .method::<m::Install, _, _>(|ctx: RpcContext, p: InstallExtensionParams| async move {
            let extension = ctx
                .services
                .extensions
                .install(&p.namespace, &p.name, p.version.as_deref())
                .await
                .map_err(to_rpc)?;
            Ok(InstalledExtensionResult { extension })
        })
        .method::<m::InstallVsix, _, _>(|ctx: RpcContext, p: InstallVsixParams| async move {
            let path = PathBuf::from(p.path.trim().trim_matches('"'));
            let extension = ctx.services.extensions.install_vsix(&path).await.map_err(to_rpc)?;
            Ok(InstalledExtensionResult { extension })
        })
        .method::<m::List, _, _>(|ctx: RpcContext, _p: Empty| async move {
            Ok(ListExtensionsResult { extensions: ctx.services.extensions.list().await.map_err(to_rpc)? })
        })
        .method::<m::SetEnabled, _, _>(|ctx: RpcContext, p: SetExtensionEnabledParams| async move {
            let extension = ctx.services.extensions.set_enabled(&p.id, p.enabled).await.map_err(to_rpc)?;
            if !p.enabled {
                stop_servers_of(&ctx, &extension.contributions.language_servers);
            }
            Ok(InstalledExtensionResult { extension })
        })
        .method::<m::Uninstall, _, _>(|ctx: RpcContext, p: ExtensionIdParams| async move {
            if let Ok(list) = ctx.services.extensions.list().await {
                if let Some(e) = list.iter().find(|e| e.id == p.id.to_ascii_lowercase()) {
                    stop_servers_of(&ctx, &e.contributions.language_servers);
                }
            }
            ctx.services.extensions.uninstall(&p.id).await.map_err(to_rpc)?;
            Ok(Empty {})
        })
        .method::<m::Readme, _, _>(|ctx: RpcContext, p: ExtensionIdParams| async move {
            Ok(ExtensionReadmeResult { markdown: ctx.services.extensions.readme(&p.id).await.map_err(to_rpc)? })
        })
        .method::<m::Theme, _, _>(|ctx: RpcContext, p: m::ThemeRef| async move {
            ctx.services.extensions.theme(&p).await.map_err(to_rpc)
        })
        .method::<m::IconTheme, _, _>(|ctx: RpcContext, p: m::ThemeRef| async move {
            ctx.services.extensions.icon_theme(&p).await.map_err(to_rpc)
        })
        .method::<m::ReadAsset, _, _>(|ctx: RpcContext, p: ReadExtensionAssetParams| async move {
            ctx.services.extensions.read_asset(&p.extension_id, &p.path).await.map_err(to_rpc)
        })
        .method::<m::LanguageConfig, _, _>(|ctx: RpcContext, p: ExtensionFileParams| async move {
            let config = ctx.services.extensions.language_config(&p.extension_id, &p.path).await.map_err(to_rpc)?;
            Ok(LanguageConfigResult { config })
        })
        .method::<m::Snippets, _, _>(|ctx: RpcContext, p: ExtensionFileParams| async move {
            let snippets = ctx.services.extensions.snippets(&p.extension_id, &p.path).await.map_err(to_rpc)?;
            Ok(SnippetsResult { snippets })
        })
        .method::<m::GetPrefs, _, _>(|ctx: RpcContext, _p: Empty| async move {
            ctx.services.extensions.prefs().await.map_err(to_rpc)
        })
        .method::<m::SetPrefs, _, _>(|ctx: RpcContext, p: m::EditorPrefs| async move {
            ctx.services.extensions.set_prefs(p).await.map_err(to_rpc)
        });
}

/// Stops running language servers that came from a disabled / removed extension.
fn stop_servers_of(ctx: &RpcContext, server_ids: &[String]) {
    for s in ctx.services.lsp.list() {
        if server_ids.contains(&s.server_id) {
            ctx.services.lsp.stop(s.session_id);
        }
    }
}

/// Maps extension errors to RPC errors.
pub fn to_rpc(err: ExtError) -> RpcError {
    match err {
        ExtError::NotFound(m) => RpcError::not_found(m),
        e @ (ExtError::Invalid(_) | ExtError::TooLarge(_)) => RpcError::bad_request(e.to_string()),
        e @ (ExtError::Unsafe(_) | ExtError::Checksum { .. }) => RpcError::forbidden(e.to_string()),
        e @ ExtError::Network(_) => RpcError::new(RpcErrorCode::ProviderError, e.to_string()),
        ExtError::Unsupported(m) => RpcError::bad_request(format!("unsupported: {m}"))
            .with_data(serde_json::json!({ "reason": "unsupported" })),
        e @ ExtError::Conflict(_) => RpcError::conflict(e.to_string()),
        e @ (ExtError::Io(_) | ExtError::Storage(_)) => {
            tracing::warn!(error = %e, "extension operation failed");
            RpcError::internal(format!("extension operation failed: {e}"))
        }
    }
}

/// Publishes extension notifications on the event bus.
#[derive(Debug, Clone)]
pub struct BusExtensionEvents {
    bus: EventBus,
}

impl BusExtensionEvents {
    /// New bridge.
    #[must_use]
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    fn publish(&self, kind: EventKind) {
        let bus = self.bus.clone();
        // Called from async contexts; publishing must not block the installer.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = bus.publish(NewEvent::new(kind)).await {
                    tracing::warn!(error = %e, "failed to publish an extension event");
                }
            });
        }
    }
}

impl ExtensionEvents for BusExtensionEvents {
    fn progress(&self, p: ExtensionInstallProgressPayload) {
        self.publish(EventKind::ExtensionInstallProgress(p));
    }

    fn changed(&self, p: ExtensionsChangedPayload) {
        self.publish(EventKind::ExtensionsChanged(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(to_rpc(ExtError::NotFound("x".into())).code, RpcErrorCode::NotFound);
        assert_eq!(to_rpc(ExtError::Unsafe("x".into())).code, RpcErrorCode::Forbidden);
        assert_eq!(
            to_rpc(ExtError::Checksum { expected: "a".into(), actual: "b".into() }).code,
            RpcErrorCode::Forbidden
        );
        assert_eq!(to_rpc(ExtError::Conflict("x".into())).code, RpcErrorCode::Conflict);
        assert_eq!(to_rpc(ExtError::Network("x".into())).code, RpcErrorCode::ProviderError);
    }
}
