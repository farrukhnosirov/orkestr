/**
 * Applying the UI configuration's shell preferences (`UiConfig.prefs`) and
 * theme overrides to the DOM — data only, never code (§29).
 *
 * Root contract: `data-ok-density` (`comfortable|compact`) on `<html>`, and
 * `--ok-bg-dim` (0..0.9). The stored theme is applied by the theme engine
 * (`ui.getConfig` → `useThemeEngine`, the same document `ui.applyPatch`
 * writes); `applyThemeOverlay` only layers a proposal's token / effect
 * changes on `<body>` while its live preview is shown.
 */
import type { PanelSide, ThemeEffects, UiConfig, UiPrefs } from "@orkestr/protocol";
import { isValidTokenName, isValidTokenValue } from "@orkestr/ui";

export const DEFAULT_PREFS: UiPrefs = {
  density: "Comfortable",
  panels: {},
  hiddenWidgets: [],
  backgroundDim: 0,
  hud: { opacity: 0.82, showAgents: true, showManagerInput: true },
};

const DEFAULT_EFFECTS: ThemeEffects = { blurPx: 18, glowIntensity: 0.6, panelOpacity: 0.72, animations: true };

/** The preferences of a document, defaults filled in. */
export function prefsOf(config: UiConfig | null | undefined): UiPrefs {
  const p = config?.prefs;
  if (!p) return DEFAULT_PREFS;
  return { ...DEFAULT_PREFS, ...p, hud: { ...DEFAULT_PREFS.hud, ...p.hud } };
}

function clamp(v: number, min: number, max: number): number {
  return Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : min;
}

/** Root attributes / variables for preferences. */
export function prefsRootAttributes(p: UiPrefs): { density: string; vars: Record<string, string> } {
  return {
    density: p.density === "Compact" ? "compact" : "comfortable",
    vars: {
      "--ok-bg-dim": String(clamp(p.backgroundDim, 0, 0.9)),
      "--ok-hud-opacity": String(clamp(p.hud.opacity, 0.4, 1)),
    },
  };
}

export function applyPrefsToRoot(p: UiPrefs, root: HTMLElement = document.documentElement): void {
  const { density, vars } = prefsRootAttributes(p);
  root.dataset["okDensity"] = density;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
}

/**
 * CSS variables for the theme overrides of a document: only valid token
 * names/values, and effects only when they differ from the defaults (so a
 * stored default document never overrides a built-in theme's effects).
 */
export function themeOverlayVars(config: UiConfig | null | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!config) return out;
  for (const [k, v] of Object.entries(config.theme.tokens)) {
    if (typeof v === "string" && isValidTokenName(k) && isValidTokenValue(v)) out[`--ok-${k}`] = v;
  }
  const e = config.theme.effects;
  if (e.blurPx !== DEFAULT_EFFECTS.blurPx) out["--ok-blur"] = `${clamp(e.blurPx, 0, 64)}px`;
  if (e.glowIntensity !== DEFAULT_EFFECTS.glowIntensity) out["--ok-glow-intensity"] = String(clamp(e.glowIntensity, 0, 1));
  if (e.panelOpacity !== DEFAULT_EFFECTS.panelOpacity) out["--ok-panel-opacity"] = String(clamp(e.panelOpacity, 0.2, 1));
  return out;
}

const overlayApplied = new WeakMap<HTMLElement, string[]>();

/** Writes the overlay to `target` (default `<body>`), removing what it set before. */
export function applyThemeOverlay(config: UiConfig | null | undefined, target: HTMLElement = document.body): void {
  for (const prop of overlayApplied.get(target) ?? []) target.style.removeProperty(prop);
  const vars = themeOverlayVars(config);
  for (const [k, v] of Object.entries(vars)) target.style.setProperty(k, v);
  overlayApplied.set(target, Object.keys(vars));
}

/** Whether a widget is hidden by the preferences. */
export function isWidgetHidden(p: UiPrefs, id: string): boolean {
  return p.hiddenWidgets.includes(id);
}

/** Preferred side of a panel (`null` = layout default). */
export function panelSide(p: UiPrefs, id: string): PanelSide | null {
  return p.panels[id]?.side ?? null;
}

/** Whether a panel is hidden by the preferences. */
export function isPanelHidden(p: UiPrefs, id: string): boolean {
  return p.panels[id]?.hidden === true;
}
