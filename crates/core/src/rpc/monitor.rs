//! Infrastructure monitor (`monitor.*`, ADR-0042). **Owner: WP5.**
//!
//! Thin adapters over [`orkestr_monitor::MonitorService`]. Every handler
//! calls `start()` first (idempotent): it loads persisted state and spawns
//! the sampler / scheduler loops if the boot sequence has not done so yet.

use orkestr_monitor::MonitorService;
use orkestr_protocol::methods::monitor::{
    self as m, AlertResult, AlertsResult, ChecksListResult, HealthCheckResult, IngestResult, LogSourcesResult,
    MetricsResult,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};

async fn svc(ctx: &RpcContext) -> Result<&MonitorService, RpcError> {
    let s = ctx.services.monitor.as_ref();
    s.start().await?;
    Ok(s)
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Overview, _, _>(
            |ctx: RpcContext, p| async move { Ok(svc(&ctx).await?.overview(p.project_id).await?) },
        )
        .method::<m::Metrics, _, _>(|ctx: RpcContext, p| async move {
            let s = svc(&ctx).await?;
            Ok(MetricsResult { series: s.metrics(p.node_id, &p.metrics, p.since, p.resolution_secs) })
        })
        .method::<m::ChecksList, _, _>(|ctx: RpcContext, p| async move {
            Ok(ChecksListResult { checks: svc(&ctx).await?.checks(p.project_id) })
        })
        .method::<m::CheckUpsert, _, _>(|ctx: RpcContext, p| async move {
            Ok(HealthCheckResult { check: svc(&ctx).await?.upsert_check(p).await? })
        })
        .method::<m::CheckDelete, _, _>(|ctx: RpcContext, p| async move {
            svc(&ctx).await?.delete_check(p.check_id).await?;
            Ok(Empty {})
        })
        .method::<m::CheckRun, _, _>(|ctx: RpcContext, p| async move {
            Ok(HealthCheckResult { check: svc(&ctx).await?.run_check(p.check_id).await? })
        })
        .method::<m::LogSources, _, _>(|ctx: RpcContext, p| async move {
            let monitor = svc(&ctx).await?;
            monitor.refresh_deployment_sources().await;
            Ok(LogSourcesResult { sources: monitor.log_sources(p.project_id) })
        })
        .method::<m::Logs, _, _>(|ctx: RpcContext, p| async move { Ok(svc(&ctx).await?.logs(&p).await?) })
        .method::<m::Ingest, _, _>(|ctx: RpcContext, p| async move {
            Ok(IngestResult { accepted: svc(&ctx).await?.ingest(p).await? })
        })
        .method::<m::Alerts, _, _>(|ctx: RpcContext, p| async move {
            Ok(AlertsResult { alerts: svc(&ctx).await?.alerts(p.project_id, p.include_resolved, p.limit).await? })
        })
        .method::<m::AckAlert, _, _>(|ctx: RpcContext, p| async move {
            Ok(AlertResult { alert: svc(&ctx).await?.ack_alert(p.alert_id).await? })
        });
}
