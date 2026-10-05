/**
 * Theme / UI-config validation — mirrors `UiConfig::validate` and
 * `ThemeConfig::validate` (Rust) rule for rule, so a document accepted by
 * the core is accepted here and vice versa. The core remains the only
 * trusted validator; this gives instant feedback in the editor.
 */
import type {
  PerformanceMode,
  ThemeBackground,
  ThemeConfig,
  ThemeEffects,
  ThemeFile,
  ThemeMotion,
  ThemeRobotDefaults,
  ThemeSounds,
  UiConfig,
  UiConfigIssue,
  UiPrefs,
} from "@orkestr/protocol";
import {
  MAX_BACKGROUND_COLORS,
  MAX_SOUND_EVENTS,
  MAX_TOKENS,
  THEME_FILE_FORMAT,
  THEME_SCHEMA_VERSION,
  UI_CONFIG_MIN_SCHEMA_VERSION,
  UI_CONFIG_SCHEMA_VERSION,
} from "@orkestr/protocol";

import { contrastReport } from "./contrast";
import { isValidTokenName, isValidTokenValue, matchesKind, parseColor, tokenKind } from "./grammar";

export const DEFAULT_EFFECTS: Readonly<ThemeEffects> = { blurPx: 18, glowIntensity: 0.6, panelOpacity: 0.72, animations: true };
export const DEFAULT_BACKGROUND: Readonly<ThemeBackground> = {
  kind: "Nebula",
  colors: [],
  intensity: 0.6,
  speed: 0.35,
  density: 0.5,
  webgl: true,
};
export const DEFAULT_MOTION: Readonly<ThemeMotion> = { intensity: 1, speed: 1 };
export const DEFAULT_SOUNDS: Readonly<ThemeSounds> = { enabled: true, volume: 0.6, events: {} };
export const DEFAULT_ROBOT: Readonly<ThemeRobotDefaults> = { skin: "nebula", scale: 1, accent: null };

const PERFORMANCE_MODES: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];
const BACKGROUND_KINDS: readonly ThemeBackground["kind"][] = ["Solid", "Gradient", "Nebula", "Grid", "Particles"];

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);

function section<T extends object>(value: unknown, defaults: Readonly<T>): T {
  if (value === undefined || value === null) return structuredClone(defaults) as T;
  if (!isObj(value)) return value as T; // wrong type: left for validation to report
  return { ...structuredClone(defaults), ...value } as T;
}

/**
 * Fills sections and fields that are absent with their defaults (the
 * equivalent of `#[serde(default)]`), without coercing wrong types.
 */
export function normalizeTheme(input: unknown): ThemeConfig {
  const t = isObj(input) ? input : {};
  return {
    ...(t as unknown as ThemeConfig),
    tokens: (t["tokens"] ?? {}) as ThemeConfig["tokens"],
    effects: (t["effects"] ?? structuredClone(DEFAULT_EFFECTS)) as ThemeEffects,
    description: (t["description"] ?? "") as string,
    author: (t["author"] ?? "") as string,
    background: section(t["background"], DEFAULT_BACKGROUND),
    motion: section(t["motion"], DEFAULT_MOTION),
    sounds: section(t["sounds"], DEFAULT_SOUNDS),
    robot: section(t["robot"], DEFAULT_ROBOT),
  };
}

function inRange(v: unknown, min: number, max: number): v is number {
  return typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
}

function isEventKey(key: string): boolean {
  return /^[a-z][a-z0-9.-]{0,63}$/.test(key);
}

const chars = (s: unknown) => (typeof s === "string" ? [...s].length : 0);

/** Validates a theme; returns every issue found (empty = valid). */
export function validateTheme(theme: ThemeConfig): UiConfigIssue[] {
  const issues: UiConfigIssue[] = [];
  const push = (path: string, message: string) => issues.push({ path, message });
  const range = (path: string, v: unknown, min: number, max: number) => {
    if (!inRange(v, min, max)) push(path, `must be between ${min} and ${max}`);
  };
  if (typeof theme.id !== "string" || !isValidTokenName(theme.id)) push("theme.id", "must match ^[a-z][a-z0-9-]{0,63}$");
  const nameLen = typeof theme.name === "string" ? [...theme.name.trim()].length : 0;
  if (nameLen === 0 || chars(theme.name) > 80) push("theme.name", "must be 1..=80 characters");
  if (theme.mode !== "Dark" && theme.mode !== "Light") push("theme.mode", "must be Dark or Light");
  const tokens = isObj(theme.tokens) ? theme.tokens : {};
  const entries = Object.entries(tokens);
  if (entries.length > MAX_TOKENS) push("theme.tokens", `at most ${MAX_TOKENS} tokens`);
  for (const [k, v] of entries) {
    if (!isValidTokenName(k)) push(`theme.tokens.${k}`, "invalid token name");
    if (typeof v !== "string" || !isValidTokenValue(v)) push(`theme.tokens.${k}`, "invalid or unsafe token value");
  }
  const fx = theme.effects as unknown;
  if (!isObj(fx)) {
    push("theme.effects", "required");
  } else {
    range("theme.effects.blurPx", fx["blurPx"], 0, 64);
    range("theme.effects.glowIntensity", fx["glowIntensity"], 0, 1);
    range("theme.effects.panelOpacity", fx["panelOpacity"], 0.2, 1);
    if (typeof fx["animations"] !== "boolean") push("theme.effects.animations", "must be a boolean");
  }
  // --- typed details (ui_theme::validate_theme_details) ---
  for (const [k, v] of entries) {
    const kind = tokenKind(k);
    if (kind && typeof v === "string" && isValidTokenValue(v) && !matchesKind(kind, v)) {
      push(`theme.tokens.${k}`, `not a valid ${kind} value`);
    }
  }
  if (chars(theme.description) > 280) push("theme.description", "at most 280 characters");
  if (chars(theme.author) > 80) push("theme.author", "at most 80 characters");
  const bg = theme.background as unknown;
  if (!isObj(bg)) push("theme.background", "required");
  else {
    if (!BACKGROUND_KINDS.includes(bg["kind"] as ThemeBackground["kind"])) push("theme.background.kind", "unknown kind");
    const colors = Array.isArray(bg["colors"]) ? (bg["colors"] as unknown[]) : [];
    if (colors.length > MAX_BACKGROUND_COLORS) push("theme.background.colors", `at most ${MAX_BACKGROUND_COLORS} colors`);
    colors.forEach((c, i) => {
      if (typeof c !== "string" || parseColor(c) === null) push(`theme.background.colors.${i}`, "not a valid color");
    });
    range("theme.background.intensity", bg["intensity"], 0, 1);
    range("theme.background.speed", bg["speed"], 0, 2);
    range("theme.background.density", bg["density"], 0, 1);
  }
  const motion = theme.motion as unknown;
  if (!isObj(motion)) push("theme.motion", "required");
  else {
    range("theme.motion.intensity", motion["intensity"], 0, 1);
    range("theme.motion.speed", motion["speed"], 0.5, 2);
  }
  const sounds = theme.sounds as unknown;
  if (!isObj(sounds)) push("theme.sounds", "required");
  else {
    range("theme.sounds.volume", sounds["volume"], 0, 1);
    const events = isObj(sounds["events"]) ? sounds["events"] : {};
    const list = Object.entries(events);
    if (list.length > MAX_SOUND_EVENTS) push("theme.sounds.events", `at most ${MAX_SOUND_EVENTS} entries`);
    for (const [k, v] of list) {
      if (!isEventKey(k) || typeof v !== "string" || !isValidTokenName(v)) {
        push(`theme.sounds.events.${k}`, "invalid event key or preset id");
      }
    }
  }
  const robot = theme.robot as unknown;
  if (!isObj(robot)) push("theme.robot", "required");
  else {
    if (typeof robot["skin"] !== "string" || !isValidTokenName(robot["skin"])) {
      push("theme.robot.skin", "must match ^[a-z][a-z0-9-]{0,63}$");
    }
    range("theme.robot.scale", robot["scale"], 0.5, 2);
    const accent = robot["accent"];
    if (accent !== null && accent !== undefined && (typeof accent !== "string" || parseColor(accent) === null)) {
      push("theme.robot.accent", "not a valid color");
    }
  }
  if (issues.length === 0 || issues.every((i) => !i.path.startsWith("theme.tokens") && i.path !== "theme.mode")) {
    for (const c of contrastReport(theme)) {
      if (c.hard && !c.pass) push(`theme.contrast.${c.id}`, `contrast ${c.ratio.toFixed(2)}:1 is below ${c.min}:1`);
    }
  }
  return issues;
}

/** Max entries in `UiPrefs.panels` (mirrors `MAX_PANEL_PREFS`). */
export const MAX_PANEL_PREFS = 64;
/** Max entries in `UiPrefs.hiddenWidgets` (mirrors `MAX_HIDDEN_WIDGETS`). */
export const MAX_HIDDEN_WIDGETS = 128;

/** Panel / widget id grammar `^[a-z][A-Za-z0-9.-]{0,63}` + end (mirrors `is_valid_ui_id`). */
export function isValidUiId(id: string): boolean {
  return /^[a-z][A-Za-z0-9.-]{0,63}$/.test(id);
}

/** Validates shell preferences — mirrors `UiPrefs::validate_into` rule for rule. */
export function validatePrefs(prefs: Partial<UiPrefs>): UiConfigIssue[] {
  const issues: UiConfigIssue[] = [];
  const push = (path: string, message: string) => issues.push({ path, message });
  const panels = prefs.panels ?? {};
  const panelIds = Object.keys(panels);
  if (panelIds.length > MAX_PANEL_PREFS) push("prefs.panels", "too many panel entries");
  for (const id of panelIds) if (!isValidUiId(id)) push(`prefs.panels.${id}`, "invalid panel id");
  const hidden = prefs.hiddenWidgets ?? [];
  if (hidden.length > MAX_HIDDEN_WIDGETS) push("prefs.hiddenWidgets", "too many hidden widgets");
  hidden.forEach((id, i) => {
    if (!isValidUiId(id)) push(`prefs.hiddenWidgets.${i}`, "invalid widget id");
  });
  if (!inRange(prefs.backgroundDim ?? 0, 0, 0.9)) push("prefs.backgroundDim", "must be between 0 and 0.9");
  if (!inRange(prefs.hud?.opacity ?? 0.82, 0.4, 1)) push("prefs.hud.opacity", "must be between 0.4 and 1");
  return issues;
}

/** Validates a whole UI config document. */
export function validateUiConfig(input: unknown): { ok: true; config: UiConfig } | { ok: false; issues: UiConfigIssue[] } {
  if (!isObj(input)) return { ok: false, issues: [{ path: "", message: "must be an object" }] };
  const cfg = input as unknown as UiConfig;
  const issues: UiConfigIssue[] = [];
  if (
    typeof cfg.schemaVersion !== "number" ||
    !Number.isInteger(cfg.schemaVersion) ||
    cfg.schemaVersion < UI_CONFIG_MIN_SCHEMA_VERSION ||
    cfg.schemaVersion > UI_CONFIG_SCHEMA_VERSION
  ) {
    issues.push({
      path: "schemaVersion",
      message: `unsupported schema version ${String(cfg.schemaVersion)} (expected ${UI_CONFIG_MIN_SCHEMA_VERSION}..=${UI_CONFIG_SCHEMA_VERSION})`,
    });
  }
  if (!isObj(cfg.theme)) issues.push({ path: "theme", message: "required" });
  else issues.push(...validateTheme(normalizeTheme(cfg.theme)));
  if (!PERFORMANCE_MODES.includes(cfg.performanceMode)) {
    issues.push({ path: "performanceMode", message: "unknown performance mode" });
  }
  if (cfg.layout?.docking != null && JSON.stringify(cfg.layout.docking).length > 256 * 1024) {
    issues.push({ path: "layout.docking", message: "docking layout too large (max 256 KiB)" });
  }
  if (cfg.prefs != null) issues.push(...validatePrefs(cfg.prefs));
  return issues.length === 0 ? { ok: true, config: cfg } : { ok: false, issues };
}

/** Wraps a theme for export (`*.orkestr-theme.json`). */
export function exportThemeFile(theme: ThemeConfig): string {
  const file: ThemeFile = { format: THEME_FILE_FORMAT, themeSchemaVersion: THEME_SCHEMA_VERSION, theme };
  return `${JSON.stringify(file, null, 2)}\n`;
}

/**
 * Parses an exported theme (v1 files load with section defaults) and
 * validates it. Also accepts a bare `ThemeConfig` object for convenience.
 */
export function parseThemeFile(json: string): { ok: true; theme: ThemeConfig } | { ok: false; issues: UiConfigIssue[] } {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch (e) {
    return { ok: false, issues: [{ path: "", message: `not JSON: ${e instanceof Error ? e.message : String(e)}` }] };
  }
  if (!isObj(raw)) return { ok: false, issues: [{ path: "", message: "not a theme file" }] };
  let themeRaw: unknown = raw;
  if ("format" in raw || "themeSchemaVersion" in raw) {
    if (raw["format"] !== THEME_FILE_FORMAT) return { ok: false, issues: [{ path: "format", message: "not an Orkestr theme" }] };
    const v = raw["themeSchemaVersion"];
    if (typeof v !== "number" || !Number.isInteger(v) || v < 1 || v > THEME_SCHEMA_VERSION) {
      return { ok: false, issues: [{ path: "themeSchemaVersion", message: `unsupported theme schema version ${String(v)}` }] };
    }
    themeRaw = raw["theme"];
  }
  if (!isObj(themeRaw)) return { ok: false, issues: [{ path: "theme", message: "required" }] };
  const theme = normalizeTheme(themeRaw);
  const issues = validateTheme(theme);
  return issues.length === 0 ? { ok: true, theme } : { ok: false, issues };
}
