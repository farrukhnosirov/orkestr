//! `push.*` handlers — Web Push subscriptions (Phase 4, ADR-0036).
//!
//! Subscriptions are owned by the calling device (remote principal) and kept
//! in the core; payloads are RFC 8291-encrypted here and delivered by the
//! gateway via `RemoteService::send_push`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use orkestr_domain::DeviceId;
use orkestr_protocol::methods::push as m;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;
use rand_core06::{OsRng, RngCore};

use super::{Principal, RpcContext, RpcRouter};
use crate::remote::push::{self, category, StoredSubscription};

fn caller(ctx: &RpcContext) -> Option<DeviceId> {
    match &ctx.principal {
        Principal::Remote(r) => Some(r.device_id),
        Principal::Local => None,
    }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::VapidKey, _, _>(|ctx: RpcContext, _p: Empty| async move {
        let svc = push::service(&ctx.services);
        svc.vapid_public_key()
            .or_else(|| ctx.services.remote.vapid_public_key())
            .map(|public_key| m::PushVapidKey { public_key })
            .ok_or_else(|| RpcError::conflict("gateway VAPID key unknown: remote access is not connected"))
    });
    router.method::<m::Subscribe, _, _>(|ctx: RpcContext, p: m::PushSubscribeParams| async move {
        let svc = push::service(&ctx.services);
        let mut id = [0u8; 12];
        OsRng.fill_bytes(&mut id);
        let id = svc.subscribe(StoredSubscription {
            id: URL_SAFE_NO_PAD.encode(id),
            device_id: caller(&ctx),
            endpoint: p.endpoint,
            p256dh: p.p256dh,
            auth: p.auth,
            categories: p.categories,
        })?;
        Ok(m::PushSubscription { id })
    });
    router.method::<m::Unsubscribe, _, _>(|ctx: RpcContext, p: m::PushUnsubscribeParams| async move {
        push::service(&ctx.services).unsubscribe(&p.endpoint, caller(&ctx))?;
        Ok(Empty {})
    });
    // Native (FCM/APNs) registrations land in Phase 5 WP-A (ADR-0037).
    super::stub::<m::RegisterNative>(router);
    router.method::<m::Test, _, _>(|ctx: RpcContext, _p: Empty| async move {
        let svc = push::service(&ctx.services);
        let payload = m::PushPayload {
            v: 1,
            event_id: String::new(),
            category: category::TEST.to_owned(),
            project_id: None,
            ts: chrono::Utc::now().to_rfc3339(),
            title: "Orkestr".to_owned(),
            body: "Test notification".to_owned(),
        };
        let sent = svc.dispatch(&ctx.services.remote, &payload, "normal", None, caller(&ctx)).await?;
        Ok(m::PushTestResult { sent })
    });
}
