//! UI `jobs.submit` / `jobs.list` / `jobs.get` (Phase 6). **Owner: WP-B.**
//!
//! Step 0 registers stubs; WP-B replaces `stub::<M>` with real handlers here.

use orkestr_protocol::methods::jobs as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Submit>(router);
    stub::<m::List>(router);
    stub::<m::Get>(router);
}
