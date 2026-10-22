//! Plugin SDK host (`plugins.*`, ADR-0034). **Owner: WP1.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP1 replaces the stubs.

use orkestr_protocol::methods::plugins as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::List>(router);
    stub::<m::Get>(router);
    stub::<m::Install>(router);
    stub::<m::Grant>(router);
    stub::<m::Uninstall>(router);
    stub::<m::Enable>(router);
    stub::<m::Disable>(router);
    stub::<m::KillAll>(router);
    stub::<m::SetSafeMode>(router);
    stub::<m::Invoke>(router);
    stub::<m::Logs>(router);
}
