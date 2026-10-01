//! `terminal.*` — **placeholder.** Terminal sessions (§15), Phase 1. Output streams as `TerminalOutput` events on `terminal:<id>`.
//!
//! Method names are reserved here so clients and docs can refer to them;
//! param/result types and the marker types are added together with the
//! handler in `crates/core/src/rpc/terminal.rs` by the owning phase.

/// Method names planned for this area (not yet routed by the core).
pub const PLANNED: &[&str] =
    &["terminal.list", "terminal.open", "terminal.write", "terminal.resize", "terminal.close", "terminal.shells"];

#[cfg(test)]
mod tests {
    #[test]
    fn planned_names_are_valid() {
        for name in super::PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }
}
