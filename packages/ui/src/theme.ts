/**
 * Theme engine (§29). Applies a validated `ThemeConfig` (data, never code) as
 * `--ok-*` CSS custom properties. Validation mirrors
 * `orkestr_protocol::ui_config` in Rust exactly, so a config accepted by the
 * core is accepted here and vice versa.
 */
import type { PerformanceMode, ThemeConfig, UiConfig, UiConfigIssue } from "@orkestr/protocol";
import { UI_CONFIG_SCHEMA_VERSION } from "@orkestr/protocol";

export const MAX_TOKENS = 256;
export const MAX_TOKEN_VALUE_LEN = 256;

const TOKEN_NAME_RE = /^[a-z][a-z0-9-]{0,63}$/;
const ALLOWED_VALUE_CHARS = /^[A-Za-z0-9 #%.,()\-+/'"_!]+$/;
const FORBIDDEN_SUBSTRINGS = ["url(", "expression(", "javascript:", "@import", "image-set(", "attr("];

export function isValidTokenName(name: string): boolean {
  return TOKEN_NAME_RE.test(name);
}

export function isValidTokenValue(value: string): boolean {
  if (value.length === 0 || value.length > MAX_TOKEN_VALUE_LEN) return false;
  if (!ALLOWED_VALUE_CHARS.test(value)) return false;
  const lower = value.toLowerCase();
  return !FORBIDDEN_SUBSTRINGS.some((bad) => lower.includes(bad));
}

function inRange(v: unknown, min: number, max: number): v is number {
  return typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
}

/** Validates a theme; returns every issue found (empty = valid). */
export function validateTheme(theme: ThemeConfig): UiConfigIssue[] {
  const issues: UiConfigIssue[] = [];
  const push = (path: string, message: string) => issues.push({ path, message });
  if (typeof theme.id !== "string" || !isValidTokenName(theme.id)) push("theme.id", "must match ^[a-z][a-z0-9-]{0,63}$");
  const nameLen = typeof theme.name === "string" ? [...theme.name.trim()].length : 0;
  if (nameLen === 0 || nameLen > 80) push("theme.name", "must be 1..=80 characters");
  if (theme.mode !== "Dark" && theme.mode !== "Light") push("theme.mode", "must be Dark or Light");
  const tokens = theme.tokens ?? {};
  const entries = Object.entries(tokens);
  if (entries.length > MAX_TOKENS) push("theme.tokens", `at most ${MAX_TOKENS} tokens`);
  for (const [k, v] of entries) {
    if (!isValidTokenName(k)) push(`theme.tokens.${k}`, "invalid token name");
    if (typeof v !== "string" || !isValidTokenValue(v)) push(`theme.tokens.${k}`, "invalid or unsafe token value");
  }
  const fx = theme.effects;
  if (!fx || typeof fx !== "object") {
    push("theme.effects", "required");
  } else {
    if (!inRange(fx.blurPx, 0, 64)) push("theme.effects.blurPx", "must be between 0 and 64");
    if (!inRange(fx.glowIntensity, 0, 1)) push("theme.effects.glowIntensity", "must be between 0 and 1");
    if (!inRange(fx.panelOpacity, 0.2, 1)) push("theme.effects.panelOpacity", "must be between 0.2 and 1");
    if (typeof fx.animations !== "boolean") push("theme.effects.animations", "must be a boolean");
  }
  return issues;
}

const PERFORMANCE_MODES: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];

/** Validates a whole UI config document. */
export function validateUiConfig(input: unknown): { ok: true; config: UiConfig } | { ok: false; issues: UiConfigIssue[] } {
  if (typeof input !== "object" || input === null) {
    return { ok: false, issues: [{ path: "", message: "must be an object" }] };
  }
  const cfg = input as UiConfig;
  const issues: UiConfigIssue[] = [];
  if (cfg.schemaVersion !== UI_CONFIG_SCHEMA_VERSION) {
    issues.push({
      path: "schemaVersion",
      message: `unsupported schema version ${String(cfg.schemaVersion)} (expected ${UI_CONFIG_SCHEMA_VERSION})`,
    });
  }
  if (!cfg.theme || typeof cfg.theme !== "object") issues.push({ path: "theme", message: "required" });
  else issues.push(...validateTheme(cfg.theme));
  if (!PERFORMANCE_MODES.includes(cfg.performanceMode)) {
    issues.push({ path: "performanceMode", message: "unknown performance mode" });
  }
  if (cfg.layout?.docking != null && JSON.stringify(cfg.layout.docking).length > 256 * 1024) {
    issues.push({ path: "layout.docking", message: "docking layout too large (max 256 KiB)" });
  }
  return issues.length === 0 ? { ok: true, config: cfg } : { ok: false, issues };
}

/** Thrown by `applyTheme` for invalid themes. */
export class ThemeValidationError extends Error {
  readonly issues: UiConfigIssue[];
  constructor(issues: UiConfigIssue[]) {
    super(`invalid theme: ${issues.map((i) => `${i.path}: ${i.message}`).join("; ")}`);
    this.name = "ThemeValidationError";
    this.issues = issues;
  }
}

const applied = new WeakMap<HTMLElement, string[]>();

/**
 * Applies a theme to `target` (default `document.documentElement`).
 * Previously applied token overrides are removed first, so switching themes
 * never leaks tokens. Throws `ThemeValidationError` if the theme is invalid.
 */
export function applyTheme(theme: ThemeConfig, target: HTMLElement = document.documentElement): void {
  const issues = validateTheme(theme);
  if (issues.length > 0) throw new ThemeValidationError(issues);
  for (const prop of applied.get(target) ?? []) target.style.removeProperty(prop);
  const props: string[] = [];
  const set = (prop: string, value: string) => {
    target.style.setProperty(prop, value);
    props.push(prop);
  };
  for (const [k, v] of Object.entries(theme.tokens ?? {})) {
    if (typeof v === "string") set(`--ok-${k}`, v);
  }
  set("--ok-blur", `${theme.effects.blurPx}px`);
  set("--ok-glow-intensity", String(theme.effects.glowIntensity));
  set("--ok-panel-opacity", String(theme.effects.panelOpacity));
  applied.set(target, props);
  target.dataset["okTheme"] = theme.id;
  target.dataset["okMode"] = theme.mode === "Light" ? "light" : "dark";
  target.dataset["okAnimations"] = theme.effects.animations ? "on" : "off";
}

/** Applies a performance mode (§31) — scales effects down via CSS. */
export function applyPerformanceMode(mode: PerformanceMode, target: HTMLElement = document.documentElement): void {
  target.dataset["okPerformance"] = mode;
}

/** Applies a full UI config (theme + performance mode). */
export function applyUiConfig(config: UiConfig, target: HTMLElement = document.documentElement): void {
  const result = validateUiConfig(config);
  if (!result.ok) throw new ThemeValidationError(result.issues);
  applyTheme(config.theme, target);
  applyPerformanceMode(config.performanceMode, target);
}

const DEFAULT_EFFECTS = { blurPx: 18, glowIntensity: 0.6, panelOpacity: 0.72, animations: true } as const;

/** Built-in themes. Nebula is the default (tokens come from tokens.css). */
export const BUILTIN_THEMES: Readonly<Record<string, ThemeConfig>> = {
  nebula: { id: "nebula", name: "Nebula", mode: "Dark", tokens: {}, effects: { ...DEFAULT_EFFECTS } },
  eclipse: {
    id: "eclipse",
    name: "Eclipse",
    mode: "Dark",
    tokens: {
      "color-bg": "#07040f",
      "color-accent": "#f472b6",
      "color-accent-strong": "#ec4899",
      "color-accent-soft": "rgba(244, 114, 182, 0.14)",
      "color-border-accent": "rgba(244, 114, 182, 0.45)",
      "gradient-accent": "linear-gradient(135deg, #f472b6 0%, #a78bfa 55%, #6366f1 100%)",
    },
    effects: { blurPx: 22, glowIntensity: 0.75, panelOpacity: 0.7, animations: true },
  },
  daylight: {
    id: "daylight",
    name: "Daylight",
    mode: "Light",
    tokens: { "color-accent": "#0891b2", "color-violet": "#7c3aed" },
    effects: { blurPx: 14, glowIntensity: 0.2, panelOpacity: 0.85, animations: true },
  },
};

/** The default UI config document. */
export function defaultUiConfig(): UiConfig {
  const nebula = BUILTIN_THEMES["nebula"];
  if (!nebula) throw new Error("nebula theme missing");
  return {
    schemaVersion: UI_CONFIG_SCHEMA_VERSION,
    theme: structuredClone(nebula),
    layout: { preset: "Coding", docking: null },
    performanceMode: "Balanced",
  };
}
