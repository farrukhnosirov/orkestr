//! `sync.*` handlers — encrypted sync + Seamless Continue (Phase 4, ADR-0031).
//!
//! Step 0 skeleton: every method returns `unimplementedFeature` (RR-G14).

use orkestr_protocol::methods::sync as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Configure>(router);
    stub::<m::Status>(router);
    stub::<m::Now>(router);
    stub::<m::ContinueOn>(router);
    stub::<m::Resume>(router);
    stub::<m::ListConflicts>(router);
    stub::<m::ResolveConflict>(router);
    stub::<m::RotateKeys>(router);
}
