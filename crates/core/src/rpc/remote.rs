//! `remote.*` / `devices.*` handlers — remote access via the gateway (Phase 4, ADR-0030).

use orkestr_domain::Actor;
use orkestr_protocol::methods::remote as m;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;

use super::{Principal, RpcContext, RpcRouter};

fn actor(ctx: &RpcContext) -> Actor {
    match &ctx.principal {
        Principal::Local => Actor::System,
        Principal::Remote(rp) => Actor::RemoteDevice(rp.device_id),
    }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::Enable, _, _>(|ctx: RpcContext, p| async move { ctx.services.remote.enable(p).await });
    router.method::<m::Disable, _, _>(|ctx: RpcContext, _: Empty| async move {
        Ok::<_, RpcError>(ctx.services.remote.disable(actor(&ctx)).await)
    });
    router.method::<m::Status, _, _>(|ctx: RpcContext, _: Empty| async move {
        Ok::<_, RpcError>(ctx.services.remote.status())
    });
    router.method::<m::RevokeAllSessions, _, _>(|ctx: RpcContext, _: Empty| async move {
        let revoked = ctx.services.remote.revoke_all_sessions(actor(&ctx)).await;
        Ok::<_, RpcError>(m::RemoteRevokeAllResult { revoked })
    });
    router.method::<m::DevicesStartPairing, _, _>(|ctx: RpcContext, p: m::DevicePairingStartParams| async move {
        ctx.services.remote.start_pairing_with(&p)
    });
    router.method::<m::DevicesApprove, _, _>(|ctx: RpcContext, p| async move { ctx.services.remote.approve(p).await });
    router.method::<m::DevicesList, _, _>(|ctx: RpcContext, _: Empty| async move {
        Ok::<_, RpcError>(ctx.services.remote.list_devices())
    });
    router.method::<m::DevicesRevoke, _, _>(|ctx: RpcContext, p: m::DeviceRevokeParams| async move {
        ctx.services.remote.revoke_device(p.device_id, actor(&ctx)).await?;
        Ok::<_, RpcError>(Empty {})
    });
    router.method::<m::DevicesRegisterStepUp, _, _>(|ctx: RpcContext, p: m::DeviceRegisterStepUpParams| async move {
        let Principal::Remote(rp) = &ctx.principal else {
            return Err(RpcError::bad_request(
                "devices.registerStepUp is called by a paired device over the E2E channel",
            ));
        };
        ctx.services.remote.register_step_up(rp.device_id, &p.credential).await?;
        Ok(Empty {})
    });
}
