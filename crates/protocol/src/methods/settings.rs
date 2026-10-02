//! `settings.*` — typed application settings (§29, §31, §55, RR-A8, RR-G13).
//!
//! Settings are a key/value JSON store in the core (one row per top-level
//! field), exposed as the typed [`AppSettings`]. Unknown or missing keys fall
//! back to defaults, so adding a field never needs a data migration.
//! `settings.update` applies an [`AppSettingsPatch`], performs side effects
//! (autostart registration) and publishes `SettingsChanged`.
//!
//! `ui.getConfig` / `ui.setConfig` (validated UI config, `crate::ui_config`)
//! remain reserved for the theme engine.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::ApprovalMode;

use super::method;

/// Method names planned for this area (not yet routed by the core).
pub const PLANNED: &[&str] = &["ui.getConfig", "ui.setConfig"];

/// Performance mode (§31) — the same enum the UI config uses. Phase 1 stores
/// the choice; the scheduler and effects engine honour it in later phases.
pub use crate::ui_config::PerformanceMode;

/// Where project data lives (§55 step 2, RR-A8). Only `LocalOnly` is
/// selectable before Phase 4.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum StorageMode {
    /// Everything stays on this machine.
    #[default]
    LocalOnly,
    /// Self-hosted sync server (Phase 4).
    SelfHosted,
    /// Orkestr cloud (Phase 4).
    Cloud,
}

/// Default event retention (RR-G13).
pub const DEFAULT_EVENT_RETENTION_DAYS: u32 = 180;
/// Maximum configurable retention.
pub const MAX_EVENT_RETENTION_DAYS: u32 = 3650;

/// All application settings. Every field has a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AppSettings {
    /// Local profile display name (onboarding step 1).
    pub profile_name: String,
    /// Storage mode (onboarding step 2).
    pub storage_mode: StorageMode,
    /// Default approval mode for new projects (onboarding step 5).
    pub default_approval_mode: ApprovalMode,
    /// Performance mode (§31).
    pub performance_mode: PerformanceMode,
    /// Closing the main window hides it to the tray; agents keep running (§12).
    pub close_to_tray: bool,
    /// Start Orkestr at OS login (§12, opt-in).
    pub autostart: bool,
    /// Active theme id.
    pub theme_id: String,
    /// Anonymous usage telemetry (off; no telemetry is collected in Phase 1).
    pub telemetry_enabled: bool,
    /// Durable events older than this are pruned (approvals, decisions,
    /// emergency stops and rollbacks are kept). 0 = keep forever.
    pub event_retention_days: u32,
    /// UI language preference: `system` (follow the OS language; the default
    /// for new installs) or a BCP-47 tag such as `ru` / `en`. The UI resolves
    /// `system` itself; AI Manager and agents reply in the resolved language.
    pub language: String,
    /// In-app notifications: do not disturb (no toasts; history still recorded).
    pub do_not_disturb: bool,
    /// Onboarding wizard finished or skipped.
    pub onboarding_completed: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            profile_name: String::new(),
            storage_mode: StorageMode::LocalOnly,
            default_approval_mode: ApprovalMode::Ask,
            performance_mode: PerformanceMode::Balanced,
            close_to_tray: true,
            autostart: false,
            theme_id: "nebula".into(),
            telemetry_enabled: false,
            event_retention_days: DEFAULT_EVENT_RETENTION_DAYS,
            language: LANGUAGE_SYSTEM.into(),
            do_not_disturb: false,
            onboarding_completed: false,
        }
    }
}

/// Partial update; absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[allow(missing_docs)]
pub struct AppSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub storage_mode: Option<StorageMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_approval_mode: Option<ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub performance_mode: Option<PerformanceMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub close_to_tray: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub autostart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub theme_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub telemetry_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub event_retention_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub do_not_disturb: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub onboarding_completed: Option<bool>,
}

/// Validation failure of a settings patch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct SettingsValidationError {
    /// camelCase field name.
    pub field: &'static str,
    /// What is wrong.
    pub message: String,
}

fn invalid(field: &'static str, message: impl Into<String>) -> SettingsValidationError {
    SettingsValidationError { field, message: message.into() }
}

impl AppSettingsPatch {
    /// Validates every present field.
    pub fn validate(&self) -> Result<(), SettingsValidationError> {
        if let Some(n) = &self.profile_name {
            if n.chars().count() > 80 || n.chars().any(char::is_control) {
                return Err(invalid("profileName", "at most 80 printable characters"));
            }
        }
        if let Some(mode) = self.storage_mode {
            if mode != StorageMode::LocalOnly {
                return Err(invalid("storageMode", "only LocalOnly is available before Phase 4"));
            }
        }
        if let Some(t) = &self.theme_id {
            let ok = !t.is_empty() && t.len() <= 48 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !ok {
                return Err(invalid("themeId", "theme ids are 1..=48 chars of [A-Za-z0-9-]"));
            }
        }
        if let Some(d) = self.event_retention_days {
            if d > MAX_EVENT_RETENTION_DAYS {
                return Err(invalid(
                    "eventRetentionDays",
                    format!("at most {MAX_EVENT_RETENTION_DAYS} days (0 = forever)"),
                ));
            }
        }
        if let Some(l) = &self.language {
            if l != LANGUAGE_SYSTEM && !is_language_tag(l) {
                return Err(invalid("language", "expected `system` or a language tag like `en` or `pt-BR`"));
            }
        }
        Ok(())
    }

    /// Applies the patch; returns the camelCase names of fields whose value changed.
    pub fn apply_to(&self, s: &mut AppSettings) -> Vec<&'static str> {
        let mut changed = Vec::new();
        macro_rules! set {
            ($field:ident, $name:literal) => {
                if let Some(v) = &self.$field {
                    if s.$field != *v {
                        s.$field = v.clone();
                        changed.push($name);
                    }
                }
            };
        }
        set!(profile_name, "profileName");
        set!(storage_mode, "storageMode");
        set!(default_approval_mode, "defaultApprovalMode");
        set!(performance_mode, "performanceMode");
        set!(close_to_tray, "closeToTray");
        set!(autostart, "autostart");
        set!(theme_id, "themeId");
        set!(telemetry_enabled, "telemetryEnabled");
        set!(event_retention_days, "eventRetentionDays");
        set!(language, "language");
        set!(do_not_disturb, "doNotDisturb");
        set!(onboarding_completed, "onboardingCompleted");
        changed
    }
}

/// `AppSettings::language` value meaning "follow the operating system".
pub const LANGUAGE_SYSTEM: &str = "system";

fn is_language_tag(s: &str) -> bool {
    let mut parts = s.split('-');
    let primary_ok =
        parts.next().is_some_and(|p| (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_lowercase()));
    primary_ok && parts.all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Result of `settings.get` / `settings.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsResult {
    /// Current settings.
    pub settings: AppSettings,
    /// Whether this OS supports autostart registration.
    pub autostart_supported: bool,
}

/// Params of `settings.update`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateSettingsParams {
    /// Fields to change.
    pub patch: AppSettingsPatch,
}

method!(
    /// `settings.get`.
    Get, "settings.get", super::Empty, SettingsResult
);
method!(
    /// `settings.update`.
    Update, "settings.update", UpdateSettingsParams, SettingsResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_names_are_valid() {
        for name in PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }

    #[test]
    fn missing_fields_default() {
        let s: AppSettings = serde_json::from_str(r#"{"closeToTray": false, "futureField": 1}"#).unwrap();
        assert!(!s.close_to_tray);
        assert_eq!(s.event_retention_days, DEFAULT_EVENT_RETENTION_DAYS);
        assert_eq!(s.theme_id, "nebula");
    }

    #[test]
    fn patch_validation_and_apply() {
        let mut s = AppSettings::default();
        let p = AppSettingsPatch { close_to_tray: Some(false), language: Some("pt-BR".into()), ..Default::default() };
        p.validate().unwrap();
        assert_eq!(p.apply_to(&mut s), vec!["closeToTray", "language"]);
        assert!(p.apply_to(&mut s).is_empty(), "idempotent");
        assert_eq!(AppSettings::default().language, LANGUAGE_SYSTEM, "new installs follow the OS language");
        AppSettingsPatch { language: Some(LANGUAGE_SYSTEM.into()), ..Default::default() }.validate().unwrap();
        AppSettingsPatch { language: Some("ru".into()), ..Default::default() }.validate().unwrap();
        for bad in [
            AppSettingsPatch { storage_mode: Some(StorageMode::Cloud), ..Default::default() },
            AppSettingsPatch { theme_id: Some("../x".into()), ..Default::default() },
            AppSettingsPatch { event_retention_days: Some(99_999), ..Default::default() },
            AppSettingsPatch { language: Some("English".into()), ..Default::default() },
            AppSettingsPatch { profile_name: Some("a\u{0007}".into()), ..Default::default() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }
}
