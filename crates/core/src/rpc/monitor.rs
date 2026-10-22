//! Infrastructure monitor (`monitor.*`, ADR-0042). **Owner: WP5.**
//!
//! Step-0 skeleton: every method is registered and answers the explicit
//! `unimplementedFeature` error until WP5 replaces the stubs.

use orkestr_protocol::methods::monitor as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Overview>(router);
    stub::<m::Metrics>(router);
    stub::<m::ChecksList>(router);
    stub::<m::CheckUpsert>(router);
    stub::<m::CheckDelete>(router);
    stub::<m::CheckRun>(router);
    stub::<m::LogSources>(router);
    stub::<m::Logs>(router);
    stub::<m::Ingest>(router);
    stub::<m::Alerts>(router);
    stub::<m::AckAlert>(router);
}
