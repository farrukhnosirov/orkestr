//! Plugin permission grants: stored in the `PermissionEngine` with principal
//! `plugin:<id>` (ADR-0034). **Owner: WP1.**

use orkestr_domain::PluginId;

/// Principal string of a plugin in the permission engine.
#[must_use]
pub fn principal(id: &PluginId) -> String {
    format!("plugin:{id}")
}
