//! Dashboard Builder (`dashboards.*`); store in `crate::dashboards_store`. **Owner: WP3.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP3 replaces the stubs.

use orkestr_protocol::methods::dashboards as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::List>(router);
    stub::<m::Get>(router);
    stub::<m::Save>(router);
    stub::<m::Delete>(router);
}
