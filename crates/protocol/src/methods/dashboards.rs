//! `dashboards.*` — Dashboard Builder: per-project and global dashboards on a
//! 12-column grid with built-in and plugin widgets (Phase 7, ADR-0034).
//! **Owner: WP3** (store `core/src/dashboards_store.rs`, handlers `core/src/rpc/dashboards.rs`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{DashboardId, ProjectId, Timestamp};

use super::{method, Empty};

/// Grid columns of every dashboard.
pub const DASHBOARD_COLUMNS: u32 = 12;
/// Max widgets on one dashboard.
pub const MAX_DASHBOARD_WIDGETS: usize = 64;
/// Max serialized size of one widget instance config (bytes).
pub const MAX_WIDGET_CONFIG_BYTES: usize = 16 * 1024;

/// One widget placed on a dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetInstance {
    /// Instance id (unique within the dashboard).
    pub instance_id: String,
    /// Widget id (`builtin:<id>` or `<packageId>/<widgetId>`).
    pub widget_id: String,
    /// Column (0-11).
    pub x: u32,
    /// Row.
    pub y: u32,
    /// Width in columns.
    pub w: u32,
    /// Height in rows.
    pub h: u32,
    /// Per-instance config (validated against the widget's `configSchema`).
    #[serde(default)]
    #[ts(type = "unknown")]
    pub config: serde_json::Value,
    /// Title override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

/// A dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Dashboard {
    /// Id.
    pub id: DashboardId,
    /// Project (`None` = global dashboard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Widgets.
    pub widgets: Vec<WidgetInstance>,
    /// Optimistic concurrency version (incremented on every save).
    pub version: u32,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// List row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardSummary {
    /// Id.
    pub id: DashboardId,
    /// Project (`None` = global).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Widget count.
    pub widget_count: u32,
    /// Last change.
    pub updated_at: Timestamp,
}

/// `dashboards.list` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListDashboardsParams {
    /// Project dashboards of this project (plus global ones when `includeGlobal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Include global dashboards (default true).
    #[serde(default = "yes")]
    pub include_global: bool,
}

fn yes() -> bool {
    true
}

/// `dashboards.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListDashboardsResult {
    /// Dashboards.
    pub dashboards: Vec<DashboardSummary>,
}

/// Params naming one dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardIdParams {
    /// Dashboard.
    pub dashboard_id: DashboardId,
}

/// Result carrying one dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardResult {
    /// The dashboard.
    pub dashboard: Dashboard,
}

/// `dashboards.save` params (create when `dashboardId` is absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveDashboardParams {
    /// Existing dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dashboard_id: Option<DashboardId>,
    /// Project (`None` = global; ignored on update).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Widgets.
    pub widgets: Vec<WidgetInstance>,
    /// Expected current version (conflict when it differs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_version: Option<u32>,
}

/// `DashboardChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashboardChangedPayload {
    /// Dashboard.
    pub dashboard_id: DashboardId,
    /// Project (`None` = global).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Deleted (otherwise created/updated).
    pub deleted: bool,
    /// Version after the change.
    pub version: u32,
}

method!(/// Dashboards.
    List, "dashboards.list", ListDashboardsParams, ListDashboardsResult);
method!(/// One dashboard.
    Get, "dashboards.get", DashboardIdParams, DashboardResult);
method!(/// Creates or updates a dashboard.
    Save, "dashboards.save", SaveDashboardParams, DashboardResult);
method!(/// Deletes a dashboard.
    Delete, "dashboards.delete", DashboardIdParams, Empty);
