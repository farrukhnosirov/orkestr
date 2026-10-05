//! `ui.*` — the validated UI configuration and the theme library (§29,
//! ADR-0015). Every write is validated by the core with
//! `UiConfig::validate` / `ThemeConfig::validate` (the only trusted validator).
//!
//! * `ui.getConfig {deviceId?}` — the effective config for a device (device
//!   override if one exists, else the global config).
//! * `ui.setConfig {config, deviceId?, baseVersion?}` — replaces the global
//!   config, or the device override when `deviceId` is set; `config: null`
//!   with a `deviceId` removes that override. `baseVersion` enables
//!   optimistic concurrency (`conflict` on mismatch).
//! * `ui.themes` — built-in + user themes.
//! * `ui.saveTheme {theme, overwrite}` / `ui.deleteTheme {id}` — user themes
//!   (built-in ids are read-only). Configs using a saved theme pick up the new
//!   version; configs using a deleted theme fall back to Nebula.
//!
//! Changes publish `UiConfigChanged` / `UiThemesChanged` on `global`.
//!
//! * `ui.previewPatch {patch, author?}` / `ui.applyPatch {patch, baseVersion?, author?, summary?, proposalId?}`
//!   — validated AI-editable UI patches ([`crate::ui_patch`]) against the global
//!   config; `ui.applyPatch` stores a new global version and publishes
//!   `UiConfigPatched` (plus `UiConfigChanged`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::method;
use crate::ui_config::{ThemeConfig, UiConfig};
use crate::ui_patch::{UiChange, UiPatchActor, UiPatchOp};
use crate::ui_theme::ContrastCheck;

/// Whether a client-chosen device id is acceptable (`^[A-Za-z0-9-]{1,64}$`).
#[must_use]
pub fn is_valid_device_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Which stored document a config came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum UiConfigScope {
    /// Shared by every device.
    #[default]
    Global,
    /// Override for one device.
    Device,
}

/// Params of `ui.getConfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetUiConfigParams {
    /// Client device id (per-device theme selection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
}

/// Result of `ui.getConfig` / `ui.setConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiConfigResult {
    /// Effective config.
    pub config: UiConfig,
    /// Version of the stored document it came from (0 = never stored).
    pub version: u64,
    /// Where it came from.
    pub scope: UiConfigScope,
    /// Contrast report of the effective theme.
    pub contrast: Vec<ContrastCheck>,
}

/// Params of `ui.setConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetUiConfigParams {
    /// New document; `null` (with `deviceId`) removes the device override.
    pub config: Option<UiConfig>,
    /// Write the override of this device instead of the global config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_id: Option<String>,
    /// Expected current version (optimistic concurrency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_version: Option<u64>,
}

/// A theme in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeEntry {
    /// The theme.
    pub theme: ThemeConfig,
    /// Built-in (read-only).
    pub builtin: bool,
    /// Last change of a user theme.
    pub updated_at: Option<orkestr_domain::Timestamp>,
}

/// Result of `ui.themes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemesResult {
    /// Built-ins first (Nebula = default), then user themes by name.
    pub themes: Vec<ThemeEntry>,
}

/// Params of `ui.saveTheme`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveThemeParams {
    /// Theme to store (validated).
    pub theme: ThemeConfig,
    /// Replace an existing user theme with the same id (otherwise `conflict`).
    #[serde(default)]
    pub overwrite: bool,
}

/// Result of `ui.saveTheme`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveThemeResult {
    /// Stored entry.
    pub entry: ThemeEntry,
    /// Contrast report (warnings are informational; hard failures were rejected).
    pub contrast: Vec<ContrastCheck>,
    /// Number of stored configs that used this theme and were updated.
    pub configs_updated: u32,
}

/// Params of `ui.deleteTheme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteThemeParams {
    /// User theme id.
    pub id: String,
}

/// Result of `ui.deleteTheme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteThemeResult {
    /// Whether a theme was removed.
    pub deleted: bool,
    /// Number of stored configs that fell back to Nebula.
    pub configs_reset: u32,
}

method!(
    /// `ui.getConfig`.
    GetConfig, "ui.getConfig", GetUiConfigParams, UiConfigResult
);
method!(
    /// `ui.setConfig`.
    SetConfig, "ui.setConfig", SetUiConfigParams, UiConfigResult
);
method!(
    /// `ui.themes`.
    Themes, "ui.themes", super::Empty, ThemesResult
);
method!(
    /// `ui.saveTheme`.
    SaveTheme, "ui.saveTheme", SaveThemeParams, SaveThemeResult
);
method!(
    /// `ui.deleteTheme`.
    DeleteTheme, "ui.deleteTheme", DeleteThemeParams, DeleteThemeResult
);

/// Params of `ui.previewPatch`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewUiPatchParams {
    /// Operations (may be empty).
    #[serde(default)]
    pub patch: Vec<UiPatchOp>,
    /// Author whose allowlist applies (default `User`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<UiPatchActor>,
}

/// Result of `ui.previewPatch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewUiPatchResult {
    /// The stored document.
    pub base: UiConfig,
    /// Its version (0 = never stored; defaults).
    pub version: u64,
    /// The document with the patch applied.
    pub config: UiConfig,
    /// What would change.
    pub changes: Vec<UiChange>,
    /// Patch that undoes the change.
    pub inverse: Vec<UiPatchOp>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

/// Params of `ui.applyPatch`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyUiPatchParams {
    /// Operations (1..=64).
    pub patch: Vec<UiPatchOp>,
    /// Version the patch was previewed against; a different stored version → `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_version: Option<u64>,
    /// Author (default `User`). AI-authored patches get the narrow allowlist
    /// even when the user applies them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<UiPatchActor>,
    /// One-line summary for the history (≤ 200 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub summary: Option<String>,
    /// The `UiPatchProposed` proposal this applies, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposal_id: Option<String>,
}

/// Result of `ui.applyPatch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyUiPatchResult {
    /// The stored document.
    pub config: UiConfig,
    /// Its new version.
    pub version: u64,
    /// What changed.
    pub changes: Vec<UiChange>,
    /// Patch that undoes the change.
    pub inverse: Vec<UiPatchOp>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

method!(
    /// `ui.previewPatch`.
    PreviewPatch, "ui.previewPatch", PreviewUiPatchParams, PreviewUiPatchResult
);
method!(
    /// `ui.applyPatch`.
    ApplyPatch, "ui.applyPatch", ApplyUiPatchParams, ApplyUiPatchResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids() {
        assert!(is_valid_device_id("0192f0a4-7b1c-7d2e-8f00-1234567890ab"));
        for bad in ["", "a/b", "x y", &"a".repeat(65)] {
            assert!(!is_valid_device_id(bad), "{bad}");
        }
    }

    #[test]
    fn set_config_params_accept_null_config() {
        let p: SetUiConfigParams = serde_json::from_str(r#"{"config":null,"deviceId":"d1"}"#).unwrap();
        assert!(p.config.is_none());
        assert_eq!(p.device_id.as_deref(), Some("d1"));
    }
}
