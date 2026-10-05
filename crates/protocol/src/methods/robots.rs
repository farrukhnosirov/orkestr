//! `robots.*` — floating robots and the Robot Designer (§30, §63 H, ADR-0016).
//!
//! A floating robot is a desktop window bound to a real `AgentId`. What a
//! robot looks like and how it behaves is **data** (this module), validated
//! here and mirrored by the desktop (`features/robots/design.ts`); robots never
//! run user-supplied code.
//!
//! * [`RobotsConfig`] — the global robots document: on/off, max visible
//!   robots, linger time after completion, the default [`RobotDesign`],
//!   per-template designs (keyed by `AgentTemplateId`, so built-in templates
//!   can be styled too although they are read-only) and remembered window
//!   positions (keyed by template id, per monitor).
//! * `robots.list` — agents with their resolved design (template design →
//!   default design, with the template's `AvatarConfig` accent / scale /
//!   `floating` flag applied).
//! * `robots.config` / `robots.setConfig` — read / patch the document. Patches
//!   are atomic in the core; one design or one position can be upserted
//!   without resending the whole document (robot windows save their own
//!   positions concurrently).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, AgentInstance, AvatarConfig, ProjectId};

use super::{method, Empty};
use crate::ui_config::UiConfigIssue;

/// Current schema version of [`RobotsConfig`].
pub const ROBOTS_SCHEMA_VERSION: u32 = 1;
/// Upper bound of `maxVisible` (ADR-0016 default 6).
pub const MAX_VISIBLE_LIMIT: u8 = 12;
/// Max stored designs / positions.
pub const MAX_ENTRIES: usize = 256;
/// Skin presets known to the renderers (2D and 3D).
pub const SKINS: &[&str] = &["nebula", "ember", "mint", "chrome", "sakura", "obsidian", "sunrise", "glacier"];
/// Accessories known to the renderers.
pub const ACCESSORIES: &[&str] = &["antenna", "headphones", "visor", "bowtie", "jetpack", "halo", "scarf"];
/// Sound presets (ids understood by the sound engine, §33).
pub const SOUND_PRESETS: &[&str] = &["none", "soft", "retro", "scifi", "chime"];

/// 2D (procedural SVG) or 3D (three.js, Ultra Visual only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RobotRenderMode {
    /// Procedural SVG (default, cheap).
    #[default]
    TwoD,
    /// Low-poly three.js robot; falls back to 2D outside Ultra Visual.
    ThreeD,
}

/// Surface material.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RobotMaterial {
    /// Soft matte.
    Matte,
    /// Glossy plastic (default).
    #[default]
    Glossy,
    /// Brushed metal.
    Metal,
    /// Emissive neon edges.
    Neon,
}

/// How much the speech bubble says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum BubbleVerbosity {
    /// Never show bubbles.
    Off,
    /// Only results (completed, failed, needs approval).
    Minimal,
    /// Results and state changes (default).
    #[default]
    Normal,
    /// Also every activity update.
    Chatty,
}

/// Robot voice (hook for the voice area, §33).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotVoice {
    /// Speak bubbles aloud.
    pub enabled: bool,
    /// TTS voice id (provider-specific; `None` = the agent's/template default).
    pub voice_id: Option<String>,
}

/// Behaviour knobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotBehaviour {
    /// Snap to screen edges/corners and to other robots after a drag.
    pub snap: bool,
    /// Hide the robot while its agent is idle.
    pub auto_hide_idle: bool,
    /// Let clicks pass through while idle (hover 600 ms to interact).
    pub click_through_idle: bool,
    /// Bubble verbosity.
    pub bubble: BubbleVerbosity,
}

impl Default for RobotBehaviour {
    fn default() -> Self {
        Self { snap: true, auto_hide_idle: false, click_through_idle: true, bubble: BubbleVerbosity::Normal }
    }
}

/// Everything the Robot Designer edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotDesign {
    /// 2D / 3D.
    pub mode: RobotRenderMode,
    /// Skin preset id (see [`SKINS`]).
    pub skin: String,
    /// Material.
    pub material: RobotMaterial,
    /// Accent color override (`#rrggbb`).
    pub accent: Option<String>,
    /// Scale (0.6..=1.6).
    pub scale: f32,
    /// Accessories (see [`ACCESSORIES`]), at most 4.
    pub accessories: Vec<String>,
    /// Animation intensity (0..=1).
    pub animation_intensity: f32,
    /// Voice.
    pub voice: RobotVoice,
    /// Sound preset (see [`SOUND_PRESETS`]).
    pub sounds: String,
    /// Behaviour.
    pub behaviour: RobotBehaviour,
}

impl Default for RobotDesign {
    fn default() -> Self {
        Self {
            mode: RobotRenderMode::TwoD,
            skin: "nebula".into(),
            material: RobotMaterial::Glossy,
            accent: None,
            scale: 1.0,
            accessories: vec!["antenna".into()],
            animation_intensity: 0.8,
            voice: RobotVoice::default(),
            sounds: "soft".into(),
            behaviour: RobotBehaviour::default(),
        }
    }
}

/// A remembered window position (physical desktop pixels of the window's
/// top-left corner) and the monitor it was on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotPosition {
    /// Monitor name (as reported by the OS); used to drop stale positions.
    pub monitor: Option<String>,
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
}

/// The robots document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotsConfig {
    /// Must equal [`ROBOTS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// "Show floating robots".
    pub enabled: bool,
    /// Max robots on the desktop; the rest collapse into a cluster (1..=12).
    pub max_visible: u8,
    /// Seconds a finished robot stays visible (celebration, result bubble), 0..=3600.
    pub linger_seconds: u32,
    /// Design used when a template has none.
    pub default_design: RobotDesign,
    /// Per-template designs (key: `AgentTemplateId`).
    pub designs: BTreeMap<String, RobotDesign>,
    /// Remembered positions (key: `AgentTemplateId`, or `cluster`).
    pub positions: BTreeMap<String, RobotPosition>,
}

impl Default for RobotsConfig {
    fn default() -> Self {
        Self {
            schema_version: ROBOTS_SCHEMA_VERSION,
            enabled: true,
            max_visible: 6,
            linger_seconds: 90,
            default_design: RobotDesign::default(),
            designs: BTreeMap::new(),
            positions: BTreeMap::new(),
        }
    }
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> UiConfigIssue {
    UiConfigIssue { path: path.into(), message: message.into() }
}

/// Whether `s` is `#rrggbb`.
#[must_use]
pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn is_valid_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl RobotDesign {
    /// Validates the design, pushing issues under `path`.
    pub fn validate_into(&self, path: &str, issues: &mut Vec<UiConfigIssue>) {
        if !SKINS.contains(&self.skin.as_str()) {
            issues.push(issue(format!("{path}.skin"), "unknown skin"));
        }
        if let Some(a) = &self.accent {
            if !is_hex_color(a) {
                issues.push(issue(format!("{path}.accent"), "must be #rrggbb"));
            }
        }
        if !(self.scale.is_finite() && (0.6..=1.6).contains(&self.scale)) {
            issues.push(issue(format!("{path}.scale"), "must be between 0.6 and 1.6"));
        }
        if !(self.animation_intensity.is_finite() && (0.0..=1.0).contains(&self.animation_intensity)) {
            issues.push(issue(format!("{path}.animationIntensity"), "must be between 0 and 1"));
        }
        if self.accessories.len() > 4 {
            issues.push(issue(format!("{path}.accessories"), "at most 4 accessories"));
        }
        for (i, a) in self.accessories.iter().enumerate() {
            if !ACCESSORIES.contains(&a.as_str()) {
                issues.push(issue(format!("{path}.accessories.{i}"), "unknown accessory"));
            }
        }
        if let Some(v) = &self.voice.voice_id {
            if v.is_empty() || v.len() > 120 || v.chars().any(char::is_control) {
                issues.push(issue(format!("{path}.voice.voiceId"), "must be 1..=120 printable characters"));
            }
        }
        if !SOUND_PRESETS.contains(&self.sounds.as_str()) {
            issues.push(issue(format!("{path}.sounds"), "unknown sound preset"));
        }
    }
}

impl RobotsConfig {
    /// Validates the whole document. Returns all issues found.
    pub fn validate(&self) -> Result<(), Vec<UiConfigIssue>> {
        let mut issues = Vec::new();
        if self.schema_version != ROBOTS_SCHEMA_VERSION {
            issues
                .push(issue("schemaVersion", format!("unsupported schema version (expected {ROBOTS_SCHEMA_VERSION})")));
        }
        if !(1..=MAX_VISIBLE_LIMIT).contains(&self.max_visible) {
            issues.push(issue("maxVisible", format!("must be between 1 and {MAX_VISIBLE_LIMIT}")));
        }
        if self.linger_seconds > 3600 {
            issues.push(issue("lingerSeconds", "must be at most 3600"));
        }
        self.default_design.validate_into("defaultDesign", &mut issues);
        if self.designs.len() > MAX_ENTRIES {
            issues.push(issue("designs", format!("at most {MAX_ENTRIES} designs")));
        }
        for (k, d) in &self.designs {
            if !is_valid_key(k) {
                issues.push(issue(format!("designs.{k}"), "invalid key"));
            }
            d.validate_into(&format!("designs.{k}"), &mut issues);
        }
        if self.positions.len() > MAX_ENTRIES {
            issues.push(issue("positions", format!("at most {MAX_ENTRIES} positions")));
        }
        for (k, p) in &self.positions {
            if !is_valid_key(k) {
                issues.push(issue(format!("positions.{k}"), "invalid key"));
            }
            let in_range = |v: f64| v.is_finite() && v.abs() <= 100_000.0;
            if !in_range(p.x) || !in_range(p.y) {
                issues.push(issue(format!("positions.{k}"), "coordinates out of range"));
            }
            if p.monitor.as_ref().is_some_and(|m| m.len() > 200) {
                issues.push(issue(format!("positions.{k}.monitor"), "too long"));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    /// The design of a template (its own, else the default) with the template's
    /// avatar accent / scale applied when the design does not override them,
    /// and whether the agent shows a floating robot at all.
    #[must_use]
    pub fn resolve(&self, template_id: Option<&str>, avatar: Option<&AvatarConfig>) -> (RobotDesign, bool) {
        let own = template_id.and_then(|id| self.designs.get(id));
        let mut design = own.cloned().unwrap_or_else(|| self.default_design.clone());
        if own.is_none() {
            if let Some(a) = avatar {
                if design.accent.is_none() {
                    design.accent = a.accent.clone().filter(|c| is_hex_color(c));
                }
                if a.scale.is_finite() {
                    design.scale = a.scale.clamp(0.6, 1.6);
                }
                if SKINS.contains(&a.robot.as_str()) {
                    design.skin.clone_from(&a.robot);
                }
            }
        }
        let floating = avatar.is_none_or(|a| a.floating);
        (design, floating)
    }
}

/// Params of `robots.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRobotsParams {
    /// Only this project's agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Only these agents (robot windows ask for their own agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_ids: Option<Vec<AgentId>>,
    /// Include agents whose run has ended (default false: active only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_finished: Option<bool>,
}

/// One robot: an agent plus its resolved design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotEntry {
    /// The agent the robot is bound to.
    pub agent: AgentInstance,
    /// Resolved design.
    pub design: RobotDesign,
    /// Whether the template wants a floating robot (`AvatarConfig.floating`).
    pub floating: bool,
}

/// Result of `robots.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRobotsResult {
    /// Robots, newest agent first.
    pub robots: Vec<RobotEntry>,
}

/// Result of `robots.config` / `robots.setConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotsConfigResult {
    /// The document.
    pub config: RobotsConfig,
}

/// Upsert (or remove with `design: null`) one template design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotDesignUpsert {
    /// Template id.
    pub template_id: String,
    /// New design; `null` resets the template to the default design.
    pub design: Option<RobotDesign>,
}

/// Upsert (or forget with `position: null`) one remembered position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobotPositionUpsert {
    /// Key (template id or `cluster`).
    pub key: String,
    /// Position; `null` forgets it.
    pub position: Option<RobotPosition>,
}

/// Params of `robots.setConfig`: every field is optional and applied atomically.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetRobotsConfigParams {
    /// Show floating robots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// Max visible robots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_visible: Option<u8>,
    /// Linger seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub linger_seconds: Option<u32>,
    /// Default design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_design: Option<RobotDesign>,
    /// One template design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub design: Option<RobotDesignUpsert>,
    /// One position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub position: Option<RobotPositionUpsert>,
}

impl SetRobotsConfigParams {
    /// Applies the patch to `config` (the result still has to be validated).
    #[must_use]
    pub fn apply(self, mut config: RobotsConfig) -> RobotsConfig {
        if let Some(v) = self.enabled {
            config.enabled = v;
        }
        if let Some(v) = self.max_visible {
            config.max_visible = v;
        }
        if let Some(v) = self.linger_seconds {
            config.linger_seconds = v;
        }
        if let Some(v) = self.default_design {
            config.default_design = v;
        }
        if let Some(u) = self.design {
            match u.design {
                Some(d) => {
                    config.designs.insert(u.template_id, d);
                }
                None => {
                    config.designs.remove(&u.template_id);
                }
            }
        }
        if let Some(u) = self.position {
            match u.position {
                Some(p) => {
                    config.positions.insert(u.key, p);
                }
                None => {
                    config.positions.remove(&u.key);
                }
            }
        }
        config
    }
}

method!(
    /// `robots.list` — agents with their resolved robot design.
    List, "robots.list", ListRobotsParams, ListRobotsResult
);
method!(
    /// `robots.config` — the robots document.
    Config, "robots.config", Empty, RobotsConfigResult
);
method!(
    /// `robots.setConfig` — patch the robots document (validated, atomic).
    SetConfig, "robots.setConfig", SetRobotsConfigParams, RobotsConfigResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        RobotsConfig::default().validate().unwrap();
    }

    #[test]
    fn reports_bad_designs() {
        let mut cfg = RobotsConfig { max_visible: 0, linger_seconds: 9999, ..RobotsConfig::default() };
        cfg.default_design.skin = "plaid".into();
        cfg.default_design.scale = 9.0;
        cfg.default_design.accent = Some("red".into());
        cfg.default_design.accessories = vec!["laser".into()];
        cfg.designs.insert("bad key!".into(), RobotDesign { sounds: "boom".into(), ..RobotDesign::default() });
        cfg.positions.insert("cluster".into(), RobotPosition { monitor: None, x: f64::NAN, y: 0.0 });
        let issues = cfg.validate().unwrap_err();
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        for p in [
            "maxVisible",
            "lingerSeconds",
            "defaultDesign.skin",
            "defaultDesign.scale",
            "defaultDesign.accent",
            "defaultDesign.accessories.0",
            "designs.bad key!",
            "designs.bad key!.sounds",
            "positions.cluster",
        ] {
            assert!(paths.contains(&p), "missing {p} in {paths:?}");
        }
    }

    #[test]
    fn patch_upserts_and_removes() {
        let tpl = "0190a0b0-0000-7000-8000-000000000001".to_owned();
        let design = RobotDesign { skin: "ember".into(), ..RobotDesign::default() };
        let cfg = SetRobotsConfigParams {
            max_visible: Some(3),
            design: Some(RobotDesignUpsert { template_id: tpl.clone(), design: Some(design.clone()) }),
            position: Some(RobotPositionUpsert {
                key: tpl.clone(),
                position: Some(RobotPosition { monitor: Some("DISPLAY1".into()), x: 10.0, y: 20.0 }),
            }),
            ..SetRobotsConfigParams::default()
        }
        .apply(RobotsConfig::default());
        assert_eq!(cfg.max_visible, 3);
        assert_eq!(cfg.designs.get(&tpl), Some(&design));
        assert!(cfg.positions.contains_key(&tpl));
        cfg.validate().unwrap();
        let cfg = SetRobotsConfigParams {
            design: Some(RobotDesignUpsert { template_id: tpl.clone(), design: None }),
            position: Some(RobotPositionUpsert { key: tpl.clone(), position: None }),
            ..SetRobotsConfigParams::default()
        }
        .apply(cfg);
        assert!(cfg.designs.is_empty() && cfg.positions.is_empty());
    }

    #[test]
    fn resolve_prefers_template_design_then_avatar() {
        let mut cfg = RobotsConfig::default();
        let avatar = AvatarConfig { robot: "mint".into(), accent: Some("#ff00aa".into()), scale: 3.0, floating: false };
        let (d, floating) = cfg.resolve(Some("t1"), Some(&avatar));
        assert_eq!(d.skin, "mint");
        assert_eq!(d.accent.as_deref(), Some("#ff00aa"));
        assert!((d.scale - 1.6).abs() < f32::EPSILON, "avatar scale is clamped");
        assert!(!floating);
        let own = RobotDesign { skin: "chrome".into(), ..RobotDesign::default() };
        cfg.designs.insert("t1".into(), own.clone());
        let (d, floating) = cfg.resolve(Some("t1"), None);
        assert_eq!(d, own);
        assert!(floating, "no avatar = floating");
        let (d, _) = cfg.resolve(None, None);
        assert_eq!(d, cfg.default_design);
    }
}
