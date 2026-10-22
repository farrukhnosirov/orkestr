//! Dashboard Builder persistence (Phase 7). **Owner: WP3.**
//!
//! Tables are prefixed `dashboards_`; [`DASHBOARD_MIGRATIONS`] is composed by
//! [`crate::migrations::all`].

use orkestr_storage::Migration;

/// Dashboard migrations (owner `core`). Empty in the step-0 skeleton; WP3
/// appends `dashboards_0001_*`.
pub const DASHBOARD_MIGRATIONS: &[Migration] = &[];
