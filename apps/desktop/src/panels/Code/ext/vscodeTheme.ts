/**
 * VS Code color themes → Monaco themes (pure; unit tested).
 *
 * Two kinds of token rules are produced:
 * - **TextMate tokens** (`tm.<fg>.<fontStyle>`): languages tokenized by an
 *   installed TextMate grammar are colored by vscode-textmate itself with the
 *   theme's scope selectors (exactly like VS Code); its color map index is
 *   encoded in the token name and mapped back to a color here.
 * - **Monarch fallbacks**: Monaco's built-in tokenizers emit generic names
 *   (`comment`, `keyword`, `string.key.json`, …); each is colored with the
 *   theme rule whose scope best matches a representative TextMate scope.
 */
import type { ResolvedTheme, TokenColorRule } from "@orkestr/protocol";

import type { ThemeData } from "../theme";

/** Monaco base theme for a VS Code `uiTheme`. */
export function baseOf(uiTheme: string): ThemeData["base"] | "hc-black" | "hc-light" {
  switch (uiTheme) {
    case "vs":
      return "vs";
    case "hc-black":
      return "hc-black";
    case "hc-light":
      return "hc-light";
    default:
      return "vs-dark";
  }
}

/** Whether a VS Code `uiTheme` is light. */
export function isLightUiTheme(uiTheme: string): boolean {
  return uiTheme === "vs" || uiTheme === "hc-light";
}

/** Monaco token → representative TextMate scope. */
export const MONARCH_SCOPES: ReadonlyArray<readonly [string, string]> = [
  ["comment", "comment"],
  ["string", "string"],
  ["string.escape", "constant.character.escape"],
  ["regexp", "string.regexp"],
  ["keyword", "keyword"],
  ["keyword.flow", "keyword.control"],
  ["storage", "storage.type"],
  ["number", "constant.numeric"],
  ["constant", "constant.language"],
  ["type", "entity.name.type"],
  ["type.identifier", "entity.name.type"],
  ["function", "entity.name.function"],
  ["variable", "variable"],
  ["identifier", "variable"],
  ["delimiter", "punctuation"],
  ["operator", "keyword.operator"],
  ["tag", "entity.name.tag"],
  ["attribute.name", "entity.other.attribute-name"],
  ["attribute.value", "string"],
  ["key", "support.type.property-name"],
  ["string.key.json", "support.type.property-name"],
  ["string.value.json", "string"],
  ["metatag", "entity.name.tag"],
  ["annotation", "meta.decorator"],
  ["invalid", "invalid"],
];

/** Whether a simple theme scope selector matches a scope (prefix by dot segments). */
export function scopeMatches(selector: string, scope: string): boolean {
  return scope === selector || scope.startsWith(`${selector}.`);
}

/**
 * The theme rule that best matches `scope` (longest matching selector; a
 * later rule wins a tie, as in VS Code). Selectors with parent scopes
 * (`a b`) or exclusions (`a - b`) are ignored here — those only matter for
 * real TextMate tokenization, which vscode-textmate handles.
 */
export function bestRule(rules: readonly TokenColorRule[], scope: string): TokenColorRule | null {
  let best: TokenColorRule | null = null;
  let bestLen = -1;
  for (const r of rules) {
    for (const sel of r.scope) {
      const s = sel.trim();
      if (!/^[\w.+-]+$/.test(s)) continue;
      if (scopeMatches(s, scope) && s.length >= bestLen && (r.foreground || r.fontStyle !== null)) {
        best = r;
        bestLen = s.length;
      }
    }
  }
  return best;
}

const bare = (hex: string): string => hex.replace(/^#/, "");

/** Monaco `fontStyle` from a TextMate font style string. */
export function monacoFontStyle(fs: string | null | undefined): string | undefined {
  if (fs === null || fs === undefined) return undefined;
  const parts = fs
    .split(/\s+/)
    .filter((p) => p === "italic" || p === "bold" || p === "underline" || p === "strikethrough");
  return parts.join(" ");
}

/** Font style bits used by vscode-textmate (Italic 1, Bold 2, Underline 4, Strikethrough 8). */
export function fontStyleName(bits: number): string {
  const out: string[] = [];
  if (bits & 1) out.push("italic");
  if (bits & 2) out.push("bold");
  if (bits & 4) out.push("underline");
  if (bits & 8) out.push("strikethrough");
  return out.join(" ");
}

/** Token name emitted by the TextMate tokenizer for a color index + font style. */
export function tmToken(foreground: number, fontStyle: number): string {
  return `tm.${foreground}.${fontStyle}`;
}

/** Monaco rules for a vscode-textmate color map (index 0 is unused). */
export function colorMapRules(colorMap: readonly string[]): ThemeData["rules"] {
  const rules: ThemeData["rules"] = [];
  for (let i = 1; i < colorMap.length; i++) {
    const c = colorMap[i];
    if (!c) continue;
    for (let fs = 0; fs < 16; fs++) {
      const style = fontStyleName(fs);
      rules.push(style ? { token: tmToken(i, fs), foreground: bare(c), fontStyle: style } : { token: tmToken(i, fs), foreground: bare(c) });
    }
  }
  return rules;
}

/** The theme as a vscode-textmate raw theme. */
export function toRawTheme(theme: Pick<ResolvedTheme, "colors" | "tokenColors">): {
  settings: { scope?: string[]; settings: { foreground?: string; background?: string; fontStyle?: string } }[];
} {
  const defaults: { foreground?: string; background?: string } = {};
  const fg = theme.colors["editor.foreground"];
  const bg = theme.colors["editor.background"];
  if (fg) defaults.foreground = fg;
  if (bg) defaults.background = bg;
  const settings: { scope?: string[]; settings: { foreground?: string; background?: string; fontStyle?: string } }[] = [
    { settings: defaults },
  ];
  for (const r of theme.tokenColors) {
    const s: { foreground?: string; background?: string; fontStyle?: string } = {};
    if (r.foreground) s.foreground = r.foreground;
    if (r.background) s.background = r.background;
    if (r.fontStyle !== null) s.fontStyle = r.fontStyle;
    settings.push(r.scope.length ? { scope: r.scope, settings: s } : { settings: s });
  }
  return { settings };
}

/** Monaco theme data for a resolved VS Code theme (+ TextMate color map rules). */
export function toMonacoTheme(
  theme: Pick<ResolvedTheme, "uiTheme" | "colors" | "tokenColors">,
  colorMap: readonly string[] = [],
): { base: "vs" | "vs-dark" | "hc-black" | "hc-light"; inherit: boolean; rules: ThemeData["rules"]; colors: Record<string, string> } {
  const rules: ThemeData["rules"] = [];
  const defaultRule = theme.tokenColors.filter((r) => r.scope.length === 0 && r.foreground).pop();
  const fg = theme.colors["editor.foreground"] ?? defaultRule?.foreground;
  if (fg) rules.push({ token: "", foreground: bare(fg) });
  for (const [token, scope] of MONARCH_SCOPES) {
    const r = bestRule(theme.tokenColors, scope);
    if (!r) continue;
    const rule: { token: string; foreground?: string; fontStyle?: string } = { token };
    if (r.foreground) rule.foreground = bare(r.foreground);
    const fs = monacoFontStyle(r.fontStyle);
    if (fs !== undefined) rule.fontStyle = fs;
    rules.push(rule);
  }
  rules.push(...colorMapRules(colorMap));
  return { base: baseOf(theme.uiTheme), inherit: true, rules, colors: { ...theme.colors } };
}

/** A TextMate raw theme equivalent to Orkestr's token-driven theme (for TextMate-tokenized languages). */
export function rawThemeFromMonaco(data: ThemeData): ReturnType<typeof toRawTheme> {
  const byToken = new Map(data.rules.map((r) => [r.token, r]));
  const tokenColors: TokenColorRule[] = [];
  for (const [token, scope] of MONARCH_SCOPES) {
    const r = byToken.get(token);
    if (!r?.foreground) continue;
    tokenColors.push({ scope: [scope], foreground: `#${r.foreground}`, background: null, fontStyle: r.fontStyle ?? null });
  }
  // A few scopes TextMate grammars use that Monarch has no token for.
  const extra: [string, string][] = [
    ["entity.name.function", "function"],
    ["support.function", "function"],
    ["meta.function-call", "function"],
    ["entity.name.type", "type"],
    ["support.type", "type"],
    ["entity.name.class", "type"],
    ["variable.parameter", "variable"],
    ["punctuation.definition.comment", "comment"],
    ["constant.character", "string.escape"],
  ];
  for (const [scope, token] of extra) {
    const r = byToken.get(token);
    if (r?.foreground) tokenColors.push({ scope: [scope], foreground: `#${r.foreground}`, background: null, fontStyle: r.fontStyle ?? null });
  }
  const colors: Record<string, string> = {};
  const fg = data.colors["editor.foreground"];
  if (fg) colors["editor.foreground"] = fg;
  return toRawTheme({ colors, tokenColors });
}
