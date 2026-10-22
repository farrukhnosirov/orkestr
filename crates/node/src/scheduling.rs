//! Node scheduler interface (Phase 6, node-protocol.md §5). Frozen in Step 0;
//! the implementation `DefaultNodeScheduler` lives in
//! `orkestr_task_engine::node_scheduler` (WP-B).

use orkestr_protocol::methods::jobs::{JobSpec, Placement, PlacementPolicy};
use orkestr_protocol::methods::nodes::NodeSnapshot;

/// Deterministic placement: filter (hard constraints) → score → tie-break
/// (coordinator, then lowest `NodeId`). Pure: no I/O, no clock.
pub trait NodeScheduler: Send + Sync {
    /// Decides where `job` runs.
    fn place(&self, job: &JobSpec, nodes: &[NodeSnapshot], policy: &PlacementPolicy) -> Placement;
}
