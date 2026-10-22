//! Deterministic node scheduler (Phase 6, node-protocol.md §5). **Owner: WP-B.**
//!
//! Filter (hard constraints) → score (weighted, normalized) → tie-break
//! (coordinator, then lowest `NodeId`); explanation strings are stored on the task.

use orkestr_node::NodeScheduler;
use orkestr_protocol::methods::jobs::{JobSpec, Placement, PlacementPolicy};
use orkestr_protocol::methods::nodes::NodeSnapshot;

/// The default scheduler.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultNodeScheduler;

impl NodeScheduler for DefaultNodeScheduler {
    fn place(&self, job: &JobSpec, nodes: &[NodeSnapshot], policy: &PlacementPolicy) -> Placement {
        // Step 0 skeleton: WP-B implements filter + score + tie-break.
        let _ = (job, nodes, policy);
        Placement::Infeasible { reason: "unimplemented-feature: node scheduler".to_owned(), explanation: Vec::new() }
    }
}
