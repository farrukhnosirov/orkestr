/**
 * Theme engine runtime (§29, §31, ADR-0015). Applies a validated
 * `ThemeConfig` — data, never code — as `--ok-*` CSS custom properties and
 * resolves the *effective* visuals for the current Performance Mode and the
 * OS reduced-motion preference (§70: visuals never compromise performance).
 *
 * Effects are written inline on the target, so they win over the stylesheet;
 * that is why the performance clamps are computed here rather than in CSS.
 */
import type { PerformanceMode, ThemeBackground, ThemeConfig, UiConfig } from "@orkestr/protocol";
import { BUILTIN_THEME_LIST, UI_CONFIG_SCHEMA_VERSION } from "@orkestr/protocol";

import { durationMs } from "./grammar";
import { normalizeTheme, validateTheme, validateUiConfig } from "./schema";
import type { UiConfigIssue } from "@orkestr/protocol";

// ---------------------------------------------------------------------------
// Built-ins
// ---------------------------------------------------------------------------

/** Built-in themes in display order (Nebula first = default). Generated from Rust. */
export const BUILTIN_THEME_ORDER: readonly string[] = BUILTIN_THEME_LIST.map((t) => t.id);

/** Built-in themes by id. */
export const BUILTIN_THEMES: Readonly<Record<string, ThemeConfig>> = Object.fromEntries(
  BUILTIN_THEME_LIST.map((t) => [t.id, t]),
);

/** Id of the default theme. */
export const DEFAULT_THEME_ID = "nebula";

/** Whether `id` names a (read-only) built-in theme. */
export function isBuiltinTheme(id: string): boolean {
  return Object.prototype.hasOwnProperty.call(BUILTIN_THEMES, id);
}

/** A deep copy of the default theme. */
export function defaultTheme(): ThemeConfig {
  const t = BUILTIN_THEMES[DEFAULT_THEME_ID] ?? BUILTIN_THEME_LIST[0];
  if (!t) throw new Error("no built-in themes");
  return structuredClone(t);
}

/** The default UI config document. */
export function defaultUiConfig(): UiConfig {
  return {
    schemaVersion: UI_CONFIG_SCHEMA_VERSION,
    theme: defaultTheme(),
    layout: { preset: "Coding", docking: null },
    performanceMode: "Balanced",
  };
}

// ---------------------------------------------------------------------------
// Effective visuals (pure)
// ---------------------------------------------------------------------------

/** What the environment allows. */
export interface VisualPolicy {
  performanceMode: PerformanceMode;
  /** `prefers-reduced-motion: reduce`. */
  reducedMotion: boolean;
}

export const DEFAULT_POLICY: Readonly<VisualPolicy> = { performanceMode: "Balanced", reducedMotion: false };

/** How the background is drawn: plain CSS, a 2D canvas, or WebGL (Ultra Visual only). */
export type BackgroundRenderer = "css" | "canvas" | "webgl";

/** `full` = theme motion, `reduced` = no decorative motion, `off` = no transitions. */
export type MotionLevel = "full" | "reduced" | "off";

export interface EffectiveBackground {
  kind: ThemeBackground["kind"];
  renderer: BackgroundRenderer;
  /** Whether the background animates (false = one static frame). */
  animated: boolean;
  /** Frame-rate cap while animating. */
  fps: number;
  /** Device-pixel scale of the canvas (lower = cheaper). */
  resolution: number;
  colors: string[];
  intensity: number;
  speed: number;
  density: number;
}

export interface EffectiveVisuals {
  blurPx: number;
  glowIntensity: number;
  panelOpacity: number;
  motion: MotionLevel;
  /** Multiplier applied to duration tokens (0 = instant). */
  durationScale: number;
  /** Amplitude of decorative motion (0..1). */
  motionIntensity: number;
  background: EffectiveBackground;
}

/** Duration tokens and their tokens.css defaults (ms). */
export const DURATION_TOKENS: Readonly<Record<string, number>> = {
  "duration-fast": 120,
  "duration-base": 200,
  "duration-slow": 360,
};

/**
 * Resolves what actually renders for a theme under a policy:
 *
 * | Mode         | blur/glow | motion             | background                     |
 * |--------------|-----------|--------------------|--------------------------------|
 * | UltraVisual  | theme     | theme              | WebGL (if allowed) 60 fps      |
 * | Balanced     | theme     | theme              | canvas 30 fps, half resolution |
 * | Performance  | off       | reduced            | canvas, one static frame       |
 * | BatterySaver | off       | off                | CSS only, static               |
 *
 * `prefers-reduced-motion` or `effects.animations = false` freeze the
 * background and remove decorative motion in every mode.
 */
export function effectiveVisuals(theme: ThemeConfig, policy: VisualPolicy = DEFAULT_POLICY): EffectiveVisuals {
  const t = normalizeTheme(theme);
  const mode = policy.performanceMode;
  const lowPower = mode === "Performance" || mode === "BatterySaver";
  const animationsAllowed = t.effects.animations && !policy.reducedMotion;

  let motion: MotionLevel = animationsAllowed ? "full" : "reduced";
  if (mode === "Performance") motion = "reduced";
  if (mode === "BatterySaver") motion = "off";
  if (!t.effects.animations) motion = "off";
  // Reduced motion keeps short state transitions (they aid orientation) but
  // removes decorative motion; "off" removes transitions too.
  const durationScale = motion === "off" ? 0 : motion === "reduced" ? 0.5 : 1 / t.motion.speed;

  const bg = t.background;
  const procedural = bg.kind === "Nebula" || bg.kind === "Grid" || bg.kind === "Particles";
  let renderer: BackgroundRenderer = "css";
  if (procedural && mode !== "BatterySaver") {
    renderer = mode === "UltraVisual" && bg.webgl && bg.kind === "Nebula" ? "webgl" : "canvas";
  }
  const animated = procedural && renderer !== "css" && !lowPower && animationsAllowed && bg.speed > 0;
  return {
    blurPx: lowPower ? 0 : t.effects.blurPx,
    glowIntensity: lowPower ? 0 : t.effects.glowIntensity,
    // Without blur, translucent panels lose legibility over busy backgrounds.
    panelOpacity: lowPower ? Math.max(t.effects.panelOpacity, 0.9) : t.effects.panelOpacity,
    motion,
    durationScale,
    motionIntensity: motion === "full" ? t.motion.intensity : 0,
    background: {
      kind: bg.kind,
      renderer,
      animated,
      fps: animated ? (mode === "UltraVisual" ? 60 : 30) : 0,
      resolution: mode === "UltraVisual" ? 1 : 0.5,
      colors: [...bg.colors],
      intensity: bg.intensity,
      speed: bg.speed,
      density: bg.density,
    },
  };
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/** Thrown by `applyTheme` for invalid themes. */
export class ThemeValidationError extends Error {
  readonly issues: UiConfigIssue[];
  constructor(issues: UiConfigIssue[]) {
    super(`invalid theme: ${issues.map((i) => `${i.path}: ${i.message}`).join("; ")}`);
    this.name = "ThemeValidationError";
    this.issues = issues;
  }
}

/** DOM event dispatched on the target after every apply (Monaco/xterm re-derive their themes). */
export const THEME_APPLIED_EVENT = "ok-theme-applied";

export interface ThemeAppliedDetail {
  themeId: string;
  mode: ThemeConfig["mode"];
  revision: number;
  visuals: EffectiveVisuals;
}

const applied = new WeakMap<HTMLElement, string[]>();
let revision = 0;

/** Increments on every successful apply. */
export function themeRevision(): number {
  return revision;
}

function fmt(n: number): string {
  return String(Math.round(n * 1000) / 1000);
}

/**
 * Applies a theme to `target` (default `document.documentElement`) under a
 * policy and returns the effective visuals. Previously applied overrides are
 * removed first, so switching themes never leaks tokens. Throws
 * `ThemeValidationError` (without touching the element) if the theme is
 * invalid.
 */
export function applyTheme(
  input: ThemeConfig,
  target: HTMLElement = document.documentElement,
  policy: VisualPolicy = DEFAULT_POLICY,
): EffectiveVisuals {
  const theme = normalizeTheme(input);
  const issues = validateTheme(theme);
  if (issues.length > 0) throw new ThemeValidationError(issues);
  const visuals = effectiveVisuals(theme, policy);

  for (const prop of applied.get(target) ?? []) target.style.removeProperty(prop);
  const props: string[] = [];
  const set = (prop: string, value: string) => {
    target.style.setProperty(prop, value);
    props.push(prop);
  };
  for (const [k, v] of Object.entries(theme.tokens)) {
    if (k in DURATION_TOKENS) continue;
    set(`--ok-${k}`, v);
  }
  for (const [k, base] of Object.entries(DURATION_TOKENS)) {
    const own = theme.tokens[k];
    const ms = own === undefined ? base : durationMs(own);
    if (ms === null) {
      if (own !== undefined) set(`--ok-${k}`, own);
      continue;
    }
    set(`--ok-${k}`, `${Math.round(ms * visuals.durationScale)}ms`);
  }
  set("--ok-blur", `${fmt(visuals.blurPx)}px`);
  set("--ok-glow-intensity", fmt(visuals.glowIntensity));
  set("--ok-panel-opacity", fmt(visuals.panelOpacity));
  set("--ok-motion-intensity", fmt(visuals.motionIntensity));
  set("--ok-bg-intensity", fmt(visuals.background.intensity));
  applied.set(target, props);

  target.dataset["okTheme"] = theme.id;
  target.dataset["okMode"] = theme.mode === "Light" ? "light" : "dark";
  target.dataset["okAnimations"] = visuals.motion === "full" ? "on" : "off";
  target.dataset["okMotion"] = visuals.motion;
  target.dataset["okPerformance"] = policy.performanceMode;
  target.dataset["okBackground"] = theme.background.kind.toLowerCase();

  revision += 1;
  const detail: ThemeAppliedDetail = { themeId: theme.id, mode: theme.mode, revision, visuals };
  target.dispatchEvent(new CustomEvent<ThemeAppliedDetail>(THEME_APPLIED_EVENT, { detail }));
  return visuals;
}

/** Subscribes to theme applications on `target`; returns an unsubscribe function. */
export function onThemeApplied(
  cb: (detail: ThemeAppliedDetail) => void,
  target: HTMLElement = document.documentElement,
): () => void {
  const listener = (e: Event) => cb((e as CustomEvent<ThemeAppliedDetail>).detail);
  target.addEventListener(THEME_APPLIED_EVENT, listener);
  return () => target.removeEventListener(THEME_APPLIED_EVENT, listener);
}

/** Marks the performance mode on `target` (CSS hooks); prefer `applyTheme(…, policy)`. */
export function applyPerformanceMode(mode: PerformanceMode, target: HTMLElement = document.documentElement): void {
  target.dataset["okPerformance"] = mode;
}

/** Applies a full UI config (theme + performance mode). */
export function applyUiConfig(
  config: UiConfig,
  target: HTMLElement = document.documentElement,
  reducedMotion = false,
): EffectiveVisuals {
  const result = validateUiConfig(config);
  if (!result.ok) throw new ThemeValidationError(result.issues);
  return applyTheme(config.theme, target, { performanceMode: config.performanceMode, reducedMotion });
}
