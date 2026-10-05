//! `ui.device` / `ui.layouts*` handlers — docking layouts per device
//! (§29, §34, ADR-0013). Storage: [`crate::layouts_store`].

use orkestr_protocol::methods::layouts::{
    self as m, DeviceInfoResult, LayoutIdParams, LayoutsDeviceParams, LayoutsState, RenameLayoutParams,
    SaveLayoutParams,
};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;
use crate::layouts_store::LayoutsStore;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Device, _, _>(|ctx: RpcContext, _| device(ctx))
        .method::<m::List, _, _>(list)
        .method::<m::Save, _, _>(save)
        .method::<m::Delete, _, _>(delete)
        .method::<m::Rename, _, _>(rename)
        .method::<m::SetActive, _, _>(set_active);
}

fn store(ctx: &RpcContext) -> LayoutsStore {
    LayoutsStore::new(ctx.services.db.clone())
}

async fn device(ctx: RpcContext) -> Result<DeviceInfoResult, RpcError> {
    let id = store(&ctx).device(None).await.map_err(storage_to_rpc)?;
    Ok(DeviceInfoResult { device_id: id.to_string() })
}

async fn list(ctx: RpcContext, p: LayoutsDeviceParams) -> Result<LayoutsState, RpcError> {
    let s = store(&ctx);
    let dev = s.device(p.device_id.as_deref()).await.map_err(storage_to_rpc)?;
    s.state(dev).await.map_err(storage_to_rpc)
}

async fn save(ctx: RpcContext, p: SaveLayoutParams) -> Result<LayoutsState, RpcError> {
    let s = store(&ctx);
    let dev = s.device(p.device_id.as_deref()).await.map_err(storage_to_rpc)?;
    let id = p.layout.id.clone();
    let state = s.save(dev, p.layout, p.activate).await.map_err(storage_to_rpc)?;
    tracing::debug!(layout = %id, activate = p.activate, "layout saved");
    Ok(state)
}

async fn delete(ctx: RpcContext, p: LayoutIdParams) -> Result<LayoutsState, RpcError> {
    let s = store(&ctx);
    let dev = s.device(p.device_id.as_deref()).await.map_err(storage_to_rpc)?;
    s.delete(dev, &p.id).await.map_err(storage_to_rpc)
}

async fn rename(ctx: RpcContext, p: RenameLayoutParams) -> Result<LayoutsState, RpcError> {
    let s = store(&ctx);
    let dev = s.device(p.device_id.as_deref()).await.map_err(storage_to_rpc)?;
    s.rename(dev, &p.id, &p.name).await.map_err(storage_to_rpc)
}

async fn set_active(ctx: RpcContext, p: LayoutIdParams) -> Result<LayoutsState, RpcError> {
    let s = store(&ctx);
    let dev = s.device(p.device_id.as_deref()).await.map_err(storage_to_rpc)?;
    s.set_active(dev, &p.id).await.map_err(storage_to_rpc)
}
