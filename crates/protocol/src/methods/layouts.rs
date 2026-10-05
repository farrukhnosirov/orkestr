//! `ui.layouts*` — docking layout presets persisted **per device** (§29, §34,
//! ADR-0013, ADR-0015).
//!
//! A [`LayoutDocument`] is one preset: the serialized dockview layout
//! (opaque to the core, validated against the panel registry by the UI), the
//! visible panels, the lock flag and the window arrangement (detached panel
//! windows and the monitors they live on). Documents are stored per device:
//! the same core can serve several desktops (Phase 4 web/remote) and each one
//! has its own monitors, so layouts never leak between machines.
//!
//! Built-in preset ids (`coding`, `mission-control`, `monitoring`, `focus`,
//! `review`, `custom`) have no stored document until the user changes them;
//! deleting a built-in id resets it. User presets use any other id.
//!
//! The device id defaults to this core's local device id (`ui.device`), so
//! the desktop never needs to send one; remote clients pass their own.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::Timestamp;

use super::method;

/// Version of [`LayoutDocument`]. Increment on breaking changes and migrate
/// old documents in the UI (`features/layout/schema.ts`) and here.
pub const LAYOUT_SCHEMA_VERSION: u32 = 1;
/// Max size of the serialized dock JSON.
pub const MAX_DOCK_BYTES: usize = 256 * 1024;
/// Max stored layouts per device.
pub const MAX_LAYOUTS_PER_DEVICE: usize = 64;
/// Max detached windows a layout may restore.
pub const MAX_POPOUTS: usize = 16;
/// Max panel ids in a layout.
pub const MAX_PANELS: usize = 64;
/// Max entries in [`LayoutDocument::widgets`].
pub const MAX_WIDGET_STATES: usize = 16;
/// Max size of one widget state value (bytes).
pub const MAX_WIDGET_STATE_BYTES: usize = 16 * 1024;

/// Built-in preset ids (§29). Order = `Ctrl+1..5` shortcuts (`custom` has none).
pub const BUILTIN_LAYOUT_IDS: &[&str] = &["coding", "mission-control", "monitoring", "focus", "review", "custom"];

/// Whether `id` names a built-in preset.
#[must_use]
pub fn is_builtin_layout(id: &str) -> bool {
    BUILTIN_LAYOUT_IDS.contains(&id)
}

/// A rectangle in physical pixels. For window placement it is **relative to
/// the monitor's work area origin**, so a layout survives monitors being
/// rearranged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width (> 0).
    pub width: u32,
    /// Height (> 0).
    pub height: u32,
}

/// A detached panel window (§28 "detachable panels", §34 multi-monitor).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PopoutWindowSpec {
    /// Panel registry id (`code`, `browser`, …).
    pub panel_id: String,
    /// Project the window is bound to; `None` = follow the active project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<String>,
    /// Monitor key (OS monitor name or a geometry key) the window was on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub monitor: Option<String>,
    /// Bounds relative to that monitor's work area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bounds: Option<LayoutRect>,
    /// Window maximized on its monitor.
    #[serde(default)]
    pub maximized: bool,
}

/// Window arrangement of a layout: where the main window and every detached
/// panel window live.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowArrangement {
    /// Monitor key of the main window, if pinned to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub main_monitor: Option<String>,
    /// Main window bounds relative to that monitor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub main_bounds: Option<LayoutRect>,
    /// Main window maximized.
    #[serde(default)]
    pub main_maximized: bool,
    /// Detached panel windows.
    #[serde(default)]
    pub popouts: Vec<PopoutWindowSpec>,
}

/// One layout preset of one device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutDocument {
    /// Must equal [`LAYOUT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Stable id `^[a-z][a-z0-9-]{0,47}$` (built-in ids or user ids).
    pub id: String,
    /// Display name (1..=80 chars). Built-ins are shown with their localized
    /// name; this one is used for user presets.
    pub name: String,
    /// Built-in preset this one was derived from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub based_on: Option<String>,
    /// Serialized dockview layout (`SerializedDockview`). Opaque to the core;
    /// the UI validates it against its panel registry before applying it.
    pub dock: serde_json::Value,
    /// Panel ids visible in the dock (summary; also used by pickers).
    #[serde(default)]
    pub panels: Vec<String>,
    /// Layout locked: no drag/drop, resize or close (Layout Editor toggle).
    #[serde(default)]
    pub locked: bool,
    /// Window arrangement (detached panels, monitors).
    #[serde(default)]
    pub windows: WindowArrangement,
    /// View state of panels in this layout, keyed by panel id (e.g. the
    /// Mission Control widget arrangement under `missionControl`). Opaque
    /// strings owned by the panel; Phase 3, additive (absent = none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub widgets: Option<std::collections::BTreeMap<String, String>>,
    /// Last change (set by the core on save).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
}

/// One validation problem of a layout document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutIssue {
    /// Field path (`windows.popouts.0.panelId`).
    pub path: String,
    /// Message.
    pub message: String,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> LayoutIssue {
    LayoutIssue { path: path.into(), message: message.into() }
}

/// Whether a layout id is acceptable (`^[a-z][a-z0-9-]{0,47}$`).
#[must_use]
pub fn is_valid_layout_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && id.len() <= 48
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether a panel id is acceptable (`^[a-zA-Z][a-zA-Z0-9_-]{0,47}$`).
#[must_use]
pub fn is_valid_panel_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && id.len() <= 48
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validates a display name (1..=80 printable chars).
#[must_use]
pub fn is_valid_layout_name(name: &str) -> bool {
    let n = name.trim().chars().count();
    (1..=80).contains(&n) && !name.chars().any(char::is_control)
}

fn is_valid_monitor_key(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= 128 && !key.chars().any(char::is_control)
}

fn check_rect(issues: &mut Vec<LayoutIssue>, path: &str, r: &LayoutRect) {
    const LIMIT: i32 = 100_000;
    if r.width == 0 || r.height == 0 || r.width > LIMIT as u32 || r.height > LIMIT as u32 {
        issues.push(issue(path, "width and height must be 1..=100000"));
    }
    if r.x.abs() > LIMIT || r.y.abs() > LIMIT {
        issues.push(issue(path, "position out of range"));
    }
}

impl LayoutDocument {
    /// Validates the document; returns every issue found.
    pub fn validate(&self) -> Result<(), Vec<LayoutIssue>> {
        let mut issues = Vec::new();
        if self.schema_version != LAYOUT_SCHEMA_VERSION {
            issues.push(issue(
                "schemaVersion",
                format!("unsupported layout schema version {} (expected {LAYOUT_SCHEMA_VERSION})", self.schema_version),
            ));
        }
        if !is_valid_layout_id(&self.id) {
            issues.push(issue("id", "must match ^[a-z][a-z0-9-]{0,47}$"));
        }
        if !is_valid_layout_name(&self.name) {
            issues.push(issue("name", "must be 1..=80 printable characters"));
        }
        if let Some(b) = &self.based_on {
            if !is_builtin_layout(b) {
                issues.push(issue("basedOn", "must be a built-in preset id"));
            }
        }
        if !self.dock.is_object() {
            issues.push(issue("dock", "must be a JSON object (dockview layout)"));
        } else {
            let size = serde_json::to_vec(&self.dock).map(|v| v.len()).unwrap_or(usize::MAX);
            if size > MAX_DOCK_BYTES {
                issues.push(issue("dock", "dock layout too large (max 256 KiB)"));
            }
        }
        if self.panels.len() > MAX_PANELS {
            issues.push(issue("panels", format!("at most {MAX_PANELS} panels")));
        }
        for (i, p) in self.panels.iter().enumerate() {
            if !is_valid_panel_id(p) {
                issues.push(issue(format!("panels.{i}"), "invalid panel id"));
            }
        }
        let w = &self.windows;
        if let Some(m) = &w.main_monitor {
            if !is_valid_monitor_key(m) {
                issues.push(issue("windows.mainMonitor", "invalid monitor key"));
            }
        }
        if let Some(r) = &w.main_bounds {
            check_rect(&mut issues, "windows.mainBounds", r);
        }
        if w.popouts.len() > MAX_POPOUTS {
            issues.push(issue("windows.popouts", format!("at most {MAX_POPOUTS} detached windows")));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, p) in w.popouts.iter().enumerate() {
            let base = format!("windows.popouts.{i}");
            if !is_valid_panel_id(&p.panel_id) {
                issues.push(issue(format!("{base}.panelId"), "invalid panel id"));
            } else if !seen.insert(p.panel_id.as_str()) {
                issues.push(issue(format!("{base}.panelId"), "panel is detached twice"));
            }
            if let Some(m) = &p.monitor {
                if !is_valid_monitor_key(m) {
                    issues.push(issue(format!("{base}.monitor"), "invalid monitor key"));
                }
            }
            if let Some(pid) = &p.project_id {
                if pid.is_empty() || pid.len() > 64 {
                    issues.push(issue(format!("{base}.projectId"), "invalid project id"));
                }
            }
            if let Some(r) = &p.bounds {
                check_rect(&mut issues, &format!("{base}.bounds"), r);
            }
        }
        if let Some(widgets) = &self.widgets {
            if widgets.len() > MAX_WIDGET_STATES {
                issues.push(issue("widgets", format!("at most {MAX_WIDGET_STATES} widget states")));
            }
            for (k, v) in widgets {
                if !is_valid_panel_id(k) {
                    issues.push(issue(format!("widgets.{k}"), "invalid panel id"));
                } else if v.len() > MAX_WIDGET_STATE_BYTES {
                    issues.push(issue(format!("widgets.{k}"), "widget state too large (max 16 KiB)"));
                }
            }
        }
        if self.panels.is_empty() && w.popouts.is_empty() {
            // ADR-0015 safety invariant: a layout shows at least one panel.
            issues.push(issue("panels", "a layout must show at least one panel"));
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

/// Params naming the device (absent = this core's local device).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutsDeviceParams {
    /// Device id (UUID); defaults to the local device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
}

/// All layouts of one device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutsState {
    /// The device these layouts belong to.
    pub device_id: String,
    /// Active preset id (`coding` until the user switches).
    pub active_id: String,
    /// Stored documents (built-ins only when customised), sorted by id.
    pub layouts: Vec<LayoutDocument>,
}

/// Params of `ui.layoutsSave`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveLayoutParams {
    /// Device id; defaults to the local device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
    /// Document to create or replace (by id).
    pub layout: LayoutDocument,
    /// Also make it the active layout.
    #[serde(default)]
    pub activate: bool,
}

/// Params naming one layout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutIdParams {
    /// Device id; defaults to the local device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
    /// Layout id.
    pub id: String,
}

/// Params of `ui.layoutsRename`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RenameLayoutParams {
    /// Device id; defaults to the local device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
    /// Layout id (user presets only).
    pub id: String,
    /// New display name.
    pub name: String,
}

/// Result of `ui.device`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceInfoResult {
    /// This core's local device id (created on first use).
    pub device_id: String,
}

method!(
    /// `ui.device` — the local device id layouts are stored under.
    Device, "ui.device", super::Empty, DeviceInfoResult
);
method!(
    /// `ui.layoutsList` — all layouts + the active id of a device.
    List, "ui.layoutsList", LayoutsDeviceParams, LayoutsState
);
method!(
    /// `ui.layoutsSave` — create or replace a layout (validated).
    Save, "ui.layoutsSave", SaveLayoutParams, LayoutsState
);
method!(
    /// `ui.layoutsDelete` — delete a user layout or reset a built-in one.
    Delete, "ui.layoutsDelete", LayoutIdParams, LayoutsState
);
method!(
    /// `ui.layoutsRename` — rename a user layout.
    Rename, "ui.layoutsRename", RenameLayoutParams, LayoutsState
);
method!(
    /// `ui.layoutsSetActive` — switch the device's active layout.
    SetActive, "ui.layoutsSetActive", LayoutIdParams, LayoutsState
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::Method;

    fn doc() -> LayoutDocument {
        LayoutDocument {
            schema_version: LAYOUT_SCHEMA_VERSION,
            id: "my-layout".into(),
            name: "My layout".into(),
            based_on: Some("coding".into()),
            dock: serde_json::json!({"grid": {}, "panels": {}}),
            panels: vec!["code".into(), "missionControl".into()],
            locked: false,
            windows: WindowArrangement {
                main_monitor: Some("\\\\.\\DISPLAY1".into()),
                main_bounds: Some(LayoutRect { x: 0, y: 0, width: 1440, height: 900 }),
                main_maximized: false,
                popouts: vec![PopoutWindowSpec {
                    panel_id: "browser".into(),
                    project_id: None,
                    monitor: Some("\\\\.\\DISPLAY2".into()),
                    bounds: Some(LayoutRect { x: 10, y: 10, width: 800, height: 600 }),
                    maximized: true,
                }],
            },
            widgets: Some([("missionControl".to_owned(), "{\"v\":1}".to_owned())].into_iter().collect()),
            updated_at: None,
        }
    }

    #[test]
    fn widget_states_are_bounded() {
        let mut d = doc();
        let mut w = std::collections::BTreeMap::new();
        w.insert("bad id".to_owned(), "x".to_owned());
        w.insert("missionControl".to_owned(), "x".repeat(MAX_WIDGET_STATE_BYTES + 1));
        d.widgets = Some(w);
        let paths: Vec<String> = d.validate().unwrap_err().into_iter().map(|i| i.path).collect();
        assert!(paths.contains(&"widgets.bad id".to_owned()), "{paths:?}");
        assert!(paths.contains(&"widgets.missionControl".to_owned()), "{paths:?}");
    }

    #[test]
    fn valid_document_passes_and_roundtrips() {
        let d = doc();
        d.validate().unwrap();
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["windows"]["popouts"][0]["panelId"], "browser");
        assert_eq!(json["basedOn"], "coding");
        let back: LayoutDocument = serde_json::from_value(json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn missing_optional_sections_default() {
        let d: LayoutDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "id": "focus", "name": "Focus", "dock": {}, "panels": ["code"]
        }))
        .unwrap();
        assert!(!d.locked);
        assert!(d.windows.popouts.is_empty());
        d.validate().unwrap();
    }

    #[test]
    fn rejects_bad_documents() {
        let mut d = doc();
        d.schema_version = 9;
        d.id = "Bad Id".into();
        d.name = String::new();
        d.based_on = Some("nope".into());
        d.dock = serde_json::json!([1, 2]);
        d.panels.push("../x".into());
        d.windows.popouts.push(d.windows.popouts[0].clone());
        d.windows.popouts[0].bounds = Some(LayoutRect { x: 0, y: 0, width: 0, height: 10 });
        let issues = d.validate().unwrap_err();
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        for want in [
            "schemaVersion",
            "id",
            "name",
            "basedOn",
            "dock",
            "panels.2",
            "windows.popouts.1.panelId",
            "windows.popouts.0.bounds",
        ] {
            assert!(paths.contains(&want), "missing {want} in {paths:?}");
        }
    }

    #[test]
    fn empty_layout_is_rejected() {
        let mut d = doc();
        d.panels.clear();
        d.windows.popouts.clear();
        assert!(d.validate().is_err());
    }

    #[test]
    fn huge_dock_is_rejected() {
        let mut d = doc();
        d.dock = serde_json::json!({ "blob": "x".repeat(MAX_DOCK_BYTES) });
        assert!(d.validate().unwrap_err().iter().any(|i| i.path == "dock"));
    }

    #[test]
    fn ids() {
        assert!(is_valid_layout_id("coding"));
        assert!(is_valid_layout_id("mission-control"));
        assert!(is_valid_layout_id("u-1a2b"));
        assert!(!is_valid_layout_id("1abc"));
        assert!(!is_valid_layout_id("a/b"));
        assert!(!is_valid_layout_id(&"a".repeat(49)));
        assert!(is_valid_panel_id("missionControl"));
        assert!(!is_valid_panel_id("<script>"));
        assert!(BUILTIN_LAYOUT_IDS.iter().all(|id| is_valid_layout_id(id)));
        for name in [Device::NAME, List::NAME, Save::NAME, Delete::NAME, Rename::NAME, SetActive::NAME] {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }
}
