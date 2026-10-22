/**
 * The living sphere (ADR-0045): a Canvas 2D renderer of layered gradients —
 * glow, a plasma core of drifting colour blobs, a specular highlight, and
 * state layers (rotating rings while agents work, a pulsing ring for
 * approvals, a warning tint on errors, a waveform ring while listening,
 * ripples while speaking, orbiting particles in full motion).
 *
 * Colours come from the theme tokens (`--ok-color-*`), re-read whenever the
 * theme changes (`themeKey`). Frame rate follows the motion level: 0 fps
 * draws one still frame (Battery Saver, unfocused app, reduced motion).
 */
import { useEffect, useRef } from "react";

import type { OrbMood, OrbMotion } from "./model";

export interface OrbSphereProps {
  /** Sphere diameter in CSS px. */
  size: number;
  /** Room around the sphere (glow, rings) in CSS px. */
  pad: number;
  mood: OrbMood;
  motion: OrbMotion;
  /** Frame-rate cap (0 = still). */
  fps: number;
  /** Ring count while working (1..3). */
  rings: number;
  /** Microphone level 0..1 (listening). */
  level: number;
  /** Changes whenever the theme changes (colours are re-read). */
  themeKey: string;
  /** Hovered / pressed: a little brighter. */
  lit: boolean;
  /** Theme override of the orb accent (`theme.robot.accent`; null = the theme accent). */
  accent: string | null;
}

type Rgb = readonly [number, number, number];

interface Palette {
  accent: Rgb;
  accentStrong: Rgb;
  violet: Rgb;
  info: Rgb;
  warning: Rgb;
  danger: Rgb;
  success: Rgb;
  bg: Rgb;
  text: Rgb;
  muted: Rgb;
}

const FALLBACK: Palette = {
  accent: [56, 189, 248],
  accentStrong: [125, 211, 252],
  violet: [139, 92, 246],
  info: [96, 165, 250],
  warning: [245, 158, 11],
  danger: [239, 68, 68],
  success: [34, 197, 94],
  bg: [8, 12, 24],
  text: [226, 232, 240],
  muted: [100, 116, 139],
};

/** Parses `#rgb`, `#rrggbb(aa)` and `rgb(a)(…)` (what a canvas normalizes colours to). */
export function parseRgb(value: string): Rgb | null {
  const v = value.trim();
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(v);
  if (hex?.[1]) {
    const h = hex[1];
    if (h.length === 3) return [parseInt(h[0]! + h[0]!, 16), parseInt(h[1]! + h[1]!, 16), parseInt(h[2]! + h[2]!, 16)];
    return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
  }
  const rgb = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(v);
  if (rgb?.[1] && rgb[2] && rgb[3]) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}

function readPalette(probe: CanvasRenderingContext2D, accentOverride: string | null): Palette {
  const css = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: Rgb): Rgb => {
    const raw = css.getPropertyValue(`--ok-color-${name}`).trim();
    if (!raw) return fallback;
    // The canvas normalizes any CSS colour (hex, hsl, oklch, color-mix…) to hex / rgba.
    probe.fillStyle = "#000000";
    probe.fillStyle = raw;
    return parseRgb(String(probe.fillStyle)) ?? fallback;
  };
  let accent = read("accent", FALLBACK.accent);
  if (accentOverride) {
    probe.fillStyle = "#000000";
    probe.fillStyle = accentOverride;
    accent = parseRgb(String(probe.fillStyle)) ?? accent;
  }
  return {
    accent,
    accentStrong: read("accent-strong", FALLBACK.accentStrong),
    violet: read("violet", FALLBACK.violet),
    info: read("info", FALLBACK.info),
    warning: read("warning", FALLBACK.warning),
    danger: read("danger", FALLBACK.danger),
    success: read("success", FALLBACK.success),
    bg: read("bg", FALLBACK.bg),
    text: read("text", FALLBACK.text),
    muted: read("text-muted", FALLBACK.muted),
  };
}

const rgba = (c: Rgb, a: number) => `rgba(${c[0]},${c[1]},${c[2]},${Math.max(0, Math.min(1, a)).toFixed(3)})`;

function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t].map(Math.round) as unknown as Rgb;
}

interface MoodColors {
  main: Rgb;
  second: Rgb;
  third: Rgb;
}

function moodColors(p: Palette, mood: OrbMood): MoodColors {
  switch (mood) {
    case "approval":
      return { main: p.warning, second: p.accent, third: p.violet };
    case "error":
      return { main: p.danger, second: p.warning, third: p.violet };
    case "stopped":
      return { main: mix(p.danger, p.muted, 0.5), second: p.muted, third: p.danger };
    case "offline":
      return { main: p.muted, second: mix(p.muted, p.bg, 0.4), third: p.muted };
    case "listening":
      return { main: p.accent, second: p.info, third: p.accentStrong };
    case "speaking":
      return { main: p.violet, second: p.accent, third: p.info };
    default:
      return { main: p.accent, second: p.violet, third: p.info };
  }
}

interface Frame {
  t: number;
  size: number;
  pad: number;
  mood: OrbMood;
  motion: OrbMotion;
  rings: number;
  level: number;
  lit: boolean;
}

/** Draws one frame. `t` in seconds. */
export function drawOrb(ctx: CanvasRenderingContext2D, p: Palette, f: Frame): void {
  const box = f.size + f.pad * 2;
  const c = box / 2;
  const full = f.motion === "full";
  const moving = f.motion !== "static";
  const col = moodColors(p, f.mood);
  const breathe = full && (f.mood === "idle" || f.mood === "working") ? Math.sin(f.t * 1.6) * 0.022 : 0;
  const speak = f.mood === "speaking" && moving ? (Math.sin(f.t * 11) * 0.5 + Math.sin(f.t * 17.3) * 0.5) * 0.05 : 0;
  const R = (f.size / 2) * (1 + breathe + speak);
  ctx.clearRect(0, 0, box, box);

  // 1. Glow.
  const glowA = (f.lit ? 0.5 : 0.36) * (f.mood === "offline" ? 0.4 : 1);
  const glow = ctx.createRadialGradient(c, c, R * 0.7, c, c, Math.min(c, R * 1.5));
  glow.addColorStop(0, rgba(col.main, glowA));
  glow.addColorStop(1, rgba(col.main, 0));
  ctx.fillStyle = glow;
  ctx.beginPath();
  ctx.arc(c, c, Math.min(c, R * 1.5), 0, Math.PI * 2);
  ctx.fill();

  // 2. Approval pulse ring (behind the sphere).
  if (f.mood === "approval") {
    const k = moving ? (f.t * 0.9) % 1 : 0.35;
    ctx.strokeStyle = rgba(p.warning, 0.75 * (1 - k));
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(c, c, R * (1.04 + 0.32 * k), 0, Math.PI * 2);
    ctx.stroke();
  }
  // Speaking ripples.
  if (f.mood === "speaking") {
    for (let i = 0; i < 2; i += 1) {
      const k = moving ? (f.t * 0.7 + i * 0.5) % 1 : 0.3 + i * 0.3;
      ctx.strokeStyle = rgba(col.main, 0.55 * (1 - k));
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.arc(c, c, R * (1.02 + 0.36 * k), 0, Math.PI * 2);
      ctx.stroke();
    }
  }

  // 3. Core sphere.
  ctx.save();
  ctx.beginPath();
  ctx.arc(c, c, R, 0, Math.PI * 2);
  ctx.clip();
  const core = ctx.createRadialGradient(c - R * 0.35, c - R * 0.4, R * 0.05, c, c, R);
  core.addColorStop(0, rgba(mix(col.main, [255, 255, 255], 0.55), 1));
  core.addColorStop(0.42, rgba(col.main, 1));
  core.addColorStop(1, rgba(mix(col.main, p.bg, 0.78), 1));
  ctx.fillStyle = core;
  ctx.fillRect(c - R, c - R, R * 2, R * 2);
  // Plasma: drifting colour blobs.
  ctx.globalCompositeOperation = "lighter";
  const blobs: [Rgb, number, number][] = [
    [col.second, 0.0, 0.55],
    [col.third, 2.1, 0.42],
    [col.main, 4.2, 0.5],
  ];
  for (const [color, phase, speed] of blobs) {
    const a = phase + (moving ? f.t * speed * (f.mood === "working" ? 1.8 : 1) : 0);
    const bx = c + Math.cos(a) * R * 0.38;
    const by = c + Math.sin(a * 1.3) * R * 0.34;
    const g = ctx.createRadialGradient(bx, by, 0, bx, by, R * 0.7);
    g.addColorStop(0, rgba(color, 0.42));
    g.addColorStop(1, rgba(color, 0));
    ctx.fillStyle = g;
    ctx.fillRect(c - R, c - R, R * 2, R * 2);
  }
  ctx.globalCompositeOperation = "source-over";
  // State tint.
  if (f.mood === "error" || f.mood === "stopped") {
    ctx.fillStyle = rgba(p.danger, 0.22);
    ctx.fillRect(c - R, c - R, R * 2, R * 2);
  }
  // Inner shadow at the rim (depth).
  const rim = ctx.createRadialGradient(c, c, R * 0.72, c, c, R);
  rim.addColorStop(0, rgba(p.bg, 0));
  rim.addColorStop(1, rgba(p.bg, 0.55));
  ctx.fillStyle = rim;
  ctx.fillRect(c - R, c - R, R * 2, R * 2);
  // Specular highlight.
  const hx = c - R * 0.34;
  const hy = c - R * 0.42;
  const spec = ctx.createRadialGradient(hx, hy, 0, hx, hy, R * 0.45);
  spec.addColorStop(0, "rgba(255,255,255,0.55)");
  spec.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = spec;
  ctx.fillRect(c - R, c - R, R * 2, R * 2);
  ctx.restore();

  // Rim line.
  ctx.strokeStyle = rgba(mix(col.main, [255, 255, 255], 0.3), f.lit ? 0.8 : 0.55);
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.arc(c, c, R - 0.5, 0, Math.PI * 2);
  ctx.stroke();

  // 4. Working rings: tilted orbits with a bright travelling arc.
  if (f.mood === "working" || (f.mood === "approval" && f.rings > 0)) {
    const n = Math.max(1, Math.min(3, f.rings));
    for (let i = 0; i < n; i += 1) {
      const tilt = -0.5 + i * (Math.PI / n);
      const rx = R * (1.2 + i * 0.06);
      const ry = R * 0.36;
      ctx.save();
      ctx.translate(c, c);
      ctx.rotate(tilt);
      ctx.strokeStyle = rgba(col.second, 0.22);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.ellipse(0, 0, rx, ry, 0, 0, Math.PI * 2);
      ctx.stroke();
      const start = (moving ? f.t * (1.4 + i * 0.35) : i * 1.7) % (Math.PI * 2);
      ctx.strokeStyle = rgba(mix(col.main, [255, 255, 255], 0.35), 0.9);
      ctx.lineWidth = 1.6;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.ellipse(0, 0, rx, ry, 0, start, start + 0.9);
      ctx.stroke();
      ctx.restore();
    }
  }

  // 5. Listening: a waveform ring driven by the microphone level.
  if (f.mood === "listening") {
    const bars = 40;
    const lvl = Math.max(0.08, Math.min(1, f.level));
    ctx.strokeStyle = rgba(mix(col.main, [255, 255, 255], 0.25), 0.85);
    ctx.lineWidth = 1.6;
    ctx.lineCap = "round";
    for (let i = 0; i < bars; i += 1) {
      const a = (i / bars) * Math.PI * 2;
      const wave = moving ? (Math.sin(a * 3 + f.t * 6) + Math.sin(a * 5 - f.t * 4.3)) * 0.25 + 0.5 : 0.5;
      const len = 2 + lvl * f.pad * 0.9 * wave;
      const r0 = R + 2;
      ctx.beginPath();
      ctx.moveTo(c + Math.cos(a) * r0, c + Math.sin(a) * r0);
      ctx.lineTo(c + Math.cos(a) * (r0 + len), c + Math.sin(a) * (r0 + len));
      ctx.stroke();
    }
  }

  // 6. Particles (full motion only).
  if (full && f.mood !== "offline") {
    for (let i = 0; i < 9; i += 1) {
      const a = f.t * (0.35 + (i % 3) * 0.12) + i * 0.7;
      const r = R * (1.12 + ((i * 37) % 10) / 40);
      const x = c + Math.cos(a) * r;
      const y = c + Math.sin(a) * r * 0.85;
      ctx.fillStyle = rgba(mix(col.second, [255, 255, 255], 0.4), 0.35 + 0.35 * Math.sin(f.t * 2 + i));
      ctx.beginPath();
      ctx.arc(x, y, 0.9 + (i % 2) * 0.5, 0, Math.PI * 2);
      ctx.fill();
    }
  }
}

export function OrbSphere(props: OrbSphereProps) {
  const ref = useRef<HTMLCanvasElement | null>(null);
  const paletteRef = useRef<Palette>(FALLBACK);
  const propsRef = useRef(props);
  const t0Ref = useRef(typeof performance !== "undefined" ? performance.now() : 0);
  propsRef.current = props;
  const box = props.size + props.pad * 2;

  // Theme colours.
  useEffect(() => {
    const ctx = ref.current?.getContext("2d");
    if (ctx) paletteRef.current = readPalette(ctx, props.accent);
  }, [props.themeKey, props.accent]);

  // Render loop (or one still frame).
  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return undefined;
    const dpr = Math.max(1, Math.min(3, window.devicePixelRatio || 1));
    canvas.width = Math.round(box * dpr);
    canvas.height = Math.round(box * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const frame = (t: number) => {
      const p = propsRef.current;
      drawOrb(ctx, paletteRef.current, { t, size: p.size, pad: p.pad, mood: p.mood, motion: p.motion, rings: p.rings, level: p.level, lit: p.lit });
    };
    if (props.fps <= 0) {
      frame(0);
      return undefined;
    }
    let raf = 0;
    let last = 0;
    const t0 = t0Ref.current;
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (now - last < 1000 / props.fps - 1) return;
      last = now;
      frame((now - t0) / 1000);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [box, props.fps, props.mood, props.motion, props.rings, props.lit, props.themeKey, props.accent, props.size, props.pad]);

  return <canvas ref={ref} className="orb-sphere" style={{ width: box, height: box }} aria-hidden="true" />;
}
