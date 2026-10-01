//! `system.*` handlers.

use orkestr_protocol::methods::system::{Health, HealthResult, Info, SystemInfo};
use orkestr_protocol::PROTOCOL_VERSION;

use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<Health, _, _>(|ctx: RpcContext, _| async move {
        Ok(HealthResult {
            status: "ok".into(),
            version: ctx.services.version.clone(),
            uptime_ms: ctx.services.uptime_ms(),
        })
    });
    router.method::<Info, _, _>(|ctx: RpcContext, _| async move {
        let s = &ctx.services;
        let info = s.platform.info().clone();
        let platform = s.platform.clone();
        let shells = tokio::task::spawn_blocking(move || {
            platform.shells().detect().into_iter().map(|sh| sh.name).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        Ok(SystemInfo {
            version: s.version.clone(),
            protocol_version: PROTOCOL_VERSION,
            node_id: s.node_id,
            os: info.os,
            arch: info.arch,
            hostname: info.hostname,
            pid: std::process::id(),
            data_dir: s.paths.data_dir.display().to_string(),
            started_at: s.started_at,
            recovered_from_crash: s.recovered_from_crash,
            shells,
        })
    });
}
