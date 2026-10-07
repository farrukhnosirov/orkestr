/**
 * Builds the Monaco theme `orkestr-dark` from the live `--ok-*` design
 * tokens, so every Orkestr theme (and AI-edited UI config) restyles the
 * editor without hardcoded colors. Pure: no Monaco import (unit tested).
 */

/** Subset of `monaco.editor.IStandaloneThemeData` used here. */
export interface ThemeData {
  base: "vs-dark" | "vs";
  inherit: boolean;
  rules: { token: string; foreground?: string; fontStyle?: string }[];
  colors: Record<string, string>;
}

export const MONACO_THEME = "orkestr-dark";

function hex2(n: number): string {
  return Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");
}

/**
 * Converts a CSS color (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
 * `rgba()`) to Monaco's `#rrggbb[aa]`. Returns null for anything else.
 */
export function cssColorToHex(value: string): string | null {
  const v = value.trim().toLowerCase();
  if (/^#[0-9a-f]{3,4}$/.test(v)) {
    return `#${[...v.slice(1)].map((c) => c + c).join("")}`;
  }
  if (/^#[0-9a-f]{6}([0-9a-f]{2})?$/.test(v)) return v;
  const m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s,/]+([\d.]+%?))?\s*\)$/.exec(v);
  if (!m) return null;
  const [r, g, b] = [Number(m[1]), Number(m[2]), Number(m[3])];
  let out = `#${hex2(r)}${hex2(g)}${hex2(b)}`;
  if (m[4] !== undefined) {
    const a = m[4].endsWith("%") ? Number(m[4].slice(0, -1)) / 100 : Number(m[4]);
    if (a < 1) out += hex2(a * 255);
  }
  return out;
}

/** Current value of a color token as `#rrggbb[aa]` (fallback to the Nebula default). */
export function tokenHex(token: string): string {
  const raw = typeof document === "undefined" ? "" : getComputedStyle(document.documentElement).getPropertyValue(token);
  return cssColorToHex(raw) ?? FALLBACK[token] ?? "#808080";
}

/** Applies an alpha (0..1) to a `#rrggbb[aa]` color. */
export function withAlpha(hex: string, alpha: number): string {
  return `${hex.slice(0, 7)}${hex2(alpha * 255)}`;
}

const FALLBACK: Record<string, string> = {
  "--ok-color-bg": "#04050d",
  "--ok-color-bg-elevated": "#0a0d1c",
  "--ok-color-surface-sunken": "#0507128c",
  "--ok-color-surface-hover": "#465aaa29",
  "--ok-color-surface-active": "#506ed23d",
  "--ok-color-border": "#889cff1f",
  "--ok-color-border-strong": "#96afff3d",
  "--ok-color-text": "#e8ecff",
  "--ok-color-text-muted": "#9aa3c8",
  "--ok-color-text-subtle": "#727ba6",
  "--ok-color-accent": "#22d3ee",
  "--ok-color-violet": "#a78bfa",
  "--ok-color-magenta": "#f472b6",
  "--ok-color-success": "#34d399",
  "--ok-color-warning": "#fbbf24",
  "--ok-color-danger": "#fb7185",
  "--ok-color-info": "#60a5fa",
};

/** Builds the theme from a token reader (e.g. `getComputedStyle(root).getPropertyValue`). */
export function buildTheme(read: (token: string) => string): ThemeData {
  const c = (token: string): string => cssColorToHex(read(token)) ?? FALLBACK[token] ?? "#808080";
  const bare = (token: string) => c(token).slice(1, 7);
  const text = c("--ok-color-text");
  const accent = c("--ok-color-accent");
  const violet = c("--ok-color-violet");
  const success = c("--ok-color-success");
  const danger = c("--ok-color-danger");
  const warning = c("--ok-color-warning");
  const elevated = c("--ok-color-bg-elevated");
  return {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "", foreground: bare("--ok-color-text") },
      { token: "comment", foreground: bare("--ok-color-text-subtle"), fontStyle: "italic" },
      { token: "keyword", foreground: bare("--ok-color-violet") },
      { token: "keyword.flow", foreground: bare("--ok-color-magenta") },
      { token: "storage", foreground: bare("--ok-color-violet") },
      { token: "string", foreground: bare("--ok-color-success") },
      { token: "string.escape", foreground: bare("--ok-color-accent") },
      { token: "regexp", foreground: bare("--ok-color-magenta") },
      { token: "number", foreground: bare("--ok-color-warning") },
      { token: "constant", foreground: bare("--ok-color-warning") },
      { token: "type", foreground: bare("--ok-color-accent") },
      { token: "type.identifier", foreground: bare("--ok-color-accent") },
      { token: "identifier", foreground: bare("--ok-color-text") },
      { token: "function", foreground: bare("--ok-color-info") },
      { token: "delimiter", foreground: bare("--ok-color-text-muted") },
      { token: "operator", foreground: bare("--ok-color-text-muted") },
      { token: "tag", foreground: bare("--ok-color-magenta") },
      { token: "attribute.name", foreground: bare("--ok-color-accent") },
      { token: "attribute.value", foreground: bare("--ok-color-success") },
      { token: "key", foreground: bare("--ok-color-info") },
      { token: "string.key.json", foreground: bare("--ok-color-info") },
      { token: "string.value.json", foreground: bare("--ok-color-success") },
      { token: "variable", foreground: bare("--ok-color-text") },
      { token: "metatag", foreground: bare("--ok-color-magenta") },
      { token: "annotation", foreground: bare("--ok-color-warning") },
      { token: "invalid", foreground: bare("--ok-color-danger") },
    ],
    colors: {
      "editor.background": "#00000000",
      "editor.foreground": text,
      "editorLineNumber.foreground": c("--ok-color-text-subtle"),
      "editorLineNumber.activeForeground": accent,
      "editorCursor.foreground": accent,
      "editor.selectionBackground": withAlpha(accent, 0.24),
      "editor.inactiveSelectionBackground": withAlpha(accent, 0.12),
      "editor.selectionHighlightBackground": withAlpha(violet, 0.18),
      "editor.wordHighlightBackground": withAlpha(violet, 0.16),
      "editor.findMatchBackground": withAlpha(warning, 0.35),
      "editor.findMatchHighlightBackground": withAlpha(warning, 0.18),
      "editor.lineHighlightBackground": withAlpha(c("--ok-color-surface-hover").slice(0, 7), 0.12),
      "editor.lineHighlightBorder": "#00000000",
      "editorIndentGuide.background1": withAlpha(text, 0.07),
      "editorIndentGuide.activeBackground1": withAlpha(accent, 0.35),
      "editorWhitespace.foreground": withAlpha(text, 0.12),
      "editorBracketMatch.background": withAlpha(accent, 0.15),
      "editorBracketMatch.border": withAlpha(accent, 0.5),
      "editorGutter.background": "#00000000",
      "editorWidget.background": elevated,
      "editorWidget.border": c("--ok-color-border-strong"),
      "editorSuggestWidget.background": elevated,
      "editorSuggestWidget.border": c("--ok-color-border-strong"),
      "editorSuggestWidget.selectedBackground": withAlpha(accent, 0.18),
      "editorHoverWidget.background": elevated,
      "editorHoverWidget.border": c("--ok-color-border-strong"),
      "editorError.foreground": danger,
      "editorWarning.foreground": warning,
      "editorInfo.foreground": c("--ok-color-info"),
      "editorOverviewRuler.border": "#00000000",
      "minimap.background": "#00000000",
      "minimapSlider.background": withAlpha(text, 0.08),
      "scrollbar.shadow": "#00000000",
      "scrollbarSlider.background": withAlpha(text, 0.1),
      "scrollbarSlider.hoverBackground": withAlpha(text, 0.18),
      "scrollbarSlider.activeBackground": withAlpha(accent, 0.3),
      "diffEditor.insertedTextBackground": withAlpha(success, 0.16),
      "diffEditor.removedTextBackground": withAlpha(danger, 0.16),
      "diffEditor.insertedLineBackground": withAlpha(success, 0.08),
      "diffEditor.removedLineBackground": withAlpha(danger, 0.08),
      "input.background": c("--ok-color-bg"),
      "input.border": c("--ok-color-border-strong"),
      focusBorder: withAlpha(accent, 0.6),
      "list.hoverBackground": withAlpha(accent, 0.1),
      "list.activeSelectionBackground": withAlpha(accent, 0.2),
      "menu.background": elevated,
      "menu.foreground": text,
      "menu.selectionBackground": withAlpha(accent, 0.2),
      "menu.border": c("--ok-color-border-strong"),
    },
  };
}
