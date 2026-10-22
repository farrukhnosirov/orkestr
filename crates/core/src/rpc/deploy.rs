//! Environments + deployments (`environments.*`, `deploy.*`, ADR-0041). **Owner: WP4.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP4 replaces the stubs.

use orkestr_protocol::methods::deploy as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::EnvironmentsList>(router);
    stub::<m::EnvironmentsCreate>(router);
    stub::<m::EnvironmentsUpdate>(router);
    stub::<m::EnvironmentsDelete>(router);
    stub::<m::Adapters>(router);
    stub::<m::Plan>(router);
    stub::<m::Start>(router);
    stub::<m::Cancel>(router);
    stub::<m::List>(router);
    stub::<m::Get>(router);
    stub::<m::Logs>(router);
    stub::<m::Rollback>(router);
}
