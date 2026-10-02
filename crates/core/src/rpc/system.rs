//! `system.*` handlers.

use orkestr_protocol::methods::system::{
    Checks, Health, HealthResult, Info, ResumeSummary, ResumeSummaryMethod, ResumeSummaryParams, SystemChecksResult,
    SystemInfo,
};
use orkestr_protocol::{RpcError, PROTOCOL_VERSION};

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

async fn checks(ctx: RpcContext) -> Result<SystemChecksResult, RpcError> {
    let platform = ctx.services.platform.clone();
    tokio::task::spawn_blocking(move || crate::checks::run_all(platform.as_ref()))
        .await
        .map_err(|e| RpcError::internal(format!("system checks failed: {e}")))
}

async fn resume_summary(ctx: RpcContext, p: ResumeSummaryParams) -> Result<ResumeSummary, RpcError> {
    let s = &ctx.services;
    let since = p.since.or(s.previous_session_started_at);
    crate::resume::summarize(&s.db, since, p.project_id, s.previous_session_started_at, s.recovered_from_crash)
        .await
        .map_err(storage_to_rpc)
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<Checks, _, _>(|ctx: RpcContext, _| checks(ctx));
    router.method::<ResumeSummaryMethod, _, _>(resume_summary);
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
