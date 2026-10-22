//! `remote.*` / `devices.*` handlers — remote access via the gateway (Phase 4, ADR-0030).
//!
//! Step 0 skeleton: every method returns `unimplementedFeature` (RR-G14).

use orkestr_protocol::methods::remote as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Enable>(router);
    stub::<m::Disable>(router);
    stub::<m::Status>(router);
    stub::<m::RevokeAllSessions>(router);
    stub::<m::DevicesStartPairing>(router);
    stub::<m::DevicesApprove>(router);
    stub::<m::DevicesList>(router);
    stub::<m::DevicesRevoke>(router);
    stub::<m::DevicesRegisterStepUp>(router);
}
