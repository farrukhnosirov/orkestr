/**
 * WCAG 2.x contrast checks — mirror of `orkestr_protocol::ui_theme`
 * (`contrast_report`). Rules and base colors are generated from Rust.
 */
import type { ContrastCheck, Rgba, ThemeConfig } from "@orkestr/protocol";
import { BASE_COLORS, CONTRAST_RULES } from "@orkestr/protocol";

import { GRADIENT_FNS, parseColor, splitTopLevel } from "./grammar";

function lin(c: number): number {
  const v = c / 255;
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
}

/** Relative luminance of an opaque color. */
export function luminance(c: Rgba): number {
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
}

/** `fg` composited over an opaque `bg`. */
export function composite(fg: Rgba, bg: Rgba): Rgba {
  const a = fg.a;
  return { r: fg.r * a + bg.r * (1 - a), g: fg.g * a + bg.g * (1 - a), b: fg.b * a + bg.b * (1 - a), a: 1 };
}

/** WCAG contrast ratio (1..21) of `fg` over `bg` (both composited onto `canvas`). */
export function contrastRatio(fg: Rgba, bg: Rgba, canvas: Rgba): number {
  const b = composite(bg, canvas);
  const f = composite(fg, b);
  const l1 = luminance(f);
  const l2 = luminance(b);
  const hi = Math.max(l1, l2);
  const lo = Math.min(l1, l2);
  return (hi + 0.05) / (lo + 0.05);
}

/** Base (tokens.css) value of a contrast-relevant token for a mode. */
export function baseColor(mode: ThemeConfig["mode"], token: string): string | null {
  return BASE_COLORS[mode][token] ?? null;
}

/**
 * The color stops of a gradient accepted by the `Gradient` grammar
 * (`GRADIENT_FNS`), or a single color for a plain color value. Parts that
 * are not colors (angles, `to right`, `circle at …`) are skipped. `null` when
 * the value is neither, or has no literal color stop (e.g. `var(...)` stops).
 * Mirror of `orkestr_protocol::ui_theme::color_stops`.
 */
export function colorStops(input: string): Rgba[] | null {
  const s = input.trim().toLowerCase();
  const single = parseColor(s);
  if (single) return [single];
  const fn = GRADIENT_FNS.find((f) => s.startsWith(f));
  if (fn === undefined || !s.endsWith(")")) return null;
  const stops: Rgba[] = [];
  for (const raw of splitTopLevel(s.slice(fn.length, -1))) {
    const part = raw.trim();
    const open = part.indexOf("(");
    let color: string | undefined;
    if (open >= 0 && !/\s/.test(part.slice(0, open))) {
      const close = part.indexOf(")");
      color = close >= 0 ? part.slice(0, close + 1) : undefined;
    } else {
      color = part.split(/\s+/).find((p) => p !== "");
    }
    const c = color === undefined ? null : parseColor(color);
    if (c) stops.push(c);
    else if (color?.startsWith("var(")) return null;
  }
  return stops.length > 0 ? stops : null;
}

function resolve(theme: ThemeConfig, token: string): Rgba[] | null {
  const v = theme.tokens[token];
  if (v !== undefined) return colorStops(v);
  const base = baseColor(theme.mode, token);
  return base === null ? null : colorStops(base);
}

/** Runs every contrast rule whose colors resolve to literals (gradients: worst stop). */
export function contrastReport(theme: ThemeConfig): ContrastCheck[] {
  const canvas: Rgba = theme.mode === "Light" ? { r: 255, g: 255, b: 255, a: 1 } : { r: 0, g: 0, b: 0, a: 1 };
  const out: ContrastCheck[] = [];
  for (const rule of CONTRAST_RULES) {
    const fg = resolve(theme, rule.fg);
    const bg = resolve(theme, rule.bg);
    if (!fg || !bg) continue;
    // The worst pair decides (a text color is a single stop).
    let worst = Infinity;
    for (const f of fg) for (const b of bg) worst = Math.min(worst, contrastRatio(f, b, canvas));
    const ratio = Math.round(worst * 100) / 100;
    out.push({ id: rule.id, fg: rule.fg, bg: rule.bg, ratio, min: rule.min, hard: rule.hard, pass: ratio >= rule.min });
  }
  return out;
}
