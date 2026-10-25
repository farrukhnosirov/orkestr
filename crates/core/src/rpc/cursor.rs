//! Cursor capture mode (`computer.cursor*`, ADR-0050). Thin adapters over
//! [`orkestr_computer::cursor::CursorService`]; the `computer` area policy
//! keeps them local-only (Admin + step-up like the rest of the area).

use orkestr_protocol::methods::cursor as m;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};

/// Registers the methods.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::CursorStatusMethod, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.cursor.status().await)
        })
        .method::<m::CursorStart, _, _>(|ctx: RpcContext, p: m::CursorStartParams| async move {
            Ok(ctx.services.cursor.start(&p.task, &p.lang, &p.source).await?)
        })
        .method::<m::CursorStop, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.cursor.stop("stop").await)
        })
        .method::<m::CursorPause, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.cursor.pause().await)
        })
        .method::<m::CursorResume, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok::<_, RpcError>(ctx.services.cursor.resume().await)
        })
        .method::<m::CursorAnswer, _, _>(|ctx: RpcContext, p: m::CursorAnswerParams| async move {
            Ok(ctx.services.cursor.answer(&p.text).await?)
        })
        .method::<m::CursorConfirm, _, _>(|ctx: RpcContext, p: m::CursorConfirmParams| async move {
            Ok(ctx.services.cursor.confirm(p.approve).await?)
        })
        .method::<m::CursorConsent, _, _>(|ctx: RpcContext, p: m::CursorConsentParams| async move {
            Ok(ctx.services.cursor.consent(&p.backend, p.granted).await?)
        })
        .method::<m::CursorSetSettings, _, _>(|ctx: RpcContext, p: m::CursorSetSettingsParams| async move {
            Ok(ctx.services.cursor.set_settings(p.settings).await?)
        })
        .method::<m::CursorScreenshot, _, _>(|ctx: RpcContext, p: m::CursorShotParams| async move {
            Ok(ctx.services.cursor.screenshot(&p.id)?)
        });
}
