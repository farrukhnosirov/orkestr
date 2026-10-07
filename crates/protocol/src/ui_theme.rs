//! Theme schema details (§29 Theme Engine, ADR-0015): typed theme sections,
//! the token catalogue with per-kind value grammars, a strict CSS color
//! parser and WCAG contrast checks, the theme exchange file format and the
//! built-in themes.
//!
//! `@orkestr/ui` (`packages/ui/src/theme/*`) implements **the same rules**;
//! both sides are tested against the shared fixtures in
//! `crates/protocol/fixtures/ui_theme/`. The core is the only trusted
//! validator — the UI validates to give instant feedback.
//!
//! Sections are additive to `ThemeConfig` (v1 documents without them load
//! with defaults; `UI_CONFIG_SCHEMA_VERSION` 2 still accepts v1). Exported theme
//! files carry their own [`THEME_SCHEMA_VERSION`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ui_config::{is_valid_token_name, ThemeConfig, ThemeMode, UiConfigIssue};

/// Version of the theme exchange format ([`ThemeFile`]). v1 = tokens + effects
/// only; v2 adds background, motion, sounds, robot defaults and metadata.
pub const THEME_SCHEMA_VERSION: u32 = 2;
/// `format` marker of exported theme files.
pub const THEME_FILE_FORMAT: &str = "orkestr-theme";
/// Max user themes stored by the core.
pub const MAX_USER_THEMES: usize = 200;
/// Max sound mappings per theme.
pub const MAX_SOUND_EVENTS: usize = 64;
/// Max procedural background colors.
pub const MAX_BACKGROUND_COLORS: usize = 4;

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// Procedural background kinds (rendered by the desktop with perf tiers:
/// CSS/canvas, WebGL only in Ultra Visual).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum BackgroundKind {
    /// Flat `color-bg`.
    Solid,
    /// Static layered gradient (`gradient-space`).
    Gradient,
    /// Animated nebula clouds.
    #[default]
    Nebula,
    /// Perspective grid.
    Grid,
    /// Drifting particles.
    Particles,
}

/// App background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ThemeBackground {
    /// What to render.
    pub kind: BackgroundKind,
    /// Palette for procedural kinds (0..=4 colors; empty = derive from accents).
    pub colors: Vec<String>,
    /// Visual strength (0..=1).
    pub intensity: f32,
    /// Animation speed (0..=2, 0 = static).
    pub speed: f32,
    /// Density of grid lines / particles (0..=1).
    pub density: f32,
    /// Allow the WebGL renderer (used only in Ultra Visual).
    pub webgl: bool,
}

impl Default for ThemeBackground {
    fn default() -> Self {
        Self {
            kind: BackgroundKind::Nebula,
            colors: Vec::new(),
            intensity: 0.6,
            speed: 0.35,
            density: 0.5,
            webgl: true,
        }
    }
}

/// Motion knobs (durations/easings themselves are tokens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ThemeMotion {
    /// Amplitude of decorative motion (0..=1; 0 = none).
    pub intensity: f32,
    /// Duration multiplier divisor (0.5..=2; 2 = twice as fast).
    pub speed: f32,
}

impl Default for ThemeMotion {
    fn default() -> Self {
        Self { intensity: 1.0, speed: 1.0 }
    }
}

/// Sound mapping (the sound engine lives in the voice/sound area).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ThemeSounds {
    /// Master switch for theme sounds.
    pub enabled: bool,
    /// Volume (0..=1).
    pub volume: f32,
    /// Event key (`task.completed`, `approval.requested`) → sound preset id
    /// (`chime-soft`, `none`).
    pub events: BTreeMap<String, String>,
}

impl Default for ThemeSounds {
    fn default() -> Self {
        Self { enabled: true, volume: 0.6, events: BTreeMap::new() }
    }
}

/// Default look of floating robots (§30; the robot engine owns the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ThemeRobotDefaults {
    /// Skin preset id.
    pub skin: String,
    /// Scale (0.5..=2).
    pub scale: f32,
    /// Accent color override (`null` = theme accent).
    pub accent: Option<String>,
}

impl Default for ThemeRobotDefaults {
    fn default() -> Self {
        Self { skin: "nebula".into(), scale: 1.0, accent: None }
    }
}

// ---------------------------------------------------------------------------
// Token catalogue
// ---------------------------------------------------------------------------

/// Value grammar of a known token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TokenKind {
    /// CSS color.
    Color,
    /// Gradient (or a color).
    Gradient,
    /// Box shadow / focus ring (generic safe value).
    Shadow,
    /// Length (`px`, `rem`, `em`, `%`).
    Length,
    /// Font family list.
    FontFamily,
    /// Font weight (100..=900).
    FontWeight,
    /// Plain number.
    Number,
    /// Duration (`ms`, `s`; ≤ 10 s).
    Duration,
    /// Easing function.
    Easing,
}

/// Every token the design system defines (`--ok-<name>`), with its grammar.
/// Unknown token names stay allowed (generic safe grammar) for forward
/// compatibility.
pub const TOKEN_CATALOGUE: &[(&str, TokenKind)] = &[
    ("color-bg", TokenKind::Color),
    ("color-bg-elevated", TokenKind::Color),
    ("color-surface", TokenKind::Color),
    ("color-surface-strong", TokenKind::Color),
    ("color-surface-sunken", TokenKind::Color),
    ("color-surface-hover", TokenKind::Color),
    ("color-surface-active", TokenKind::Color),
    ("color-overlay", TokenKind::Color),
    ("color-border", TokenKind::Color),
    ("color-border-strong", TokenKind::Color),
    ("color-border-accent", TokenKind::Color),
    ("color-text", TokenKind::Color),
    ("color-text-muted", TokenKind::Color),
    ("color-text-subtle", TokenKind::Color),
    ("color-text-inverse", TokenKind::Color),
    ("color-accent", TokenKind::Color),
    ("color-accent-strong", TokenKind::Color),
    ("color-accent-soft", TokenKind::Color),
    ("color-violet", TokenKind::Color),
    ("color-violet-strong", TokenKind::Color),
    ("color-violet-soft", TokenKind::Color),
    ("color-magenta", TokenKind::Color),
    ("color-success", TokenKind::Color),
    ("color-success-soft", TokenKind::Color),
    ("color-warning", TokenKind::Color),
    ("color-warning-soft", TokenKind::Color),
    ("color-danger", TokenKind::Color),
    ("color-danger-soft", TokenKind::Color),
    ("color-info", TokenKind::Color),
    ("color-info-soft", TokenKind::Color),
    ("gradient-accent", TokenKind::Gradient),
    ("gradient-accent-soft", TokenKind::Gradient),
    ("gradient-panel", TokenKind::Gradient),
    ("gradient-space", TokenKind::Gradient),
    ("glow-accent", TokenKind::Shadow),
    ("glow-violet", TokenKind::Shadow),
    ("shadow-sm", TokenKind::Shadow),
    ("shadow-md", TokenKind::Shadow),
    ("shadow-panel", TokenKind::Shadow),
    ("focus-ring", TokenKind::Shadow),
    ("radius-xs", TokenKind::Length),
    ("radius-sm", TokenKind::Length),
    ("radius-md", TokenKind::Length),
    ("radius-lg", TokenKind::Length),
    ("radius-xl", TokenKind::Length),
    ("radius-pill", TokenKind::Length),
    ("space-1", TokenKind::Length),
    ("space-2", TokenKind::Length),
    ("space-3", TokenKind::Length),
    ("space-4", TokenKind::Length),
    ("space-5", TokenKind::Length),
    ("space-6", TokenKind::Length),
    ("space-8", TokenKind::Length),
    ("space-10", TokenKind::Length),
    ("font-sans", TokenKind::FontFamily),
    ("font-display", TokenKind::FontFamily),
    ("font-mono", TokenKind::FontFamily),
    ("font-size-xs", TokenKind::Length),
    ("font-size-sm", TokenKind::Length),
    ("font-size-md", TokenKind::Length),
    ("font-size-lg", TokenKind::Length),
    ("font-size-xl", TokenKind::Length),
    ("font-size-2xl", TokenKind::Length),
    ("font-weight-regular", TokenKind::FontWeight),
    ("font-weight-medium", TokenKind::FontWeight),
    ("font-weight-semibold", TokenKind::FontWeight),
    ("line-height", TokenKind::Number),
    ("letter-spacing-caps", TokenKind::Length),
    ("duration-fast", TokenKind::Duration),
    ("duration-base", TokenKind::Duration),
    ("duration-slow", TokenKind::Duration),
    ("ease", TokenKind::Easing),
    ("ease-spring", TokenKind::Easing),
];

/// Grammar of a known token, `None` for free-form names.
#[must_use]
pub fn token_kind(name: &str) -> Option<TokenKind> {
    TOKEN_CATALOGUE.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

// ---------------------------------------------------------------------------
// Grammars (identical in packages/ui/src/theme/grammar.ts)
// ---------------------------------------------------------------------------

/// `-?(\d+(\.\d*)?|\.\d+)` — no exponents, no `inf`/`nan`.
fn plain_number(s: &str) -> Option<f64> {
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let dots = body.chars().filter(|c| *c == '.').count();
    if dots > 1 || body == "." {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `var(--ok-<token>)`.
#[must_use]
pub fn is_var_ref(s: &str) -> bool {
    s.trim().strip_prefix("var(--ok-").and_then(|r| r.strip_suffix(')')).is_some_and(is_valid_token_name)
}

/// An sRGB color with straight alpha. Channels are 0..=255 (not rounded).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rgba {
    /// Red 0..=255.
    pub r: f64,
    /// Green 0..=255.
    pub g: f64,
    /// Blue 0..=255.
    pub b: f64,
    /// Alpha 0..=1.
    pub a: f64,
}

fn fn_args<'a>(s: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = s.strip_prefix(name)?.trim_start().strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect())
}

fn percent(s: &str) -> Option<f64> {
    s.strip_suffix('%').and_then(plain_number).filter(|v| (0.0..=100.0).contains(v))
}

fn alpha(s: &str) -> Option<f64> {
    match s.strip_suffix('%') {
        Some(_) => percent(s).map(|v| v / 100.0),
        None => plain_number(s).filter(|v| (0.0..=1.0).contains(v)),
    }
}

fn channel(s: &str) -> Option<f64> {
    match s.strip_suffix('%') {
        Some(_) => percent(s).map(|v| v * 2.55),
        None => plain_number(s).filter(|v| (0.0..=255.0).contains(v)),
    }
}

fn hex_digit(c: char) -> Option<f64> {
    c.to_digit(16).map(f64::from)
}

/// Parses `#rgb[a]`, `#rrggbb[aa]`, `rgb[a]()`, `hsl[a]()` (comma or space
/// syntax), `transparent`, `black`, `white`. Anything else → `None`.
#[must_use]
pub fn parse_color(input: &str) -> Option<Rgba> {
    let s = input.trim().to_ascii_lowercase();
    match s.as_str() {
        "transparent" => return Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
        "black" => return Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
        "white" => return Some(Rgba { r: 255.0, g: 255.0, b: 255.0, a: 1.0 }),
        _ => {}
    }
    if let Some(hex) = s.strip_prefix('#') {
        let digits: Option<Vec<f64>> = hex.chars().map(hex_digit).collect();
        let d = digits?;
        return match d.len() {
            3 | 4 => Some(Rgba {
                r: d[0] * 17.0,
                g: d[1] * 17.0,
                b: d[2] * 17.0,
                a: d.get(3).map_or(1.0, |v| v * 17.0 / 255.0),
            }),
            6 | 8 => Some(Rgba {
                r: d[0] * 16.0 + d[1],
                g: d[2] * 16.0 + d[3],
                b: d[4] * 16.0 + d[5],
                a: if d.len() == 8 { (d[6] * 16.0 + d[7]) / 255.0 } else { 1.0 },
            }),
            _ => None,
        };
    }
    let rgb = fn_args(&s, "rgba").or_else(|| fn_args(&s, "rgb"));
    if let Some(args) = rgb {
        if !(3..=4).contains(&args.len()) {
            return None;
        }
        return Some(Rgba {
            r: channel(args[0])?,
            g: channel(args[1])?,
            b: channel(args[2])?,
            a: match args.get(3) {
                Some(a) => alpha(a)?,
                None => 1.0,
            },
        });
    }
    let hsl = fn_args(&s, "hsla").or_else(|| fn_args(&s, "hsl"));
    if let Some(args) = hsl {
        if !(3..=4).contains(&args.len()) {
            return None;
        }
        let h = plain_number(args[0].strip_suffix("deg").unwrap_or(args[0]))?.rem_euclid(360.0);
        let sat = percent(args[1])? / 100.0;
        let light = percent(args[2])? / 100.0;
        let a = match args.get(3) {
            Some(a) => alpha(a)?,
            None => 1.0,
        };
        let c = (1.0 - (2.0 * light - 1.0).abs()) * sat;
        let hp = h / 60.0;
        let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
        let (r1, g1, b1) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = light - c / 2.0;
        return Some(Rgba { r: (r1 + m) * 255.0, g: (g1 + m) * 255.0, b: (b1 + m) * 255.0, a });
    }
    None
}

fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// Splits on commas that are not inside parentheses.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

const GRADIENT_FNS: &[&str] = &[
    "linear-gradient(",
    "radial-gradient(",
    "conic-gradient(",
    "repeating-linear-gradient(",
    "repeating-radial-gradient(",
];

fn is_length(s: &str) -> bool {
    let s = s.trim();
    if s == "0" || is_var_ref(s) {
        return true;
    }
    ["px", "rem", "em", "%", "vh", "vw"].iter().any(|u| s.strip_suffix(u).and_then(plain_number).is_some())
}

fn is_font_family(s: &str) -> bool {
    if is_var_ref(s) {
        return true;
    }
    let ok_char = |c: char| c.is_ascii_alphanumeric() || " -_.".contains(c);
    s.split(',').all(|part| {
        let p = part.trim();
        if p.is_empty() {
            return false;
        }
        let quoted = (p.starts_with('\'') && p.ends_with('\'')) || (p.starts_with('"') && p.ends_with('"'));
        if quoted {
            p.len() >= 3 && p[1..p.len() - 1].chars().all(ok_char)
        } else {
            p.chars().all(ok_char)
        }
    })
}

fn is_duration(s: &str) -> bool {
    let s = s.trim();
    if is_var_ref(s) {
        return true;
    }
    let ms = if let Some(v) = s.strip_suffix("ms") {
        plain_number(v)
    } else {
        s.strip_suffix('s').and_then(plain_number).map(|v| v * 1000.0)
    };
    ms.is_some_and(|v| (0.0..=10_000.0).contains(&v))
}

fn is_easing(s: &str) -> bool {
    let s = s.trim();
    if is_var_ref(s) || ["linear", "ease", "ease-in", "ease-out", "ease-in-out"].contains(&s) {
        return true;
    }
    let Some(args) = fn_args(s, "cubic-bezier") else { return false };
    let nums: Option<Vec<f64>> = args.iter().map(|a| plain_number(a)).collect();
    matches!(nums.as_deref(), Some([x1, _, x2, _]) if (0.0..=1.0).contains(x1) && (0.0..=1.0).contains(x2))
}

/// Whether `value` matches the grammar of `kind` (the generic safety rules of
/// `ui_config::is_valid_token_value` are checked separately).
#[must_use]
pub fn matches_kind(kind: TokenKind, value: &str) -> bool {
    let v = value.trim();
    match kind {
        TokenKind::Color => parse_color(v).is_some() || is_var_ref(v),
        TokenKind::Gradient => {
            balanced(v)
                && split_top_level(v).iter().all(|layer| {
                    let l = layer.trim().to_ascii_lowercase();
                    parse_color(&l).is_some()
                        || is_var_ref(&l)
                        || (GRADIENT_FNS.iter().any(|f| l.starts_with(f)) && l.ends_with(')'))
                })
        }
        TokenKind::Shadow => balanced(v),
        TokenKind::Length => is_length(v),
        TokenKind::FontFamily => is_font_family(v),
        TokenKind::FontWeight => {
            is_var_ref(v) || plain_number(v).is_some_and(|w| w.fract() == 0.0 && (100.0..=900.0).contains(&w))
        }
        TokenKind::Number => is_var_ref(v) || plain_number(v).is_some(),
        TokenKind::Duration => is_duration(v),
        TokenKind::Easing => is_easing(v),
    }
}

// ---------------------------------------------------------------------------
// Contrast (WCAG 2.x)
// ---------------------------------------------------------------------------

fn lin(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance of an opaque color.
#[must_use]
pub fn luminance(c: Rgba) -> f64 {
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// `fg` composited over an opaque `bg`.
#[must_use]
pub fn composite(fg: Rgba, bg: Rgba) -> Rgba {
    let a = fg.a;
    Rgba { r: fg.r * a + bg.r * (1.0 - a), g: fg.g * a + bg.g * (1.0 - a), b: fg.b * a + bg.b * (1.0 - a), a: 1.0 }
}

/// WCAG contrast ratio (1..=21) of `fg` over `bg` (both composited first).
#[must_use]
pub fn contrast_ratio(fg: Rgba, bg: Rgba, canvas: Rgba) -> f64 {
    let bg = composite(bg, canvas);
    let fg = composite(fg, bg);
    let (l1, l2) = (luminance(fg), luminance(bg));
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Defaults of the contrast-relevant tokens per base mode (must equal
/// `packages/ui/src/styles/tokens.css`).
#[must_use]
pub fn base_color(mode: ThemeMode, token: &str) -> Option<&'static str> {
    let table = if mode == ThemeMode::Light { BASE_LIGHT } else { BASE_DARK };
    table.iter().find(|(k, _)| *k == token).map(|(_, v)| *v)
}

/// Dark base values of the contrast-relevant tokens.
pub const BASE_DARK: &[(&str, &str)] = &[
    ("color-bg", "#04050d"),
    ("color-bg-elevated", "#0a0d1c"),
    ("color-text", "#e8ecff"),
    ("color-text-muted", "#9aa3c8"),
    ("color-text-subtle", "#727ba6"),
    ("color-text-inverse", "#040611"),
    ("color-accent", "#22d3ee"),
    ("color-danger", "#fb7185"),
    ("color-success", "#34d399"),
    ("color-warning", "#fbbf24"),
    ("gradient-accent", "linear-gradient(135deg, #22d3ee 0%, #6d8bff 52%, #a78bfa 100%)"),
];

/// Light base values of the contrast-relevant tokens.
pub const BASE_LIGHT: &[(&str, &str)] = &[
    ("color-bg", "#eef1fb"),
    ("color-bg-elevated", "#ffffff"),
    ("color-text", "#121633"),
    ("color-text-muted", "#4b5378"),
    ("color-text-subtle", "#636b90"),
    ("color-text-inverse", "#ffffff"),
    ("color-accent", "#0e7490"),
    ("color-danger", "#be123c"),
    ("color-success", "#047857"),
    ("color-warning", "#a16207"),
    // Light mode inherits the root (dark) accent gradient in tokens.css.
    ("gradient-accent", "linear-gradient(135deg, #22d3ee 0%, #6d8bff 52%, #a78bfa 100%)"),
];

/// One contrast check (`hard` failures make the theme invalid).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContrastCheck {
    /// Stable id (`text-bg`).
    pub id: String,
    /// Foreground token.
    pub fg: String,
    /// Background token.
    pub bg: String,
    /// Ratio rounded to 2 decimals.
    pub ratio: f64,
    /// Required minimum.
    pub min: f64,
    /// Failing makes the theme invalid (ADR-0015 safety invariant).
    pub hard: bool,
    /// `ratio >= min`.
    pub pass: bool,
}

/// `(id, fg, bg, min, hard)`. A gradient token (`gradient-accent`) is checked
/// against every color stop and reports the worst one, because primary
/// buttons paint `color-text-inverse` directly on that gradient.
pub const CONTRAST_RULES: &[(&str, &str, &str, f64, bool)] = &[
    ("text-bg", "color-text", "color-bg", 4.5, true),
    ("text-elevated", "color-text", "color-bg-elevated", 4.5, false),
    ("muted-bg", "color-text-muted", "color-bg", 4.5, false),
    ("subtle-bg", "color-text-subtle", "color-bg", 4.5, false),
    ("accent-fg", "color-text-inverse", "color-accent", 4.5, false),
    ("accent-gradient-fg", "color-text-inverse", "gradient-accent", 4.5, false),
    ("accent-bg", "color-accent", "color-bg", 3.0, false),
    ("danger-bg", "color-danger", "color-bg", 3.0, true),
    ("success-bg", "color-success", "color-bg", 3.0, false),
    ("warning-bg", "color-warning", "color-bg", 3.0, false),
];

/// The color stops of a gradient accepted by the `Gradient` grammar
/// (`GRADIENT_FNS`), or a single color for a plain color value. Parts that
/// are not colors (angles, `to right`, `circle at …`) are skipped. `None` when
/// the value is neither, or has no literal color stop (e.g. `var(...)` stops).
#[must_use]
pub fn color_stops(input: &str) -> Option<Vec<Rgba>> {
    let s = input.trim().to_ascii_lowercase();
    if let Some(c) = parse_color(&s) {
        return Some(vec![c]);
    }
    let inner = GRADIENT_FNS.iter().find_map(|f| s.strip_prefix(f))?.strip_suffix(')')?;
    let mut stops = Vec::new();
    for part in split_top_level(inner) {
        let part = part.trim();
        // A color function keeps its own spaces: take it up to its `)`.
        let color = match part.find('(') {
            Some(open) if !part[..open].contains(char::is_whitespace) => part.find(')').map(|close| &part[..=close]),
            _ => part.split_whitespace().next(),
        };
        if let Some(c) = color.and_then(parse_color) {
            stops.push(c);
        } else if color.is_some_and(|c| c.starts_with("var(")) {
            // A stop we cannot resolve: the whole gradient is unknowable.
            return None;
        }
    }
    (!stops.is_empty()).then_some(stops)
}

fn resolve_colors(theme: &ThemeConfig, token: &str) -> Option<Vec<Rgba>> {
    match theme.tokens.get(token) {
        Some(v) => color_stops(v),
        None => base_color(theme.mode, token).and_then(color_stops),
    }
}

/// Runs every contrast rule whose colors are resolvable (tokens set to
/// `var(...)` or other non-literal values are skipped).
#[must_use]
pub fn contrast_report(theme: &ThemeConfig) -> Vec<ContrastCheck> {
    let canvas = if theme.mode == ThemeMode::Light {
        Rgba { r: 255.0, g: 255.0, b: 255.0, a: 1.0 }
    } else {
        Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }
    };
    CONTRAST_RULES
        .iter()
        .filter_map(|(id, fg, bg, min, hard)| {
            let fgs = resolve_colors(theme, fg)?;
            let bgs = resolve_colors(theme, bg)?;
            // The worst pair decides (a text color is a single stop).
            let ratio = fgs
                .iter()
                .flat_map(|f| bgs.iter().map(move |b| contrast_ratio(*f, *b, canvas)))
                .fold(f64::INFINITY, f64::min);
            let ratio = (ratio * 100.0).round() / 100.0;
            Some(ContrastCheck {
                id: (*id).into(),
                fg: (*fg).into(),
                bg: (*bg).into(),
                ratio,
                min: *min,
                hard: *hard,
                pass: ratio >= *min,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Section validation (called by `UiConfig::validate`)
// ---------------------------------------------------------------------------

fn range(issues: &mut Vec<UiConfigIssue>, path: &str, v: f32, min: f32, max: f32) {
    if !(v.is_finite() && v >= min && v <= max) {
        issues.push(UiConfigIssue { path: path.into(), message: format!("must be between {min} and {max}") });
    }
}

fn push(issues: &mut Vec<UiConfigIssue>, path: impl Into<String>, message: impl Into<String>) {
    issues.push(UiConfigIssue { path: path.into(), message: message.into() });
}

/// Sound event keys: `^[a-z][a-z0-9.-]{0,63}$`.
#[must_use]
pub fn is_valid_event_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && key.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

/// Validates the typed parts of a theme (token grammars, sections, contrast).
pub fn validate_theme_details(t: &ThemeConfig, issues: &mut Vec<UiConfigIssue>) {
    for (k, v) in &t.tokens {
        if let Some(kind) = token_kind(k) {
            // Unsafe values are already reported by the generic rule.
            if crate::ui_config::is_valid_token_value(v) && !matches_kind(kind, v) {
                push(issues, format!("theme.tokens.{k}"), format!("not a valid {kind:?} value"));
            }
        }
    }
    if t.description.chars().count() > 280 {
        push(issues, "theme.description", "at most 280 characters");
    }
    if t.author.chars().count() > 80 {
        push(issues, "theme.author", "at most 80 characters");
    }
    let bg = &t.background;
    if bg.colors.len() > MAX_BACKGROUND_COLORS {
        push(issues, "theme.background.colors", format!("at most {MAX_BACKGROUND_COLORS} colors"));
    }
    for (i, c) in bg.colors.iter().enumerate() {
        if parse_color(c).is_none() {
            push(issues, format!("theme.background.colors.{i}"), "not a valid color");
        }
    }
    range(issues, "theme.background.intensity", bg.intensity, 0.0, 1.0);
    range(issues, "theme.background.speed", bg.speed, 0.0, 2.0);
    range(issues, "theme.background.density", bg.density, 0.0, 1.0);
    range(issues, "theme.motion.intensity", t.motion.intensity, 0.0, 1.0);
    range(issues, "theme.motion.speed", t.motion.speed, 0.5, 2.0);
    range(issues, "theme.sounds.volume", t.sounds.volume, 0.0, 1.0);
    if t.sounds.events.len() > MAX_SOUND_EVENTS {
        push(issues, "theme.sounds.events", format!("at most {MAX_SOUND_EVENTS} entries"));
    }
    for (k, v) in &t.sounds.events {
        if !is_valid_event_key(k) || !is_valid_token_name(v) {
            push(issues, format!("theme.sounds.events.{k}"), "invalid event key or preset id");
        }
    }
    if !is_valid_token_name(&t.robot.skin) {
        push(issues, "theme.robot.skin", "must match ^[a-z][a-z0-9-]{0,63}$");
    }
    range(issues, "theme.robot.scale", t.robot.scale, 0.5, 2.0);
    if let Some(a) = &t.robot.accent {
        if parse_color(a).is_none() {
            push(issues, "theme.robot.accent", "not a valid color");
        }
    }
    for c in contrast_report(t) {
        if c.hard && !c.pass {
            push(issues, format!("theme.contrast.{}", c.id), format!("contrast {:.2}:1 is below {}:1", c.ratio, c.min));
        }
    }
}

// ---------------------------------------------------------------------------
// Exchange file + built-ins
// ---------------------------------------------------------------------------

/// Exported theme file (`*.orkestr-theme.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeFile {
    /// Always [`THEME_FILE_FORMAT`].
    pub format: String,
    /// 1 or 2 (v1 files load with section defaults).
    pub theme_schema_version: u32,
    /// The theme.
    pub theme: ThemeConfig,
}

impl ThemeFile {
    /// Wraps a theme for export.
    #[must_use]
    pub fn new(theme: ThemeConfig) -> Self {
        Self { format: THEME_FILE_FORMAT.into(), theme_schema_version: THEME_SCHEMA_VERSION, theme }
    }

    /// Parses and validates an exported theme.
    pub fn parse(json: &str) -> Result<ThemeConfig, Vec<UiConfigIssue>> {
        let file: ThemeFile = serde_json::from_str(json)
            .map_err(|e| vec![UiConfigIssue { path: String::new(), message: format!("not a theme file: {e}") }])?;
        if file.format != THEME_FILE_FORMAT {
            return Err(vec![UiConfigIssue { path: "format".into(), message: "not an Orkestr theme".into() }]);
        }
        if file.theme_schema_version == 0 || file.theme_schema_version > THEME_SCHEMA_VERSION {
            return Err(vec![UiConfigIssue {
                path: "themeSchemaVersion".into(),
                message: format!("unsupported theme schema version {}", file.theme_schema_version),
            }]);
        }
        file.theme.validate().map(|()| file.theme)
    }
}

const BUILTIN_SOURCES: &[&str] = &[
    include_str!("themes/nebula.json"),
    include_str!("themes/midnight-pro.json"),
    include_str!("themes/aurora.json"),
    include_str!("themes/carbon.json"),
    include_str!("themes/solar-light.json"),
    include_str!("themes/high-contrast.json"),
];

/// Built-in themes in display order (Nebula first = default).
///
/// # Panics
/// Never at runtime: the JSON sources are compiled in and covered by tests.
#[must_use]
pub fn builtin_themes() -> Vec<ThemeConfig> {
    BUILTIN_SOURCES.iter().map(|s| serde_json::from_str(s).expect("built-in theme JSON is valid")).collect()
}

/// Whether `id` is a built-in theme id (reserved; read-only).
#[must_use]
pub fn is_builtin_theme(id: &str) -> bool {
    builtin_themes().iter().any(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_valid_and_unique() {
        let all = builtin_themes();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].id, "nebula");
        let mut ids: Vec<_> = all.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6);
        for t in &all {
            t.validate().unwrap_or_else(|e| panic!("{}: {e:?}", t.id));
            for c in contrast_report(t) {
                assert!(c.pass, "{} fails {} ({})", t.id, c.id, c.ratio);
            }
        }
        let hc = all.iter().find(|t| t.id == "high-contrast").unwrap();
        assert!(contrast_report(hc).iter().all(|c| c.ratio >= 7.0), "high contrast is AAA");
    }

    #[test]
    fn color_parser() {
        let c = parse_color("#fff").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (255.0, 255.0, 255.0, 1.0));
        let c = parse_color("rgba(10, 20, 30, 0.5)").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (10.0, 20.0, 30.0, 0.5));
        let c = parse_color("rgb(10 20 30 / 50%)").unwrap();
        assert!((c.a - 0.5).abs() < 1e-9);
        let c = parse_color("hsl(120, 100%, 50%)").unwrap();
        assert!((c.g - 255.0).abs() < 1e-9 && c.r.abs() < 1e-9);
        for bad in ["#ggg", "rgb(1,2)", "rgb(300,0,0)", "hsl(1,2,3)", "red", "1e3", "rgb(1e2,0,0)"] {
            assert!(parse_color(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn gradient_color_stops() {
        let hex = |s: &str| color_stops(s).map(|v| v.iter().map(|c| (c.r, c.g, c.b)).collect::<Vec<_>>());
        assert_eq!(hex("#ffffff"), Some(vec![(255.0, 255.0, 255.0)]));
        assert_eq!(
            hex("linear-gradient(135deg, #000 0%, rgba(255, 0, 0, 0.5) 50%, hsl(0 0% 100%) 100%)"),
            Some(vec![(0.0, 0.0, 0.0), (255.0, 0.0, 0.0), (255.0, 255.0, 255.0)])
        );
        assert_eq!(hex("repeating-radial-gradient(circle at 10% 20%, #fff, #000 40px)").map(|v| v.len()), Some(2));
        assert_eq!(hex("linear-gradient(to right, var(--ok-color-accent), #fff)"), None);
        assert_eq!(hex("url(x.png)"), None);
        assert_eq!(hex("linear-gradient(135deg)"), None);
    }

    /// Regression: primary buttons paint `color-text-inverse` on the accent
    /// gradient, so every stop must be checked (Solar Light used to pass with
    /// white text at 3.19:1 on its amber stop).
    #[test]
    fn accent_gradient_rule_reports_worst_stop() {
        let mut t = builtin_themes().into_iter().find(|t| t.id == "solar-light").unwrap();
        t.tokens.insert(
            "gradient-accent".into(),
            "linear-gradient(135deg, #d97706 0%, #ea580c 50%, #db2777 100%)".into(),
        );
        t.tokens.insert("color-text-subtle".into(), "#8a7c66".into());
        let report = contrast_report(&t);
        let get = |id: &str| report.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no {id}"));
        assert!((get("accent-gradient-fg").ratio - 3.19).abs() < 0.011);
        assert!(!get("accent-gradient-fg").pass);
        assert!(!get("subtle-bg").pass);
        assert!(get("accent-fg").pass, "the flat accent alone hid the failure");
        // Soft rules: the theme stays valid but no longer claims AA.
        assert!(t.validate().is_ok());
    }

    #[test]
    fn contrast_known_values() {
        let black = parse_color("#000").unwrap();
        let white = parse_color("#fff").unwrap();
        assert!((contrast_ratio(white, black, black) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(black, black, black) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn theme_file_roundtrip_and_v1() {
        let nebula = builtin_themes().remove(0);
        let json = serde_json::to_string(&ThemeFile::new(nebula.clone())).unwrap();
        assert_eq!(ThemeFile::parse(&json).unwrap(), nebula);
        let v1 = r#"{"format":"orkestr-theme","themeSchemaVersion":1,"theme":{"id":"old","name":"Old","mode":"Dark","tokens":{},"effects":{"blurPx":10,"glowIntensity":0.5,"panelOpacity":0.8,"animations":true}}}"#;
        let t = ThemeFile::parse(v1).unwrap();
        assert_eq!(t.background, ThemeBackground::default());
        assert!(ThemeFile::parse(&v1.replace("orkestr-theme", "x")).is_err());
        assert!(ThemeFile::parse(&v1.replace("\"themeSchemaVersion\":1", "\"themeSchemaVersion\":9")).is_err());
    }

    /// Shared fixtures — `packages/ui` runs the same file.
    #[test]
    fn shared_fixtures() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fixtures {
            colors: Vec<(String, Option<[f64; 4]>)>,
            kinds: Vec<(TokenKind, String, bool)>,
            themes: Vec<ThemeCase>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ThemeCase {
            name: String,
            patch: serde_json::Value,
            issues: Vec<String>,
            #[serde(default)]
            contrast: BTreeMap<String, f64>,
        }
        let f: Fixtures = serde_json::from_str(include_str!("../fixtures/ui_theme/cases.json")).unwrap();
        for (input, expected) in f.colors {
            let got = parse_color(&input).map(|c| [c.r, c.g, c.b, c.a]);
            match (got, expected) {
                (None, None) => {}
                (Some(g), Some(e)) => {
                    for i in 0..4 {
                        assert!((g[i] - e[i]).abs() < 0.01, "{input}: {g:?} vs {e:?}");
                    }
                }
                (g, e) => panic!("{input}: got {g:?}, expected {e:?}"),
            }
        }
        for (kind, value, ok) in f.kinds {
            assert_eq!(matches_kind(kind, &value), ok, "{kind:?} {value}");
        }
        let base = serde_json::to_value(&builtin_themes()[0]).unwrap();
        for case in f.themes {
            let mut doc = base.clone();
            merge(&mut doc, &case.patch);
            let theme: ThemeConfig = serde_json::from_value(doc).unwrap_or_else(|e| panic!("{}: {e}", case.name));
            let mut got: Vec<String> = match theme.validate() {
                Ok(()) => vec![],
                Err(issues) => issues.into_iter().map(|i| i.path).collect(),
            };
            got.sort();
            let mut want = case.issues.clone();
            want.sort();
            assert_eq!(got, want, "{}", case.name);
            let report = contrast_report(&theme);
            for (id, ratio) in case.contrast {
                let c = report.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{}: no {id}", case.name));
                assert!((c.ratio - ratio).abs() < 0.011, "{} {id}: {} vs {ratio}", case.name, c.ratio);
            }
        }
    }

    fn merge(dst: &mut serde_json::Value, patch: &serde_json::Value) {
        match (dst, patch) {
            (serde_json::Value::Object(d), serde_json::Value::Object(p)) => {
                for (k, v) in p {
                    merge(d.entry(k.clone()).or_insert(serde_json::Value::Null), v);
                }
            }
            (d, p) => *d = p.clone(),
        }
    }
}
