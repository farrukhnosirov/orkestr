//! `snapshots.*` — **placeholder.** Snapshots and rollback (§25), Phase 1.
//!
//! Method names are reserved here so clients and docs can refer to them;
//! param/result types and the marker types are added together with the
//! handler in `crates/core/src/rpc/snapshots.rs` by the owning phase.

/// Method names planned for this area (not yet routed by the core).
pub const PLANNED: &[&str] =
    &["snapshots.list", "snapshots.create", "snapshots.rollbackTask", "snapshots.rollbackWorkspace"];

#[cfg(test)]
mod tests {
    #[test]
    fn planned_names_are_valid() {
        for name in super::PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }
}
