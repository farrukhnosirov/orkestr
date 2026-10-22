//! Marketplace (`marketplace.*`, ADR-0040). **Owner: WP2.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP2 replaces the stubs.

use orkestr_protocol::methods::marketplace as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Sources>(router);
    stub::<m::AddSource>(router);
    stub::<m::RemoveSource>(router);
    stub::<m::Refresh>(router);
    stub::<m::Search>(router);
    stub::<m::Get>(router);
    stub::<m::Install>(router);
    stub::<m::Publish>(router);
    stub::<m::Keys>(router);
    stub::<m::TrustKey>(router);
    stub::<m::UntrustKey>(router);
}
