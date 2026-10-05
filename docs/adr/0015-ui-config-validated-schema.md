# ADR-0015: UI configuration as a validated schema (AI-editable UI)

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (schema, tokens, presets, versioning), 3 (theme editor, layout editor, AI edits, robots/sounds sections)
- **Spec:** §28, §29 ("AI-editable UI: the Manager changes a safe UI configuration schema, not arbitrary executable UI code"), §31, §33 (Sound Engine), §34, §64 ("hardcoded UI" forbidden), §67 ("UI theme/layout change without rewriting Core"), §70
- **Related:** ADR-0013 · **Risk:** R-20

## Context

Everything visual must be configurable: colors, gradients, typography, spacing, radius, opacity, blur, glow, shadows, animations, backgrounds, sounds and layouts (§29). The Manager (an AI) may change the UI on request ("make it darker and move the terminal to the bottom"), but must not generate executable code (§29). Config changes must be safe and reversible, and must never lock the user out of the UI: unreadable contrast, hidden emergency controls (§70).

## Decision drivers

1. Data, not code: no `eval`, no arbitrary CSS strings, no HTML or JS.
2. Strict validation in **Rust** (the core is the authority) with the same schema in TS for editor UX.
3. Small, previewable, undoable AI edits.
4. Safety invariants: emergency stop and the approvals indicator are always visible; minimum contrast.
5. The same config renders on Web (Phase 4); mobile uses the token subset.

## Considered options

| Option | Verdict |
|---|---|
| **Typed `UiConfig` (Rust, serde `deny_unknown_fields`) + JSON Schema export (schemars) + RFC 6902 JSON Patch edits with path allowlist** | **Chosen** |
| Free CSS / CSS-in-JS strings from the AI | Rejected. Code injection, `url()` exfiltration, and the UI can be broken by hiding controls |
| AI-generated React components | Forbidden (§29) |
| Tailwind class lists | Rejected. Unbounded and hard to validate semantically |

## Decision

### Schema (`orkestr-protocol::ui_config`, contract §8)

```rust
#[derive(Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiConfig {
    pub schema_version: u32,                       // 1
    pub theme: Theme,
    pub layouts: BTreeMap<String, LayoutPreset>,   // "coding" | "missionControl" | "monitoring" | "focus" | custom ids [a-z0-9-]{1,32}
    pub active_layout: String,
    pub performance_mode: PerformanceMode,         // UltraVisual | Balanced | Performance | BatterySaver (§31)
    pub robots: RobotsConfig,                      // Phase 3: per-agent skin, scale, behavior refs (assets by id)
    pub sounds: SoundsConfig,                      // Phase 3: event → asset id + volume 0..1, DND windows
    pub hud: HudConfig,                            // Phase 3
}

#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Theme {
    pub id: String, pub name: String, pub base: ThemeBase,        // Dark | Light
    pub colors: BTreeMap<ColorToken, Color>,                       // ColorToken enum: bg, bgElevated, surface, border, text, textMuted, accent, accentFg, success, warning, danger, info, glow, …
    pub gradients: BTreeMap<GradientToken, Gradient>,              // Gradient { angle_deg: 0..360, stops: Vec<(Color, f32 0..1)> max 5 }
    pub typography: Typography,                                    // font family from an allowlist of bundled fonts + system stacks; size 10..24 px; weight 300..800
    pub spacing_scale: f32,                                        // 0.75..1.5
    pub radius: Length,                                            // 0..24 px
    pub opacity: BTreeMap<OpacityToken, f32>,                      // 0.3..1 (panels never below 0.3)
    pub blur: Length,                                              // 0..40 px
    pub glow: GlowSpec,                                            // intensity 0..1, color token ref
    pub shadows: ShadowSpec,
    pub motion: MotionSpec,                                        // Full | Reduced | Off; durations 0..1000 ms
    pub background: Background,                                    // Solid(ColorToken) | Gradient(GradientToken) | Image{asset_id: ObjectId, dim: 0..1}
}
pub struct Color(String); // validated: #rgb | #rrggbb | #rrggbbaa | rgb()/rgba()/hsl()/hsla() numeric only
```

The TS side gets the types through ts-rs. A JSON Schema exported with `schemars` drives editor forms and pre-validation in the UI. **The core is the only validator that is trusted.**

### Rendering

`@orkestr/ui`'s theme engine maps tokens to CSS variables (`--ok-color-bg`, `--ok-radius`, `--ok-blur`, …) on `:root`. Panels use only variables (contract §8). Performance Mode clamps the effects:
- `Performance` turns blur and glow off and motion to Reduced;
- `BatterySaver` also turns Rive/Three animations off and lowers the robot frame rate to 10 fps.
Motion also respects `prefers-reduced-motion`.

### Editing and versioning

- Methods:
  - `ui.getConfig` returns `{config, version}`.
  - `ui.applyPatch {patch: JsonPatchOp[], baseVersion, dryRun}` returns `{config, version, warnings}` or `bad_request` with the failing JSON pointer.
  - `ui.history` and `ui.revert {version}`.
- Every change is stored as a new row in `ui_config_versions(version, config_json, author: user|manager|agent, summary, created_at)`. It emits `UiConfigChanged` (CA-6) and is fully revertible (RR-A4).
- **Patch path allowlist for AI actors:** `/theme/**`, `/layouts/**`, `/activeLayout`, `/performanceMode`, `/robots/**`, `/sounds/**`, `/hud/**`. `/schemaVersion` is never allowed.
- **AI edit flow:**
  1. The Manager proposes a patch.
  2. The core validates it with a dry run.
  3. The UI shows a live preview (applied locally, not persisted) with **Keep / Revert**.
  4. If nothing is chosen within 15 s, the change auto-reverts.
  5. Persisting needs a user click, unless the user enabled "Let Manager change UI without confirmation".
- **Safety invariants** (validated after each patch; a failure rejects it):
  - WCAG contrast ≥ 4.5:1 for `text`/`bg` and `accentFg`/`accent`, and ≥ 3:1 for `danger` on `bg`.
  - Every layout must include the shell-level Emergency Stop and Approvals indicator. These live in the title bar and are not dockable, so a layout cannot remove them. A layout must also contain at least one visible panel.
  - Asset references (`ObjectId`) must exist in the object store and be of an allowed MIME type (png, jpg, webp, svg sanitized with no script, riv, json-lottie, ogg, wav).
- **Safe mode:** start the desktop with `--safe-ui`, or hold Shift during startup, to load the built-in default theme and layout regardless of the stored config.

## Consequences

- **Positive:** the UI is themeable and rearrangeable by users and by the Manager without code execution; every change can be reverted; Web reuses it.
- **Negative:** designers cannot use arbitrary CSS. New visual capabilities need a schema extension (a minor `schemaVersion` bump with a migration in Rust). This is intentional.
- **Negative:** the schema must be maintained in Rust. Generated TS and JSON Schema keep the UI side in sync (ADR-0006).

## Implementation notes (Phase 0 foundation, 2026-10-02)

A first, smaller schema is implemented and enforced on both sides; it is the v1 of the schema above, not a replacement of the decision.

- **As built** (`orkestr_protocol::ui_config`, `UI_CONFIG_SCHEMA_VERSION = 1`): `UiConfig { schemaVersion, theme: ThemeConfig, layout: LayoutConfig, performanceMode }`; `ThemeConfig { id, name, mode: Dark | Light, tokens: Map<name, value>, effects: { blurPx 0..64, glowIntensity 0..1, panelOpacity 0.2..1, animations } }`; `LayoutConfig { preset: Coding | MissionControl | Monitoring | Focus | Custom, docking?: JSON ≤ 256 KiB }`; `PerformanceMode { UltraVisual, Balanced, Performance, BatterySaver }`.
- Tokens are free-form names `^[a-z][a-z0-9-]{0,63}$` (≤ 256) with values restricted to a safe character set and no `url(`, `expression(`, `javascript:`, `@import`, `image-set(`, `attr(` — instead of the typed `ColorToken`/`Color` grammar above. `UiConfig::validate()` (Rust) and `validateUiConfig`/`validateTheme` (`@orkestr/ui`) implement the same rules; the engine sets `--ok-<token>`, `--ok-blur`, `--ok-glow-intensity`, `--ok-panel-opacity` on `:root` (`applyTheme`, `applyUiConfig`, `applyPerformanceMode`).
- Not implemented: `deny_unknown_fields`, `schemars` JSON Schema, layouts map / robots / sounds / HUD sections, contrast invariants, `ui.*` RPC methods, `ui_config_versions`, safe mode. Note the panel-opacity floor is 0.2 in code vs 0.3 in the rule above.

## Implementation notes (Phase 3 integration, 2026-10-05)

- **Schema v2** (`UI_CONFIG_SCHEMA_VERSION = 2`, `UI_CONFIG_MIN_SCHEMA_VERSION = 1`): theme sections (description, author, background, motion, sounds, robot defaults — theme engine) and the optional shell `prefs` section (density, panel placement, hidden widgets, background dim, HUD — AI-editable UI). Both are additive with defaults, so v1 documents still validate and are upgraded on read (`UiConfig::upgrade`). Rust `UiConfig::validate` / `UiPrefs::validate_into` and TS `validateUiConfig` / `validatePrefs` (`@orkestr/ui`) apply the same rules.
- **One store**: `core_ui_config` (global + per-device rows, versioned) serves `ui.getConfig` / `ui.setConfig` (theme engine) and `ui.previewPatch` / `ui.applyPatch` (AI-editable UI) under one write lock; patches target the global document and publish `UiConfigPatched` plus `UiConfigChanged`, so the theme engine re-applies patched tokens. The separate `ui_config_versions` table of the HUD branch was dropped before release.
- **Layout presets**: an applied patch that changes `layout.preset` switches the device's layout (layout area, `ui.layouts*`); the raw docking JSON stays non-editable by AI actors.
- **Per-layout widget state**: `LayoutDocument.widgets` (≤ 16 entries, ≤ 16 KiB each, keyed by panel id) stores panel view state with the device's layout — Mission Control's widget arrangement lives under `missionControl`.
- **Theme sounds / robots**: theme `sounds.events` keys and preset ids are the Sound Engine's catalogue (`features/sound/recipes.ts`); theme `robot.skin` uses the robot skin palette ids and styles robots the user has not styled.
