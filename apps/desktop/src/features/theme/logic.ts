/**
 * Pure helpers of the Theme Editor (§29): editor token groups, presets,
 * scale transforms, id allocation, duplication. No React, no I/O — tested in
 * `theme.test.ts`.
 */
import type { ThemeConfig, UiConfigIssue } from "@orkestr/protocol";
import { isBuiltinTheme, isValidTokenName, normalizeTheme, plainNumber, validateTheme } from "@orkestr/ui";

// ---------------------------------------------------------------------------
// Token groups shown by the editor (labels: `theme:tokens.<token>`)
// ---------------------------------------------------------------------------

export interface TokenGroup {
  /** i18n key suffix: `theme:groups.<id>`. */
  id: string;
  tokens: readonly string[];
}

export const COLOR_GROUPS: readonly TokenGroup[] = [
  { id: "backgrounds", tokens: ["color-bg", "color-bg-elevated", "color-overlay"] },
  {
    id: "surfaces",
    tokens: ["color-surface", "color-surface-strong", "color-surface-sunken", "color-surface-hover", "color-surface-active"],
  },
  { id: "borders", tokens: ["color-border", "color-border-strong", "color-border-accent"] },
  { id: "text", tokens: ["color-text", "color-text-muted", "color-text-subtle", "color-text-inverse"] },
  {
    id: "accents",
    tokens: ["color-accent", "color-accent-strong", "color-accent-soft", "color-violet", "color-violet-strong", "color-magenta"],
  },
  {
    id: "statuses",
    tokens: [
      "color-success",
      "color-success-soft",
      "color-warning",
      "color-warning-soft",
      "color-danger",
      "color-danger-soft",
      "color-info",
      "color-info-soft",
    ],
  },
];

export const GRADIENT_TOKENS: readonly string[] = ["gradient-accent", "gradient-accent-soft", "gradient-space"];

/** Every token the editor can label (for i18n parity checks). */
export const EDITOR_TOKENS: readonly string[] = [...COLOR_GROUPS.flatMap((g) => g.tokens), ...GRADIENT_TOKENS];

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

export interface FontPreset {
  /** i18n key suffix: `theme:fonts.<id>`. */
  id: string;
  stack: string;
}

export const SANS_FONTS: readonly FontPreset[] = [
  { id: "inter", stack: "\"Inter\", \"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif" },
  { id: "system", stack: "system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif" },
  { id: "humanist", stack: "\"Segoe UI\", Candara, \"Trebuchet MS\", \"Noto Sans\", sans-serif" },
  { id: "geometric", stack: "\"Montserrat\", \"Century Gothic\", Futura, \"Segoe UI\", sans-serif" },
  { id: "rounded", stack: "\"Nunito\", \"Arial Rounded MT Bold\", \"Segoe UI\", system-ui, sans-serif" },
  { id: "serif", stack: "\"Iowan Old Style\", Georgia, Cambria, \"Times New Roman\", serif" },
];

export const MONO_FONTS: readonly FontPreset[] = [
  { id: "jetbrains", stack: "\"JetBrains Mono\", \"Cascadia Code\", Consolas, ui-monospace, monospace" },
  { id: "cascadia", stack: "\"Cascadia Code\", \"Cascadia Mono\", Consolas, ui-monospace, monospace" },
  { id: "consolas", stack: "Consolas, \"Courier New\", ui-monospace, monospace" },
  { id: "sfmono", stack: "\"SF Mono\", Menlo, Monaco, ui-monospace, monospace" },
  { id: "fira", stack: "\"Fira Code\", \"Fira Mono\", Consolas, ui-monospace, monospace" },
];

/** Base values (tokens.css) of the scaled token families. */
export const FONT_SIZES: Readonly<Record<string, number>> = {
  "font-size-xs": 11,
  "font-size-sm": 12,
  "font-size-md": 13,
  "font-size-lg": 15,
  "font-size-xl": 18,
  "font-size-2xl": 24,
};
export const RADII: Readonly<Record<string, number>> = {
  "radius-xs": 4,
  "radius-sm": 6,
  "radius-md": 10,
  "radius-lg": 14,
  "radius-xl": 20,
};
export const SPACES: Readonly<Record<string, number>> = {
  "space-1": 4,
  "space-2": 8,
  "space-3": 12,
  "space-4": 16,
  "space-5": 20,
  "space-6": 24,
  "space-8": 32,
  "space-10": 40,
};
export const FONT_WEIGHTS: Readonly<Record<string, number>> = {
  "font-weight-regular": 400,
  "font-weight-medium": 500,
  "font-weight-semibold": 600,
};

export const EASING_PRESETS: Readonly<Record<string, string>> = {
  smooth: "cubic-bezier(0.2, 0.8, 0.2, 1)",
  snappy: "cubic-bezier(0.3, 0, 0, 1)",
  gentle: "cubic-bezier(0.4, 0, 0.2, 1)",
  linear: "linear",
};

export type ShadowPreset = "none" | "soft" | "medium" | "deep";
export const SHADOW_PRESETS: Readonly<Record<ShadowPreset, Record<string, string>>> = {
  none: { "shadow-sm": "0 0 0 transparent", "shadow-md": "0 0 0 transparent", "shadow-panel": "0 0 0 transparent" },
  soft: {
    "shadow-sm": "0 1px 2px rgba(0, 0, 0, 0.25)",
    "shadow-md": "0 6px 18px rgba(0, 0, 0, 0.22)",
    "shadow-panel": "0 10px 30px rgba(0, 0, 0, 0.25), inset 0 1px 0 rgba(255, 255, 255, 0.04)",
  },
  medium: {
    "shadow-sm": "0 1px 2px rgba(0, 0, 0, 0.4)",
    "shadow-md": "0 8px 24px rgba(0, 0, 0, 0.38)",
    "shadow-panel": "0 18px 48px rgba(0, 0, 0, 0.45), inset 0 1px 0 rgba(255, 255, 255, 0.05)",
  },
  deep: {
    "shadow-sm": "0 2px 4px rgba(0, 0, 0, 0.5)",
    "shadow-md": "0 12px 32px rgba(0, 0, 0, 0.55)",
    "shadow-panel": "0 28px 70px rgba(0, 0, 0, 0.6), inset 0 1px 0 rgba(255, 255, 255, 0.06)",
  },
};

/** Sound events the theme can map (§33). INTEGRATOR(voice-sound): keep in sync with the Sound Engine's event keys. */
export const SOUND_EVENTS: readonly string[] = [
  "wake",
  "task.started",
  "task.completed",
  "agent.error",
  "approval.requested",
  "build.failed",
  "deploy.finished",
  "alert",
];

/** Sound preset ids. INTEGRATOR(voice-sound): replace with the Sound Engine's preset catalogue. */
export const SOUND_PRESETS: readonly string[] = ["none", "chime-soft", "chime-bright", "ping", "pulse", "thud", "alert-soft", "alert-strong", "whoosh"];

/** Robot skin ids. INTEGRATOR(robots): replace with the Robot Designer's skin catalogue. */
export const ROBOT_SKINS: readonly string[] = ["orbit", "pixel", "nova", "drone"];

// ---------------------------------------------------------------------------
// Scales
// ---------------------------------------------------------------------------

function px(v: string | undefined): number | null {
  if (v === undefined || !v.trim().endsWith("px")) return null;
  return plainNumber(v.trim().slice(0, -2));
}

/** Current scale of a token family relative to its base (median of overridden ratios; 1 if none). */
export function familyScale(tokens: Readonly<Record<string, string>>, base: Readonly<Record<string, number>>): number {
  const ratios: number[] = [];
  for (const [k, b] of Object.entries(base)) {
    const v = px(tokens[k]);
    if (v !== null && b > 0) ratios.push(v / b);
  }
  if (ratios.length === 0) return 1;
  ratios.sort((a, b) => a - b);
  return Math.round((ratios[Math.floor(ratios.length / 2)] ?? 1) * 100) / 100;
}

/** Writes a token family scaled from its base (scale 1 removes the overrides). */
export function withFamilyScale(
  tokens: Readonly<Record<string, string>>,
  base: Readonly<Record<string, number>>,
  scale: number,
): Record<string, string> {
  const out = { ...tokens };
  for (const [k, b] of Object.entries(base)) {
    if (Math.abs(scale - 1) < 0.001) delete out[k];
    else out[k] = `${Math.round(b * scale * 10) / 10}px`;
  }
  return out;
}

/** Sets or removes (`null`/empty) one token. */
export function withToken(tokens: Readonly<Record<string, string>>, name: string, value: string | null): Record<string, string> {
  const out = { ...tokens };
  if (value === null || value.trim() === "") delete out[name];
  else out[name] = value;
  return out;
}

/** Which shadow preset the tokens equal (`null` = custom). */
export function shadowPresetOf(tokens: Readonly<Record<string, string>>): ShadowPreset | null {
  if (!("shadow-panel" in tokens) && !("shadow-md" in tokens) && !("shadow-sm" in tokens)) return "medium";
  for (const [id, preset] of Object.entries(SHADOW_PRESETS) as [ShadowPreset, Record<string, string>][]) {
    if (Object.entries(preset).every(([k, v]) => tokens[k] === v)) return id;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Ids, duplication, import
// ---------------------------------------------------------------------------

/** `My Theme 2!` → `my-theme-2` (always a valid id). */
export function slugify(name: string): string {
  const s = name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  const id = /^[a-z]/.test(s) ? s : `theme-${s}`.replace(/-+$/, "");
  return isValidTokenName(id) ? id : "theme";
}

/** A free id derived from `name` (never a built-in id or one in `taken`). */
export function uniqueThemeId(name: string, taken: ReadonlySet<string>): string {
  const base = slugify(name);
  const free = (id: string) => !taken.has(id) && !isBuiltinTheme(id);
  if (free(base)) return base;
  for (let i = 2; i < 10_000; i += 1) {
    const id = `${base.slice(0, 56)}-${i}`;
    if (free(id)) return id;
  }
  return `${base.slice(0, 40)}-${Date.now().toString(36)}`;
}

/** A copy of `theme` under a new id and name. */
export function duplicateTheme(theme: ThemeConfig, name: string, taken: ReadonlySet<string>): ThemeConfig {
  const copy = structuredClone(normalizeTheme(theme));
  copy.name = [...name].slice(0, 80).join("");
  copy.id = uniqueThemeId(copy.name, taken);
  return copy;
}

/**
 * Prepares an imported theme for the library: built-in or taken ids get a
 * fresh id (the import never overwrites anything silently).
 */
export function prepareImport(theme: ThemeConfig, taken: ReadonlySet<string>): ThemeConfig {
  const t = structuredClone(theme);
  if (isBuiltinTheme(t.id) || taken.has(t.id)) t.id = uniqueThemeId(t.name, taken);
  return t;
}

/** File name of an exported theme. */
export function exportFileName(theme: ThemeConfig): string {
  return `${theme.id}.orkestr-theme.json`;
}

// ---------------------------------------------------------------------------
// Draft state machine
// ---------------------------------------------------------------------------

export interface DraftState {
  /** The theme being edited. */
  draft: ThemeConfig;
  /** Saved version it started from (reset target). */
  base: ThemeConfig;
  /** Validation issues of `draft` (empty = valid). */
  issues: UiConfigIssue[];
  /** Last valid version of `draft` — what the live preview shows. */
  preview: ThemeConfig;
  /** `draft` differs from `base`. */
  dirty: boolean;
}

function same(a: ThemeConfig, b: ThemeConfig): boolean {
  return JSON.stringify(normalizeTheme(a)) === JSON.stringify(normalizeTheme(b));
}

/** Starts editing `base`. */
export function startDraft(base: ThemeConfig): DraftState {
  const b = normalizeTheme(structuredClone(base));
  return { draft: structuredClone(b), base: b, issues: validateTheme(b), preview: b, dirty: false };
}

/**
 * Applies an edit: the draft always changes (so the user sees what they
 * typed), the live preview only advances when the result is valid — an
 * invalid or illegible theme is never applied to the app (§70).
 */
export function editDraft(state: DraftState, edit: (t: ThemeConfig) => ThemeConfig): DraftState {
  const draft = normalizeTheme(edit(structuredClone(state.draft)));
  const issues = validateTheme(draft);
  return { ...state, draft, issues, preview: issues.length === 0 ? draft : state.preview, dirty: !same(draft, state.base) };
}

/** Back to the saved version. */
export function resetDraft(state: DraftState): DraftState {
  return startDraft(state.base);
}

/** Whether saving would create a new library entry (built-ins are read-only). */
export function savesAsNew(state: DraftState, userIds: ReadonlySet<string>): boolean {
  return isBuiltinTheme(state.draft.id) || !userIds.has(state.draft.id);
}
