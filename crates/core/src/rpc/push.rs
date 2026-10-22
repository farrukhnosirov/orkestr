//! `push.*` handlers — Web Push subscriptions (Phase 4, ADR-0036).
//!
//! Step 0 skeleton: every method returns `unimplementedFeature` (RR-G14).

use orkestr_protocol::methods::push as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::VapidKey>(router);
    stub::<m::Subscribe>(router);
    stub::<m::Unsubscribe>(router);
    stub::<m::Test>(router);
}
