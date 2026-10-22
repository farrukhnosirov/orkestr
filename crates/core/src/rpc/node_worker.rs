//! Worker-side `node.hello` / `node.heartbeat` handlers (Phase 6, called by the
//! coordinator; policy: coordinator principals only). **Owner: WP-A.**
//!
//! Step 0 registers stubs; WP-A replaces `stub::<M>` with real handlers here.

use orkestr_protocol::methods::nodes as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::Hello>(router);
    stub::<m::Heartbeat>(router);
}
