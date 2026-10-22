//! Incident stage machine. **Owner: WP6.**

use orkestr_protocol::methods::detective::IncidentStage;

/// Whether `from -> to` is allowed. Step-0 skeleton: only cancellation of a
/// non-terminal incident is defined; WP6 completes the table.
#[must_use]
pub fn can_transition(from: IncidentStage, to: IncidentStage) -> bool {
    !from.is_terminal() && to == IncidentStage::Cancelled
}
