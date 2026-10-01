//! Versioned UI configuration schema (§29 Theme Engine / Layout Editor,
//! §31 Performance Mode, contract §8).
//!
//! The UI (and the AI Manager, when it "edits the UI") only ever changes this
//! **data**; it is validated here and by `@orkestr/ui`'s theme engine and then
//! applied as CSS variables — never evaluated as code.
//!
//! Token values use a deliberately small grammar: colors (`#hex`, `rgb()`,
//! `rgba()`, `hsl()`, `hsla()`), lengths/numbers, font-family lists,
//! gradients and `var(--ok-…)` references. `url(`, `expression(`,
//! `javascript:`, `@import`, `;`, `{`, `}`, `<`, `>` and `\` are rejected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Current schema version. Increment on breaking changes and add a migration
/// in both the Rust validator and the TS theme engine.
pub const UI_CONFIG_SCHEMA_VERSION: u32 = 1;

/// Max number of tokens in a theme.
pub const MAX_TOKENS: usize = 256;
/// Max length of a token value.
pub const MAX_TOKEN_VALUE_LEN: usize = 256;

/// Light or dark base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ThemeMode {
    /// Dark (default; "Nebula").
    #[default]
    Dark,
    /// Light.
    Light,
}

/// Visual effects knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeEffects {
    /// Backdrop blur of glass panels in px (0..=64).
    pub blur_px: f32,
    /// Glow intensity (0..=1).
    pub glow_intensity: f32,
    /// Panel background opacity (0.2..=1).
    pub panel_opacity: f32,
    /// Enable animations.
    pub animations: bool,
}

impl Default for ThemeEffects {
    fn default() -> Self {
        Self { blur_px: 18.0, glow_intensity: 0.6, panel_opacity: 0.72, animations: true }
    }
}

/// A theme: token overrides on top of the built-in defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeConfig {
    /// Stable id (`nebula`, `my-theme`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Base mode.
    pub mode: ThemeMode,
    /// Token overrides. Keys are token names **without** the `--ok-` prefix
    /// (`color-bg`, `radius-md`); values follow the grammar in the module docs.
    pub tokens: BTreeMap<String, String>,
    /// Effects.
    pub effects: ThemeEffects,
}

/// Layout presets (§29).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum LayoutPreset {
    /// Code-centric.
    #[default]
    Coding,
    /// Mission Control (§32).
    MissionControl,
    /// Monitoring.
    Monitoring,
    /// Focus.
    Focus,
    /// User-defined.
    Custom,
}

/// Docking layout.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LayoutConfig {
    /// Active preset.
    pub preset: LayoutPreset,
    /// Serialized docking layout (dockview JSON) for `Custom`. Opaque to the
    /// core; the desktop validates it against its panel registry before use.
    pub docking: Option<serde_json::Value>,
}

/// Performance modes (§31).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PerformanceMode {
    /// Everything on.
    UltraVisual,
    /// Default.
    #[default]
    Balanced,
    /// Minimal effects.
    Performance,
    /// Lowest power.
    BatterySaver,
}

/// The whole UI configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiConfig {
    /// Must equal [`UI_CONFIG_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Theme.
    pub theme: ThemeConfig,
    /// Layout.
    pub layout: LayoutConfig,
    /// Performance mode.
    pub performance_mode: PerformanceMode,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            schema_version: UI_CONFIG_SCHEMA_VERSION,
            theme: ThemeConfig {
                id: "nebula".into(),
                name: "Nebula".into(),
                mode: ThemeMode::Dark,
                tokens: BTreeMap::new(),
                effects: ThemeEffects::default(),
            },
            layout: LayoutConfig::default(),
            performance_mode: PerformanceMode::default(),
        }
    }
}

/// One validation problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiConfigIssue {
    /// JSON-pointer-like path (`theme.tokens.color-bg`).
    pub path: String,
    /// Message.
    pub message: String,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> UiConfigIssue {
    UiConfigIssue { path: path.into(), message: message.into() }
}

/// Whether a token name is acceptable (`^[a-z][a-z0-9-]{0,63}$`).
#[must_use]
pub fn is_valid_token_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether a token value is acceptable (see module docs).
#[must_use]
pub fn is_valid_token_value(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_TOKEN_VALUE_LEN {
        return false;
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || " #%.,()-+/'\"_!".contains(c);
    if !value.chars().all(allowed) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    !["url(", "expression(", "javascript:", "@import", "image-set(", "attr("].iter().any(|bad| lower.contains(bad))
}

fn check_range(issues: &mut Vec<UiConfigIssue>, path: &str, v: f32, min: f32, max: f32) {
    if !(v.is_finite() && v >= min && v <= max) {
        issues.push(issue(path, format!("must be between {min} and {max}")));
    }
}

impl UiConfig {
    /// Validates the document. Returns all issues found.
    pub fn validate(&self) -> Result<(), Vec<UiConfigIssue>> {
        let mut issues = Vec::new();
        if self.schema_version != UI_CONFIG_SCHEMA_VERSION {
            issues.push(issue(
                "schemaVersion",
                format!("unsupported schema version {} (expected {UI_CONFIG_SCHEMA_VERSION})", self.schema_version),
            ));
        }
        let t = &self.theme;
        if !is_valid_token_name(&t.id) {
            issues.push(issue("theme.id", "must match ^[a-z][a-z0-9-]{0,63}$"));
        }
        if t.name.trim().is_empty() || t.name.chars().count() > 80 {
            issues.push(issue("theme.name", "must be 1..=80 characters"));
        }
        if t.tokens.len() > MAX_TOKENS {
            issues.push(issue("theme.tokens", format!("at most {MAX_TOKENS} tokens")));
        }
        for (k, v) in &t.tokens {
            if !is_valid_token_name(k) {
                issues.push(issue(format!("theme.tokens.{k}"), "invalid token name"));
            }
            if !is_valid_token_value(v) {
                issues.push(issue(format!("theme.tokens.{k}"), "invalid or unsafe token value"));
            }
        }
        check_range(&mut issues, "theme.effects.blurPx", t.effects.blur_px, 0.0, 64.0);
        check_range(&mut issues, "theme.effects.glowIntensity", t.effects.glow_intensity, 0.0, 1.0);
        check_range(&mut issues, "theme.effects.panelOpacity", t.effects.panel_opacity, 0.2, 1.0);
        if let Some(docking) = &self.layout.docking {
            let size = serde_json::to_vec(docking).map(|v| v.len()).unwrap_or(usize::MAX);
            if size > 256 * 1024 {
                issues.push(issue("layout.docking", "docking layout too large (max 256 KiB)"));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        UiConfig::default().validate().unwrap();
    }

    #[test]
    fn token_values() {
        for ok in [
            "#0a0f1e",
            "rgba(10, 20, 30, 0.5)",
            "12px",
            "'Inter', system-ui, sans-serif",
            "var(--ok-color-accent)",
            "linear-gradient(135deg, #22d3ee 0%, #8b5cf6 100%)",
            "0 0 24px rgba(34,211,238,0.45)",
        ] {
            assert!(is_valid_token_value(ok), "{ok}");
        }
        for bad in [
            "url(http://x)",
            "red; background: url(x)",
            "expression(alert(1))",
            "<script>",
            "a{b}",
            "",
            "JavaScript:alert(1)",
        ] {
            assert!(!is_valid_token_value(bad), "{bad}");
        }
    }

    #[test]
    fn reports_all_issues() {
        let mut cfg = UiConfig { schema_version: 99, ..UiConfig::default() };
        cfg.theme.tokens.insert("Bad Name".into(), "#fff".into());
        cfg.theme.tokens.insert("color-bg".into(), "url(x)".into());
        cfg.theme.effects.glow_intensity = 3.0;
        let issues = cfg.validate().unwrap_err();
        assert_eq!(issues.len(), 4, "{issues:?}");
    }
}
