//! Bug Detective (`detective.*`, ADR-0042). **Owner: WP6.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP6 replaces the stubs.

use orkestr_protocol::methods::detective as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Open>(router);
    stub::<m::List>(router);
    stub::<m::Get>(router);
    stub::<m::ApproveFix>(router);
    stub::<m::ApproveMerge>(router);
    stub::<m::Cancel>(router);
}
