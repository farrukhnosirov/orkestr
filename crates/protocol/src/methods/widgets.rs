//! `widgets.*` — widget catalogue (built-in + plugin widgets) and sandboxed
//! plugin widget bundles (Phase 7, ADR-0034). **Owner: WP3.**
//!
//! Plugin widgets run in `<iframe sandbox="allow-scripts">` (no same-origin)
//! with an injected CSP and no network; data only through the widget SDK
//! bridge, limited to Read-class, non-local-only methods of the widget's
//! `dataBinding` allowlist (enforced in the host and in core).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{DataBinding, PluginId, WidgetSize};

use super::{method, Empty};

/// Where a widget comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum WidgetSource {
    /// Native widget shipped with the desktop app.
    BuiltIn,
    /// Plugin widget (sandboxed iframe).
    Plugin {
        /// Plugin.
        plugin_id: PluginId,
        /// Package id.
        package_id: String,
    },
}

/// A widget available to the Dashboard Builder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetDescriptor {
    /// Widget id (`builtin:<id>` or `<packageId>/<widgetId>`).
    pub id: String,
    /// Title.
    pub title: String,
    /// Source.
    pub source: WidgetSource,
    /// Supported sizes (first = default).
    pub sizes: Vec<WidgetSize>,
    /// Data binding allowlist.
    pub data_binding: DataBinding,
    /// JSON schema of the per-instance config.
    #[ts(type = "unknown")]
    pub config_schema: serde_json::Value,
}

/// `widgets.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListWidgetsResult {
    /// Built-in widgets (`builtin:<id>`, rendered natively) followed by widgets of
    /// enabled plugins whose data binding passes the allowlist.
    pub widgets: Vec<WidgetDescriptor>,
}

/// `widgets.bundle` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetBundleParams {
    /// Plugin widget id (`<packageId>/<widgetId>`).
    pub widget_id: String,
}

/// `widgets.bundle` result: self-contained HTML only (no external resources).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WidgetBundleResult {
    /// Widget id.
    pub widget_id: String,
    /// HTML document (inline scripts/styles only).
    pub html: String,
    /// CSP the host injects (`default-src 'none'; script-src 'unsafe-inline'; ...`).
    pub csp: String,
    /// sha256 of `html` (hex).
    pub sha256: String,
}

method!(/// Plugin widgets.
    List, "widgets.list", Empty, ListWidgetsResult);
method!(/// Sandboxed HTML bundle of a plugin widget.
    Bundle, "widgets.bundle", WidgetBundleParams, WidgetBundleResult);
