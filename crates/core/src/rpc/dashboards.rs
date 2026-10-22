//! Dashboard Builder (`dashboards.*`, ADR-0034); store in [`crate::dashboards_store`]. **Owner: WP3.**
//!
//! `dashboards.save` validates the whole layout (12-column grid, no overlap,
//! known widgets, per-instance config against the widget's `configSchema`,
//! plugin data bindings via the widgets allowlist) and publishes
//! `DashboardChanged`; `dashboards.delete` publishes it with `deleted: true`.
//! Remote principals restricted to projects see global dashboards and those of
//! their projects only.

use orkestr_domain::ProjectId;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::dashboards::{
    self as m, Dashboard, DashboardChangedPayload, DashboardIdParams, DashboardResult, ListDashboardsParams,
    ListDashboardsResult, SaveDashboardParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{NewEvent, RpcError};

use super::{Principal, RpcContext, RpcRouter};
use crate::dashboards_store::{validate_layout, DashboardWrite, DashboardsStore};
use crate::error::{events_to_rpc, storage_to_rpc};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(list)
        .method::<m::Get, _, _>(|ctx: RpcContext, p: DashboardIdParams| async move {
            let dashboard = store(&ctx).get(p.dashboard_id).await.map_err(storage_to_rpc)?;
            ensure_scope(&ctx, dashboard.project_id)?;
            Ok(DashboardResult { dashboard })
        })
        .method::<m::Save, _, _>(save)
        .method::<m::Delete, _, _>(delete);
}

fn store(ctx: &RpcContext) -> DashboardsStore {
    DashboardsStore::new(ctx.services.db.clone())
}

/// Remote callers limited to projects may only touch global dashboards and
/// those of their projects.
fn ensure_scope(ctx: &RpcContext, project: Option<ProjectId>) -> Result<(), RpcError> {
    if let (Principal::Remote(rp), Some(p)) = (&ctx.principal, project) {
        if !rp.project_ids.is_empty() && !rp.project_ids.contains(&p) {
            return Err(RpcError::not_found("dashboard not found"));
        }
    }
    Ok(())
}

async fn list(ctx: RpcContext, p: ListDashboardsParams) -> Result<ListDashboardsResult, RpcError> {
    ensure_scope(&ctx, p.project_id)?;
    let dashboards = store(&ctx).list(p.project_id, p.include_global).await.map_err(storage_to_rpc)?;
    Ok(ListDashboardsResult { dashboards })
}

async fn ensure_project(ctx: &RpcContext, project_id: ProjectId) -> Result<(), RpcError> {
    ctx.services
        .projects
        .get(project_id)
        .await
        .map_err(storage_to_rpc)?
        .map(|_| ())
        .ok_or_else(|| RpcError::not_found(format!("project {project_id} not found")))
}

async fn publish(ctx: &RpcContext, d: &Dashboard, deleted: bool) -> Result<(), RpcError> {
    let payload = DashboardChangedPayload { dashboard_id: d.id, project_id: d.project_id, deleted, version: d.version };
    let mut ev = NewEvent::new(EventKind::DashboardChanged(payload)).correlation(ctx.correlation_id);
    if let Some(p) = d.project_id {
        ev = ev.project(p);
    }
    ctx.services.events.publish(ev).await.map(|_| ()).map_err(events_to_rpc)
}

async fn save(ctx: RpcContext, p: SaveDashboardParams) -> Result<DashboardResult, RpcError> {
    let s = store(&ctx);
    let previous = match p.dashboard_id {
        Some(id) => Some(s.get(id).await.map_err(storage_to_rpc)?),
        None => None,
    };
    let scope = previous.as_ref().map_or(p.project_id, |d| d.project_id);
    ensure_scope(&ctx, scope)?;
    if previous.is_none() {
        if let Some(pid) = p.project_id {
            ensure_project(&ctx, pid).await?;
        }
    }
    let catalog = super::widgets::catalog_map(&ctx.services).await?;
    validate_layout(&p.widgets, &catalog, previous.as_ref().map(|d| d.widgets.as_slice())).map_err(storage_to_rpc)?;
    let dashboard = s
        .save(DashboardWrite {
            id: p.dashboard_id,
            project_id: scope,
            name: p.name,
            widgets: p.widgets,
            expected_version: p.expected_version,
        })
        .await
        .map_err(storage_to_rpc)?;
    publish(&ctx, &dashboard, false).await?;
    Ok(DashboardResult { dashboard })
}

async fn delete(ctx: RpcContext, p: DashboardIdParams) -> Result<Empty, RpcError> {
    let s = store(&ctx);
    let existing = s.get(p.dashboard_id).await.map_err(storage_to_rpc)?;
    ensure_scope(&ctx, existing.project_id)?;
    let removed = s.delete(p.dashboard_id).await.map_err(storage_to_rpc)?;
    publish(&ctx, &removed, true).await?;
    Ok(Empty {})
}
