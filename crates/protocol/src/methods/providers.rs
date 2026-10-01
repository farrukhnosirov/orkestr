//! `providers.*` — **placeholder.** Provider Hub (§4, §7), Phase 1.
//!
//! Method names are reserved here so clients and docs can refer to them;
//! param/result types and the marker types are added together with the
//! handler in `crates/core/src/rpc/providers.rs` by the owning phase.

/// Method names planned for this area (not yet routed by the core).
pub const PLANNED: &[&str] = &[
    "providers.list",
    "providers.create",
    "providers.update",
    "providers.delete",
    "providers.models",
    "providers.health",
    "providers.setCredential",
];

#[cfg(test)]
mod tests {
    #[test]
    fn planned_names_are_valid() {
        for name in super::PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }
}
