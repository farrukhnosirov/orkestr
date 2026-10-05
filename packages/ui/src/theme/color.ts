/**
 * Color math for the theme editor (HSV picker, derived accent tokens).
 * Pure functions; values are sRGB 0..255 with straight alpha.
 */
import type { Rgba } from "@orkestr/protocol";

import { parseColor, toHex } from "./grammar";

export interface Hsva {
  /** Hue 0..360. */
  h: number;
  /** Saturation 0..1. */
  s: number;
  /** Value 0..1. */
  v: number;
  /** Alpha 0..1. */
  a: number;
}

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

export function rgbaToHsva(c: Rgba): Hsva {
  const r = c.r / 255;
  const g = c.g / 255;
  const b = c.b / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d > 0) {
    if (max === r) h = 60 * (((g - b) / d) % 6);
    else if (max === g) h = 60 * ((b - r) / d + 2);
    else h = 60 * ((r - g) / d + 4);
  }
  if (h < 0) h += 360;
  return { h, s: max === 0 ? 0 : d / max, v: max, a: c.a };
}

export function hsvaToRgba(c: Hsva): Rgba {
  const h = ((c.h % 360) + 360) % 360;
  const s = clamp(c.s, 0, 1);
  const v = clamp(c.v, 0, 1);
  const k = (n: number) => (n + h / 60) % 6;
  const f = (n: number) => v - v * s * Math.max(0, Math.min(k(n), 4 - k(n), 1));
  return { r: f(5) * 255, g: f(3) * 255, b: f(1) * 255, a: clamp(c.a, 0, 1) };
}

/** `rgba(r, g, b, a)` with integer channels and a 2-decimal alpha. */
export function toRgbaString(c: Rgba): string {
  const r = Math.round(clamp(c.r, 0, 255));
  const g = Math.round(clamp(c.g, 0, 255));
  const b = Math.round(clamp(c.b, 0, 255));
  const a = Math.round(clamp(c.a, 0, 1) * 100) / 100;
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}

/** Canonical string for an edited color: hex when opaque, `rgba()` otherwise. */
export function formatColor(c: Rgba): string {
  return c.a >= 1 ? toHex(c) : toRgbaString(c);
}

/** Mixes `a` toward `b` by `t` (0..1), alpha included. */
export function mix(a: Rgba, b: Rgba, t: number): Rgba {
  const k = clamp(t, 0, 1);
  return { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: a.a + (b.a - a.a) * k };
}

/** Same color with another alpha. */
export function withAlpha(c: Rgba, a: number): Rgba {
  return { ...c, a: clamp(a, 0, 1) };
}

const BLACK: Rgba = { r: 0, g: 0, b: 0, a: 1 };

/**
 * Tokens derived from one accent color (strong/soft variants and the accent
 * border), so picking a single color re-tints the system consistently. Glow
 * and focus ring follow `--ok-color-accent` by themselves (tokens.css uses
 * `color-mix`). Returns `{}` for an unparsable color.
 */
export function deriveAccentTokens(accent: string, mode: "Dark" | "Light"): Record<string, string> {
  const c = parseColor(accent);
  if (!c) return {};
  const opaque = withAlpha(c, 1);
  return {
    "color-accent": toHex(opaque),
    "color-accent-strong": toHex(mix(opaque, BLACK, mode === "Light" ? 0.22 : 0.15)),
    "color-accent-soft": toRgbaString(withAlpha(opaque, mode === "Light" ? 0.12 : 0.14)),
    "color-border-accent": toRgbaString(withAlpha(opaque, 0.45)),
  };
}
