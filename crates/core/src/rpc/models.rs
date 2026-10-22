//! Local model store + distribution (`models.*`) and the worker-side blob
//! transfer / install methods (`node.blobHas|blobPut|blobGet|modelInstall`)
//! (Phase 6, ADR-0039). **Owner: WP-D.**
//!
//! Step 0 registers stubs; WP-D replaces `stub::<M>` with real handlers here.

use orkestr_protocol::methods::models as m;

use super::{stub, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    stub::<m::List>(router);
    stub::<m::Import>(router);
    stub::<m::Distribute>(router);
    stub::<m::BlobHas>(router);
    stub::<m::BlobPut>(router);
    stub::<m::BlobGet>(router);
    stub::<m::ModelInstall>(router);
}
