//! Job control (Phase 6). **Owner: WP-C.**
//!
//! * UI: `jobs.migrate`, `jobs.cancel`;
//! * worker side (coordinator principals only): `jobs.offer`, `jobs.drain`,
//!   `node.jobCancel`.
//!
//! Step 0 registers stubs; WP-C replaces `stub::<M>` with real handlers here.

use orkestr_protocol::methods::jobs as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Migrate>(router);
    stub::<m::Cancel>(router);
    stub::<m::Offer>(router);
    stub::<m::Drain>(router);
    stub::<m::JobCancel>(router);
}
