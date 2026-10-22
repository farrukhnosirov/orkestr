//! Plugin SDK host (`plugins.*`, ADR-0034). **Owner: WP1.**
//!
//! Thin handlers over [`orkestr_plugins::PluginService`]. Remote callers are
//! restricted by `MethodPolicy` in dispatch (install / grant / enable / safe
//! mode are local-only; disable / killAll are emergency class).

use orkestr_protocol::methods::plugins::{
    self as m, GrantPluginParams, InstallPluginParams, InstallPluginResult, InvokePluginParams, InvokePluginResult,
    KillAllPluginsParams, KillAllPluginsResult, ListPluginsParams, ListPluginsResult, PluginIdParams, PluginLogsParams,
    PluginLogsResult, PluginResult, PluginStateParams, SafeModeResult, SetSafeModeParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};

fn reason(p: Option<String>, default: &str) -> String {
    p.map(|r| r.trim().chars().take(500).collect::<String>())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| default.into())
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListPluginsParams| async move {
            let svc = &ctx.services.plugins;
            let mut plugins = svc.list().await.map_err(RpcError::from)?;
            if let Some(kind) = p.kind {
                plugins.retain(|i| i.manifest.kinds.contains(&kind));
            }
            Ok(ListPluginsResult { plugins, safe_mode: svc.safe_mode() })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: PluginIdParams| async move {
            Ok(PluginResult { plugin: ctx.services.plugins.get(p.plugin_id).await.map_err(RpcError::from)? })
        })
        .method::<m::Install, _, _>(|ctx: RpcContext, p: InstallPluginParams| async move {
            let r: InstallPluginResult = ctx.services.plugins.install(p).await.map_err(RpcError::from)?;
            Ok(r)
        })
        .method::<m::Grant, _, _>(|ctx: RpcContext, p: GrantPluginParams| async move {
            Ok(PluginResult { plugin: ctx.services.plugins.grant(p).await.map_err(RpcError::from)? })
        })
        .method::<m::Uninstall, _, _>(|ctx: RpcContext, p: PluginIdParams| async move {
            ctx.services.plugins.uninstall(p.plugin_id).await.map_err(RpcError::from)?;
            Ok(Empty {})
        })
        .method::<m::Enable, _, _>(|ctx: RpcContext, p: PluginStateParams| async move {
            let plugin = ctx
                .services
                .plugins
                .set_enabled(p.plugin_id, true, Some(reason(p.reason, "user")))
                .await
                .map_err(RpcError::from)?;
            Ok(PluginResult { plugin })
        })
        .method::<m::Disable, _, _>(|ctx: RpcContext, p: PluginStateParams| async move {
            let plugin = ctx
                .services
                .plugins
                .set_enabled(p.plugin_id, false, Some(reason(p.reason, "user")))
                .await
                .map_err(RpcError::from)?;
            Ok(PluginResult { plugin })
        })
        .method::<m::KillAll, _, _>(|ctx: RpcContext, p: KillAllPluginsParams| async move {
            let r: KillAllPluginsResult = ctx.services.plugins.kill(None, &reason(p.reason, "kill switch")).await;
            Ok(r)
        })
        .method::<m::SetSafeMode, _, _>(|ctx: RpcContext, p: SetSafeModeParams| async move {
            let safe_mode = ctx.services.plugins.set_safe_mode(p.enabled).await.map_err(RpcError::from)?;
            Ok(SafeModeResult { safe_mode })
        })
        .method::<m::Invoke, _, _>(|ctx: RpcContext, p: InvokePluginParams| async move {
            if p.input.to_string().len() > 1024 * 1024 {
                return Err(RpcError::bad_request("input larger than 1 MiB"));
            }
            let r: InvokePluginResult = ctx.services.plugins.invoke(p).await.map_err(RpcError::from)?;
            Ok(r)
        })
        .method::<m::Logs, _, _>(|ctx: RpcContext, p: PluginLogsParams| async move {
            let lines = ctx.services.plugins.logs(p.plugin_id, p.limit.unwrap_or(200)).await.map_err(RpcError::from)?;
            Ok(PluginLogsResult { lines })
        });
}
