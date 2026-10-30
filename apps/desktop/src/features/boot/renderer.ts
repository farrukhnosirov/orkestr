/**
 * Startup animation renderer (Canvas 2D, one full-window canvas; ADR-0053).
 *
 * Draws the cinematic background (theme background, vignette, a revealed
 * grid, drifting particles, HUD corner brackets and rulers) and the Jarvis
 * reactor (same visual language as `voice/jarvis/renderer.ts`) assembling
 * itself: rings draw in (stroke-dash style with a bright head), the outer
 * ticks light up clockwise, the coils fill and the core ignites with a bloom
 * and a shock wave. On exit the reactor shrinks and flies to the title-bar
 * logo while the background fades and reveals the app.
 *
 * Glows are pre-rendered sprites and the grid is cached per size, so a frame
 * is a few hundred cheap draw calls. Colours come from the theme tokens
 * (`readPalette`); light themes draw with normal blending.
 */
import { readPalette, type JarvisPalette, type Rgb } from "../voice/jarvis/renderer";
import type { BootFrameState, BootVariant } from "./sequence";
import { ease, span } from "./sequence";

const TAU = Math.PI * 2;
const rgba = (c: Rgb, a: number) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${Math.max(0, Math.min(1, a)).toFixed(3)})`;
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const WHITE: Rgb = [255, 255, 255];
const BLACK: Rgb = [0, 0, 0];

export { readPalette };

export interface BootLayout {
  w: number;
  h: number;
  cx: number;
  cy: number;
  /** Reactor radius (ring system), px. */
  R: number;
  compact: boolean;
}

/** Reactor placement for a window size (compact: Assistant mode or a small window). */
export function layoutFor(w: number, h: number, compact: boolean): BootLayout {
  const small = compact || w < 640 || h < 460;
  const R = Math.max(40, Math.min(small ? 120 : 175, Math.min(w, h * 1.2) * (small ? 0.2 : 0.165)));
  return { w, h, cx: w / 2, cy: Math.round(h * (small ? 0.4 : 0.43)), R, compact: small };
}

/** Where the reactor flies to on exit (the title-bar logo), viewport px. */
export interface FlyTarget {
  x: number;
  y: number;
  size: number;
}

/**
 * Times (s) of the assembly for a version: the full script is authored for
 * the first-launch version and compressed for the short one; the static
 * version is drawn fully assembled.
 */
export function assemblyScale(v: Exclude<BootVariant, "none">): number {
  return v === "full" ? 1 : v === "short" ? 0.3 : 0;
}

/** Ignition time (s) of the core for a version. */
export function igniteAt(v: Exclude<BootVariant, "none">): number {
  return 2.0 * assemblyScale(v);
}

export interface BootDrawInput {
  s: BootFrameState;
  variant: Exclude<BootVariant, "none">;
  /** Real seconds since the sequence started (spins, particles). */
  now: number;
  target: FlyTarget | null;
}

interface Dot {
  x: number;
  y: number;
  vx: number;
  vy: number;
  r: number;
  ph: number;
}

function glowSprite(color: Rgb, px: number, hot: boolean): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = c.height = px;
  const g = c.getContext("2d")!;
  const r = px / 2;
  const grad = g.createRadialGradient(r, r, 0, r, r, r);
  if (hot) grad.addColorStop(0, "rgba(255,255,255,0.95)");
  grad.addColorStop(hot ? 0.12 : 0, rgba(color, hot ? 0.9 : 0.55));
  grad.addColorStop(0.45, rgba(color, 0.2));
  grad.addColorStop(1, rgba(color, 0));
  g.fillStyle = grad;
  g.fillRect(0, 0, px, px);
  return c;
}

/** Deterministic pseudo-random (stable particles between frames and captures). */
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 0xffffffff;
  };
}

export class BootRenderer {
  private ctx: CanvasRenderingContext2D;
  private dpr = 1;
  private L: BootLayout = { w: 0, h: 0, cx: 0, cy: 0, R: 0, compact: false };
  private pal: JarvisPalette;
  private grid: HTMLCanvasElement | null = null;
  private sprites: { glow: HTMLCanvasElement; core: HTMLCanvasElement } | null = null;
  private dots: Dot[] = [];

  constructor(private readonly canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) throw new Error("no 2d context");
    this.ctx = ctx;
    this.pal = readPalette(null);
  }

  get layout(): BootLayout {
    return this.L;
  }

  resize(w: number, h: number, dpr: number, compact: boolean): BootLayout {
    this.dpr = Math.min(2, Math.max(1, dpr));
    this.canvas.width = Math.max(1, Math.round(w * this.dpr));
    this.canvas.height = Math.max(1, Math.round(h * this.dpr));
    this.L = layoutFor(w, h, compact);
    this.grid = null;
    this.sprites = null;
    const r = rng(7);
    const n = Math.round(Math.min(90, (w * h) / 16000));
    this.dots = Array.from({ length: n }, () => ({
      x: r() * w,
      y: r() * h,
      vx: (r() - 0.5) * 6,
      vy: -2 - r() * 7,
      r: 0.5 + r() * 1.3,
      ph: r() * TAU,
    }));
    return this.L;
  }

  setPalette(p: JarvisPalette): void {
    if (JSON.stringify(p) === JSON.stringify(this.pal) && this.grid) return;
    this.pal = p;
    this.grid = null;
    this.sprites = null;
  }

  private ensureSprites() {
    if (this.sprites) return this.sprites;
    const px = Math.max(96, Math.round(this.L.R * 2.4 * this.dpr));
    const { pal } = this;
    this.sprites = {
      glow: glowSprite(pal.accent, px, false),
      core: glowSprite(mix(pal.accentStrong, WHITE, pal.light ? 0 : 0.25), px, !pal.light),
    };
    return this.sprites;
  }

  /** The grid (masked to fade towards the edges), cached per size and palette. */
  private ensureGrid(): HTMLCanvasElement {
    if (this.grid) return this.grid;
    const { w, h, cx, cy } = this.L;
    const c = document.createElement("canvas");
    c.width = Math.round(w * this.dpr);
    c.height = Math.round(h * this.dpr);
    const g = c.getContext("2d")!;
    g.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    const step = this.L.compact ? 28 : 44;
    const col = this.pal.light ? mix(this.pal.accent, BLACK, 0.2) : this.pal.accent;
    g.strokeStyle = rgba(col, this.pal.light ? 0.1 : 0.085);
    g.lineWidth = 1;
    g.beginPath();
    for (let x = (cx % step) - step; x <= w + step; x += step) {
      g.moveTo(Math.round(x) + 0.5, 0);
      g.lineTo(Math.round(x) + 0.5, h);
    }
    for (let y = (cy % step) - step; y <= h + step; y += step) {
      g.moveTo(0, Math.round(y) + 0.5);
      g.lineTo(w, Math.round(y) + 0.5);
    }
    g.stroke();
    // Crosses on every 4th intersection.
    g.strokeStyle = rgba(col, this.pal.light ? 0.22 : 0.2);
    g.beginPath();
    for (let x = (cx % (step * 4)) - step * 4; x <= w + step; x += step * 4) {
      for (let y = (cy % (step * 4)) - step * 4; y <= h + step; y += step * 4) {
        g.moveTo(x - 3, y + 0.5);
        g.lineTo(x + 4, y + 0.5);
        g.moveTo(x + 0.5, y - 3);
        g.lineTo(x + 0.5, y + 4);
      }
    }
    g.stroke();
    // Radial mask: strongest around the reactor, gone at the edges.
    g.globalCompositeOperation = "destination-in";
    const rad = Math.hypot(w, h) * 0.55;
    const m = g.createRadialGradient(cx, cy, 0, cx, cy, rad);
    m.addColorStop(0, "rgba(0,0,0,1)");
    m.addColorStop(0.55, "rgba(0,0,0,0.55)");
    m.addColorStop(1, "rgba(0,0,0,0)");
    g.fillStyle = m;
    g.fillRect(0, 0, w, h);
    this.grid = c;
    return c;
  }

  draw(input: BootDrawInput): void {
    const { ctx, L, pal } = this;
    const { s, variant, now } = input;
    if (L.w <= 0) return;
    const k = assemblyScale(variant);
    const T = (x: number) => x * k; // script time → this version
    const t = s.t;
    const exitE = s.phase === "exit" || s.phase === "done" ? ease(s.exitP) : 0;
    const delay = s.fly ? 0.2 : 0;
    const bgAlpha = 1 - ease((s.exitP - delay) / (1 - delay));
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    ctx.clearRect(0, 0, L.w, L.h);

    // ── background ──────────────────────────────────────────────────────
    const base = pal.light ? pal.bg : mix(pal.bg, BLACK, 0.45);
    ctx.globalAlpha = bgAlpha;
    ctx.fillStyle = rgba(base, 1);
    ctx.fillRect(0, 0, L.w, L.h);
    const vig = ctx.createRadialGradient(L.cx, L.cy, 0, L.cx, L.cy, Math.hypot(L.w, L.h) * 0.6);
    vig.addColorStop(0, rgba(pal.accent, pal.light ? 0.1 : 0.12));
    vig.addColorStop(0.45, rgba(pal.accent, pal.light ? 0.03 : 0.035));
    vig.addColorStop(1, rgba(pal.light ? mix(pal.bg, BLACK, 0.06) : BLACK, pal.light ? 0.5 : 0.55));
    ctx.fillStyle = vig;
    ctx.fillRect(0, 0, L.w, L.h);

    // Grid revealed by an expanding ring from the centre.
    const reveal = k === 0 ? 1 : ease(span(t, T(0.1), T(1.3) + 0.15));
    if (reveal > 0) {
      const grid = this.ensureGrid();
      const rr = Math.hypot(L.w, L.h) * 0.62 * reveal;
      ctx.save();
      ctx.beginPath();
      ctx.arc(L.cx, L.cy, Math.max(1, rr), 0, TAU);
      ctx.clip();
      ctx.globalAlpha = bgAlpha * (0.35 + 0.65 * reveal);
      ctx.drawImage(grid, 0, 0, L.w, L.h);
      ctx.restore();
      if (reveal < 1) {
        ctx.globalAlpha = bgAlpha * (1 - reveal) * 0.6;
        ctx.strokeStyle = rgba(pal.accentStrong, 1);
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(L.cx, L.cy, rr, 0, TAU);
        ctx.stroke();
      }
    }

    // Particles.
    const dotA = bgAlpha * (k === 0 ? 0.6 : ease(span(t, T(0.2), T(1.0) + 0.2)));
    if (dotA > 0.01) {
      ctx.globalCompositeOperation = pal.light ? "source-over" : "lighter";
      const col = pal.light ? mix(pal.accent, BLACK, 0.15) : pal.accentStrong;
      for (const d of this.dots) {
        const x = (((d.x + d.vx * now) % L.w) + L.w) % L.w;
        const y = (((d.y + d.vy * now) % L.h) + L.h) % L.h;
        const tw = 0.35 + 0.65 * (0.5 + 0.5 * Math.sin(now * 1.3 + d.ph));
        ctx.globalAlpha = dotA * tw * (pal.light ? 0.45 : 0.55);
        ctx.fillStyle = rgba(col, 1);
        ctx.beginPath();
        ctx.arc(x, y, d.r, 0, TAU);
        ctx.fill();
      }
      ctx.globalCompositeOperation = "source-over";
    }

    // HUD corner brackets + rulers.
    const hudA = bgAlpha * (k === 0 ? 1 : ease(span(t, T(0.25), T(0.6) + 0.15)));
    if (hudA > 0.01) this.drawHudFrame(hudA, now);

    // Ignition flash over the whole window.
    const ign = igniteAt(variant);
    const flash = k === 0 ? 0 : Math.max(0, 1 - Math.max(0, t - ign) / 0.7) * (t >= ign ? 1 : 0);
    if (flash > 0.01) {
      ctx.globalAlpha = bgAlpha * flash * (pal.light ? 0.12 : 0.1);
      ctx.fillStyle = rgba(pal.accentStrong, 1);
      ctx.fillRect(0, 0, L.w, L.h);
    }
    ctx.globalAlpha = 1;

    // ── reactor ────────────────────────────────────────────────────────
    let cx = L.cx;
    let cy = L.cy;
    let R = L.R;
    let alpha = 1;
    if (exitE > 0) {
      if (s.fly && input.target) {
        const p = ease(s.exitP * 1.05);
        cx += (input.target.x - cx) * p;
        cy += (input.target.y - cy) * p;
        R += (input.target.size * 0.5 - R) * p;
        alpha = 1 - ease((s.exitP - 0.7) / 0.3);
      } else {
        R *= 1 + exitE * 0.08;
        alpha = 1 - exitE;
      }
    }
    if (alpha > 0.01) this.drawReactor(cx, cy, R, alpha, input, flash);
  }

  private drawHudFrame(a: number, now: number): void {
    const { ctx, L, pal } = this;
    const col = pal.light ? mix(pal.accent, BLACK, 0.25) : pal.accentStrong;
    const inset = L.compact ? 10 : 18;
    const len = L.compact ? 16 : 30;
    ctx.strokeStyle = rgba(col, a * 0.55);
    ctx.lineWidth = 1.2;
    ctx.beginPath();
    for (const [x, y, sx, sy] of [
      [inset, inset + (L.compact ? 0 : 20), 1, 1],
      [L.w - inset, inset + (L.compact ? 0 : 20), -1, 1],
      [inset, L.h - inset, 1, -1],
      [L.w - inset, L.h - inset, -1, -1],
    ] as const) {
      ctx.moveTo(x, y + sy * len);
      ctx.lineTo(x, y);
      ctx.lineTo(x + sx * len, y);
    }
    ctx.stroke();
    if (L.compact) return;
    // Horizontal rulers left and right of the reactor with moving ticks.
    const { cx, cy, R } = L;
    const r0 = R * 1.18;
    const r1 = Math.min(R * 2.4, L.w / 2 - 40);
    if (r1 <= r0 + 20) return;
    for (const dir of [-1, 1]) {
      const grad = ctx.createLinearGradient(cx + dir * r0, cy, cx + dir * r1, cy);
      grad.addColorStop(0, rgba(col, a * 0.5));
      grad.addColorStop(1, rgba(col, 0));
      ctx.strokeStyle = grad;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(cx + dir * r0, cy + 0.5);
      ctx.lineTo(cx + dir * r1, cy + 0.5);
      ctx.stroke();
      const stepPx = 12;
      const off = (now * 18) % stepPx;
      ctx.beginPath();
      for (let d = r0 + off; d < r1; d += stepPx) {
        const h = Math.round((d - r0) / stepPx) % 4 === 0 ? 5 : 2.5;
        ctx.moveTo(cx + dir * d + 0.5, cy - h);
        ctx.lineTo(cx + dir * d + 0.5, cy + h);
      }
      ctx.stroke();
    }
  }

  private drawReactor(cx: number, cy: number, R: number, alpha: number, input: BootDrawInput, flash: number): void {
    const { ctx, pal } = this;
    const { s, variant, now } = input;
    const k = assemblyScale(variant);
    const T = (x: number) => x * k;
    const t = s.t;
    const P = (start: number, dur: number) => (k === 0 ? 1 : ease(span(t, T(start), T(dur))));
    const light = pal.light;
    const add = light ? "source-over" : "lighter";
    const main = pal.accent;
    const strong = pal.accentStrong;
    const second = pal.violet;
    const A = (x: number) => x * alpha * (light ? 1.15 : 1);
    const sp = this.ensureSprites();
    const small = R < 50;

    // Spin: rings accelerate from rest once the assembly starts.
    const spinStart = T(0.5);
    const sx = Math.max(0, (k === 0 ? now + 2 : now) - spinStart);
    const ramp = 1.6;
    const spin = (speed: number) => speed * (sx < ramp ? (sx * sx) / (2 * ramp) : sx - ramp / 2);
    const rot = [spin(0.12), spin(-0.55), spin(0.95), spin(-1.6)] as const;

    // Lens backdrop.
    const lensA = P(0.15, 0.6);
    ctx.globalCompositeOperation = "source-over";
    const lens = ctx.createRadialGradient(cx, cy, R * 0.1, cx, cy, R * 1.02);
    lens.addColorStop(0, rgba(pal.bg, (light ? 0.8 : 0.85) * lensA * alpha));
    lens.addColorStop(0.75, rgba(pal.bg, (light ? 0.6 : 0.7) * lensA * alpha));
    lens.addColorStop(1, rgba(pal.bg, 0));
    ctx.fillStyle = lens;
    ctx.beginPath();
    ctx.arc(cx, cy, R * 1.02, 0, TAU);
    ctx.fill();

    ctx.globalCompositeOperation = add;
    // Outer bloom (grows with the ignition).
    const lit = P(1.9, 0.5);
    const bloom = R * (1.25 + flash * 0.5);
    ctx.globalAlpha = A(0.12 + lit * 0.3 + flash * 0.35);
    ctx.drawImage(sp.glow, cx - bloom, cy - bloom, bloom * 2, bloom * 2);
    ctx.globalAlpha = 1;

    // Faint guide circle (drawn in first).
    const guide = P(0.2, 0.8);
    if (guide > 0) {
      ctx.strokeStyle = rgba(main, A(0.28));
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(cx, cy, R * 0.98, -Math.PI / 2, -Math.PI / 2 + TAU * guide);
      ctx.stroke();
      if (guide < 1) this.spark(cx + Math.cos(-Math.PI / 2 + TAU * guide) * R * 0.98, cy + Math.sin(-Math.PI / 2 + TAU * guide) * R * 0.98, R, alpha);
    }

    // Outer tick ring: ticks light up clockwise from the top.
    const tickP = k === 0 ? 1 : span(t, T(0.45), T(1.1));
    const tickR = R * 0.93;
    const ticks = small ? 48 : 120;
    ctx.lineCap = "butt";
    for (let i = 0; i < ticks; i++) {
      const pos = i / ticks;
      const major = small ? i % 8 === 0 : i % 10 === 0;
      const mid = small ? i % 4 === 0 : i % 5 === 0;
      const len = major ? R * 0.07 : mid ? R * 0.045 : R * 0.025;
      const a = -Math.PI / 2 + pos * TAU + rot[0];
      const on = tickP >= 1 ? 1 : Math.max(0, Math.min(1, (tickP - pos) * 14));
      const head = tickP < 1 ? Math.max(0, 1 - Math.abs(tickP - pos) * 30) : 0;
      const base = major ? 0.85 : mid ? 0.5 : 0.28;
      const alphaTick = guide > 0 ? 0.06 + on * base + head * 0.6 : 0;
      if (alphaTick <= 0.005) continue;
      ctx.strokeStyle = rgba(head > 0.2 ? mix(strong, WHITE, light ? 0 : 0.5) : major ? strong : main, A(alphaTick));
      ctx.lineWidth = major ? 1.6 : 1;
      const cs = Math.cos(a);
      const sn = Math.sin(a);
      ctx.beginPath();
      ctx.moveTo(cx + cs * tickR, cy + sn * tickR);
      ctx.lineTo(cx + cs * (tickR - len - head * R * 0.03), cy + sn * (tickR - len - head * R * 0.03));
      ctx.stroke();
    }
    // HUD brackets.
    const br = P(1.3, 0.3);
    if (br > 0) {
      ctx.strokeStyle = rgba(strong, A(0.9 * br));
      ctx.lineWidth = 2;
      for (const b of [0, Math.PI]) {
        ctx.beginPath();
        ctx.arc(cx, cy, tickR + 2, b + rot[0] - 0.32 * br, b + rot[0] + 0.32 * br);
        ctx.stroke();
      }
    }

    // Radial spectrum: grows out with a travelling wave.
    const specP = P(1.4, 0.6);
    if (specP > 0) {
      const inner = R * 0.64;
      const bars = small ? 32 : 64;
      ctx.lineCap = "round";
      ctx.lineWidth = Math.max(1.4, (TAU * inner) / bars / 2.1);
      for (let i = 0; i < bars; i++) {
        const half = i < bars / 2 ? i : bars - 1 - i;
        const v = Math.max(0, 0.18 + 0.22 * Math.sin(half * 0.55 + now * 2.6) * Math.sin(half * 0.21 - now * 1.4) + (s.phase === "holding" ? 0.12 * Math.sin(now * 4 + half) : 0));
        const a = -Math.PI / 2 + ((i + 0.5) / bars) * TAU;
        const len = (R * 0.025 + v * R * 0.2) * specP;
        const cs = Math.cos(a);
        const sn = Math.sin(a);
        ctx.strokeStyle = rgba(mix(main, strong, v), A((0.3 + v * 0.6) * specP));
        ctx.beginPath();
        ctx.moveTo(cx + cs * inner, cy + sn * inner);
        ctx.lineTo(cx + cs * (inner + len), cy + sn * (inner + len));
        ctx.stroke();
      }
    }

    // Segmented arcs (4): drawn in one after another.
    ctx.lineCap = "butt";
    const segR = R * 0.585;
    ctx.lineWidth = Math.max(2, R * 0.035);
    for (let i = 0; i < 4; i++) {
      const p = P(0.6 + i * 0.1, 0.45);
      if (p <= 0) continue;
      const a0 = rot[1] + (i / 4) * TAU + 0.12 - Math.PI / 2;
      const a1 = a0 + (TAU / 4 - 0.38) * p;
      ctx.strokeStyle = rgba(i % 2 ? second : main, A(0.78));
      ctx.beginPath();
      ctx.arc(cx, cy, segR, a0, a1);
      ctx.stroke();
      if (p < 1) this.spark(cx + Math.cos(a1) * segR, cy + Math.sin(a1) * segR, R, alpha);
    }
    const thin = P(0.5, 0.7);
    if (thin > 0) {
      ctx.lineWidth = 1;
      ctx.strokeStyle = rgba(main, A(0.32));
      ctx.beginPath();
      ctx.arc(cx, cy, segR - R * 0.04, Math.PI / 2, Math.PI / 2 - TAU * thin, true);
      ctx.stroke();
    }

    // Dashed ring.
    const dash = P(0.8, 0.7);
    if (dash > 0) {
      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(rot[2]);
      ctx.setLineDash([R * 0.05, R * 0.035]);
      ctx.lineWidth = 1.4;
      ctx.strokeStyle = rgba(strong, A(0.55));
      ctx.beginPath();
      ctx.arc(0, 0, R * 0.5, 0, TAU * dash);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.restore();
    }

    // Chaser ring: segments appear one by one, then a bright head runs around.
    const chP = k === 0 ? 1 : span(t, T(1.0), T(0.7));
    if (chP > 0) {
      const chR = R * 0.43;
      const headA = ((rot[3] % TAU) + TAU) % TAU;
      ctx.lineWidth = Math.max(2, R * 0.03);
      for (let i = 0; i < 18; i++) {
        const on = Math.max(0, Math.min(1, (chP - i / 18) * 18));
        if (on <= 0) continue;
        const a0 = (i / 18) * TAU - Math.PI / 2;
        let d = (headA - a0) % TAU;
        if (d < 0) d += TAU;
        const g = chP >= 1 ? Math.max(0, 1 - d / (TAU * 0.4)) : 0.4;
        ctx.strokeStyle = rgba(mix(main, strong, g), A((0.22 + g * 0.78) * on));
        ctx.beginPath();
        ctx.arc(cx, cy, chR, a0 + 0.04, a0 + TAU / 18 - 0.06);
        ctx.stroke();
      }
    }

    // Holding: scanning sweep («waiting for systems»).
    const sweepA = s.phase === "holding" ? Math.min(1, s.hold * 2) : 0;
    if (sweepA > 0.01) {
      const sw = now * 2.4;
      const grad = ctx.createConicGradient(sw, cx, cy);
      grad.addColorStop(0, rgba(strong, 0.5 * sweepA * alpha));
      grad.addColorStop(0.14, rgba(main, 0));
      grad.addColorStop(1, rgba(main, 0));
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.arc(cx, cy, R * 0.9, 0, TAU);
      ctx.arc(cx, cy, R * 0.36, 0, TAU, true);
      ctx.fill();
    }

    // Core: ring + coils fill one by one, then ignition.
    const coreRing = P(1.45, 0.35);
    const coreR = R * 0.3;
    if (coreRing > 0) {
      ctx.lineWidth = Math.max(1.2, R * 0.018);
      ctx.strokeStyle = rgba(strong, A(0.85));
      ctx.beginPath();
      ctx.arc(cx, cy, coreR, -Math.PI / 2, -Math.PI / 2 + TAU * coreRing);
      ctx.stroke();
    }
    const coils = small ? 8 : 10;
    const coilP = k === 0 ? 1 : span(t, T(1.55), T(0.4));
    for (let i = 0; i < coils; i++) {
      const on = Math.max(0, Math.min(1, (coilP - i / coils) * coils));
      if (on <= 0) continue;
      const a0 = -rot[1] * 0.3 + (i / coils) * TAU - Math.PI / 2;
      ctx.fillStyle = rgba(i % 2 ? main : strong, A((0.5 + lit * 0.15) * on));
      ctx.beginPath();
      ctx.arc(cx, cy, coreR * 0.9, a0 + 0.06, a0 + TAU / coils - 0.08);
      ctx.arc(cx, cy, coreR * 0.66, a0 + TAU / coils - 0.12, a0 + 0.1, true);
      ctx.closePath();
      ctx.fill();
    }
    if (coreRing > 0) {
      ctx.lineWidth = 1;
      ctx.strokeStyle = rgba(main, A(0.6 * coreRing));
      ctx.beginPath();
      ctx.arc(cx, cy, coreR * 0.58, 0, TAU);
      ctx.stroke();
    }
    // Ignition: hot glow, shock wave, centre lens.
    const breathe = 0.04 * Math.sin(now * 2.2);
    if (lit > 0) {
      const g = coreR * (1.2 + breathe + flash * 1.6);
      ctx.globalAlpha = A(Math.min(1, 0.8 * lit + flash * 0.4));
      ctx.drawImage(sp.core, cx - g, cy - g, g * 2, g * 2);
      ctx.globalAlpha = 1;
    }
    const ign = igniteAt(variant);
    if (k > 0 && t >= ign && t < ign + 0.9) {
      const p = (t - ign) / 0.9;
      ctx.strokeStyle = rgba(strong, (1 - p) * 0.9 * alpha);
      ctx.lineWidth = 1 + (1 - p) * 4;
      ctx.beginPath();
      ctx.arc(cx, cy, coreR + p * R * 1.4, 0, TAU);
      ctx.stroke();
      const p2 = Math.max(0, (t - ign - 0.15) / 0.75);
      if (p2 > 0 && p2 < 1) {
        ctx.strokeStyle = rgba(main, (1 - p2) * 0.5 * alpha);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(cx, cy, coreR + p2 * R * 2.1, 0, TAU);
        ctx.stroke();
      }
    }
    ctx.globalCompositeOperation = "source-over";
    if (lit > 0) {
      ctx.fillStyle = rgba(light ? strong : mix(strong, WHITE, 0.65), A(0.95 * lit));
      ctx.beginPath();
      ctx.arc(cx, cy, coreR * (0.24 + breathe * 0.5 + flash * 0.08), 0, TAU);
      ctx.fill();
    }
  }

  /** Bright head of a ring being drawn in. */
  private spark(x: number, y: number, R: number, alpha: number): void {
    const { ctx, pal } = this;
    const prev = ctx.globalCompositeOperation;
    ctx.globalCompositeOperation = pal.light ? "source-over" : "lighter";
    const sp = this.ensureSprites();
    const g = Math.max(6, R * 0.09);
    ctx.globalAlpha = alpha * 0.9;
    ctx.drawImage(sp.core, x - g, y - g, g * 2, g * 2);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = prev;
  }
}
