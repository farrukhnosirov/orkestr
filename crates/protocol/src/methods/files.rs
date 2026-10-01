//! `files.*` — **placeholder.** Workspace files (§14 Files/Code panels), Phase 1. Confined to the project root.
//!
//! Method names are reserved here so clients and docs can refer to them;
//! param/result types and the marker types are added together with the
//! handler in `crates/core/src/rpc/files.rs` by the owning phase.

/// Method names planned for this area (not yet routed by the core).
pub const PLANNED: &[&str] = &["files.list", "files.read", "files.write", "files.search"];

#[cfg(test)]
mod tests {
    #[test]
    fn planned_names_are_valid() {
        for name in super::PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }
}
