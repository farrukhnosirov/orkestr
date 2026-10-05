//! Shell preferences of the UI configuration (§29 AI-editable UI, §34 HUD).
//!
//! Appended to [`crate::ui_config::UiConfig`] as the optional `prefs` section
//! (schema v1, additive: documents without it load with the defaults).
//! Everything here is plain data the desktop interprets — panel placement
//! hints, hidden widgets, density, background dim and HUD look — so the AI
//! Manager can rearrange and declutter the UI through validated JSON patches
//! (`ui.previewPatch` / `ui.applyPatch`, [`crate::ui_patch`]) without ever
//! producing code.
//!
//! Ids of panels and widgets are opaque, namespaced strings owned by the
//! feature that renders them (`terminal`, `missionControl.claude`,
//! `missionControl.tokenChart`); unknown ids are kept and simply ignored by
//! the UI, so features can be added without a schema change.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ui_config::UiConfigIssue;

/// Max entries in [`UiPrefs::panels`].
pub const MAX_PANEL_PREFS: usize = 64;
/// Max entries in [`UiPrefs::hidden_widgets`].
pub const MAX_HIDDEN_WIDGETS: usize = 128;

/// Spacing density of the whole shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Density {
    /// Default spacing.
    #[default]
    Comfortable,
    /// Tighter spacing, smaller chrome.
    Compact,
}

/// Where a panel or lane prefers to live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PanelSide {
    /// Left column.
    Left,
    /// Right column.
    Right,
    /// Bottom row.
    Bottom,
    /// Main / centre area.
    Center,
}

/// Placement hint of one panel / lane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PanelPlacement {
    /// Preferred side (`None` = wherever the layout puts it).
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<PanelSide>,
    /// Hidden.
    pub hidden: bool,
}

/// HUD / Ambient window look (§34).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HudPrefs {
    /// Background opacity of the HUD (0.4..=1).
    pub opacity: f32,
    /// Show the agent / robot strip.
    pub show_agents: bool,
    /// Show the mini Manager input.
    pub show_manager_input: bool,
}

impl Default for HudPrefs {
    fn default() -> Self {
        Self { opacity: 0.82, show_agents: true, show_manager_input: true }
    }
}

/// Shell preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct UiPrefs {
    /// Spacing density.
    pub density: Density,
    /// Placement hints by panel / lane id.
    pub panels: BTreeMap<String, PanelPlacement>,
    /// Ids of hidden widgets (charts, cards).
    pub hidden_widgets: Vec<String>,
    /// Extra darkening of the app background (0 = none, 0.9 = almost black).
    pub background_dim: f32,
    /// HUD look.
    pub hud: HudPrefs,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            density: Density::Comfortable,
            panels: BTreeMap::new(),
            hidden_widgets: Vec::new(),
            background_dim: 0.0,
            hud: HudPrefs::default(),
        }
    }
}

/// Whether a panel / widget id is acceptable: `^[a-z][A-Za-z0-9.-]{0,63}$`.
#[must_use]
pub fn is_valid_ui_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn range(issues: &mut Vec<UiConfigIssue>, path: &str, v: f32, min: f32, max: f32) {
    if !(v.is_finite() && v >= min && v <= max) {
        issues.push(UiConfigIssue { path: path.into(), message: format!("must be between {min} and {max}") });
    }
}

impl UiPrefs {
    /// Removes duplicate hidden widget ids (keeps the first occurrence).
    pub fn normalize(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        self.hidden_widgets.retain(|w| seen.insert(w.clone()));
    }

    /// Appends every problem to `issues`.
    pub fn validate_into(&self, issues: &mut Vec<UiConfigIssue>) {
        let mut push = |path: String, message: &str| issues.push(UiConfigIssue { path, message: message.into() });
        if self.panels.len() > MAX_PANEL_PREFS {
            push("prefs.panels".into(), "too many panel entries");
        }
        for id in self.panels.keys() {
            if !is_valid_ui_id(id) {
                push(format!("prefs.panels.{id}"), "invalid panel id");
            }
        }
        if self.hidden_widgets.len() > MAX_HIDDEN_WIDGETS {
            push("prefs.hiddenWidgets".into(), "too many hidden widgets");
        }
        for (i, id) in self.hidden_widgets.iter().enumerate() {
            if !is_valid_ui_id(id) {
                push(format!("prefs.hiddenWidgets.{i}"), "invalid widget id");
            }
        }
        range(issues, "prefs.backgroundDim", self.background_dim, 0.0, 0.9);
        range(issues, "prefs.hud.opacity", self.hud.opacity, 0.4, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        for ok in ["terminal", "missionControl.claude", "mission-control.token-chart", "a"] {
            assert!(is_valid_ui_id(ok), "{ok}");
        }
        for bad in ["", "Terminal", "1x", "a b", "a/b", "a<script>", &"a".repeat(65)] {
            assert!(!is_valid_ui_id(bad), "{bad}");
        }
    }

    #[test]
    fn defaults_validate_and_missing_fields_default() {
        let mut issues = Vec::new();
        UiPrefs::default().validate_into(&mut issues);
        assert!(issues.is_empty());
        let p: UiPrefs = serde_json::from_str(r#"{"density":"Compact"}"#).unwrap();
        assert_eq!(p.density, Density::Compact);
        assert_eq!(p.hud, HudPrefs::default());
    }

    #[test]
    fn ranges_and_dedupe() {
        let mut p = UiPrefs { background_dim: 2.0, ..UiPrefs::default() };
        p.hidden_widgets = vec!["a".into(), "a".into(), "b".into()];
        p.normalize();
        assert_eq!(p.hidden_widgets, vec!["a".to_owned(), "b".to_owned()]);
        let mut issues = Vec::new();
        p.validate_into(&mut issues);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "prefs.backgroundDim");
    }
}
