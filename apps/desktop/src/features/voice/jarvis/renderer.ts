/**
 * Jarvis reactor renderer (Canvas 2D, GPU-composited in Chromium): a lens
 * backdrop, an outer tick ring, a radial spectrum around the circle,
 * segmented / dashed rings turning at different speeds, a chaser ring, the
 * arc-reactor core (coils + glow), a scanning sweep while thinking, ripples
 * while speaking, drifting particles and a bloom flash on wake. Glows are
 * pre-rendered sprites (no `shadowBlur`), so a 150 px frame costs ~200 draw
 * calls. Colours come from the theme tokens; light themes draw with normal
 * blending and deeper colours, dark themes add light.
 */
import { BANDS, type ReactorFrame } from "./model";

export type Rgb = readonly [number, number, number];

export interface JarvisPalette {
  accent: Rgb;
  accentStrong: Rgb;
  violet: Rgb;
  danger: Rgb;
  warning: Rgb;
  muted: Rgb;
  bg: Rgb;
  text: Rgb;
  light: boolean;
}

const FALLBACK: JarvisPalette = {
  accent: [56, 189, 248],
  accentStrong: [125, 211, 252],
  violet: [139, 92, 246],
  danger: [239, 68, 68],
  warning: [245, 158, 11],
  muted: [100, 116, 139],
  bg: [8, 12, 24],
  text: [226, 232, 240],
  light: false,
};

function parseRgb(value: string): Rgb | null {
  const v = value.trim();
  const hex = /^#([0-9a-f]{6})(?:[0-9a-f]{2})?$/i.exec(v);
  if (hex?.[1]) {
    const n = parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const rgb = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(v);
  if (rgb?.[1] && rgb[2] && rgb[3]) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}

/** Reads the palette from the theme tokens (`--ok-color-*`), normalized by a probe canvas. */
export function readPalette(accentOverride: string | null): JarvisPalette {
  if (typeof document === "undefined") return FALLBACK;
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) return FALLBACK;
  const css = getComputedStyle(document.documentElement);
  const norm = (raw: string, fallback: Rgb): Rgb => {
    if (!raw) return fallback;
    probe.fillStyle = "#000000";
    probe.fillStyle = raw;
    return parseRgb(String(probe.fillStyle)) ?? fallback;
  };
  const read = (name: string, fallback: Rgb) => norm(css.getPropertyValue(`--ok-color-${name}`).trim(), fallback);
  const bg = read("bg", FALLBACK.bg);
  const lum = (0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2]) / 255;
  return {
    accent: accentOverride ? norm(accentOverride, read("accent", FALLBACK.accent)) : read("accent", FALLBACK.accent),
    accentStrong: read("accent-strong", FALLBACK.accentStrong),
    violet: read("violet", FALLBACK.violet),
    danger: read("danger", FALLBACK.danger),
    warning: read("warning", FALLBACK.warning),
    muted: read("text-muted", FALLBACK.muted),
    bg,
    text: read("text", FALLBACK.text),
    light: lum > 0.55,
  };
}

const TAU = Math.PI * 2;
const rgba = (c: Rgb, a: number) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${Math.max(0, Math.min(1, a)).toFixed(3)})`;
function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

interface Particle {
  a: number;
  r: number;
  v: number;
  life: number;
  size: number;
}

/** Glow sprite: a soft radial disc of `color` (cached per colour). */
function glowSprite(color: Rgb, px: number, hot: boolean): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = c.height = px;
  const g = c.getContext("2d")!;
  const r = px / 2;
  const grad = g.createRadialGradient(r, r, 0, r, r, r);
  if (hot) grad.addColorStop(0, "rgba(255,255,255,0.95)");
  grad.addColorStop(hot ? 0.12 : 0, rgba(color, hot ? 0.9 : 0.55));
  grad.addColorStop(0.45, rgba(color, 0.22));
  grad.addColorStop(1, rgba(color, 0));
  g.fillStyle = grad;
  g.fillRect(0, 0, px, px);
  return c;
}

export class ReactorRenderer {
  private ctx: CanvasRenderingContext2D;
  private size = 0;
  private dpr = 1;
  private pal: JarvisPalette = FALLBACK;
  private sprites: { glow: HTMLCanvasElement; core: HTMLCanvasElement; alarm: HTMLCanvasElement } | null = null;
  private particles: Particle[] = [];
  private lastT = 0;

  constructor(private readonly canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) throw new Error("no 2d context");
    this.ctx = ctx;
  }

  resize(size: number, dpr: number): void {
    this.size = size;
    this.dpr = Math.min(2, Math.max(1, dpr));
    this.canvas.width = Math.round(size * this.dpr);
    this.canvas.height = Math.round(size * this.dpr);
    this.sprites = null;
  }

  setPalette(p: JarvisPalette): void {
    this.pal = p;
    this.sprites = null;
  }

  private ensureSprites() {
    if (this.sprites) return this.sprites;
    const px = Math.max(64, Math.round(this.size * this.dpr));
    this.sprites = {
      glow: glowSprite(this.pal.accent, px, false),
      core: glowSprite(mix(this.pal.accentStrong, [255, 255, 255], this.pal.light ? 0 : 0.25), px, !this.pal.light),
      alarm: glowSprite(this.pal.danger, px, false),
    };
    return this.sprites;
  }

  /** Draws one frame. */
  draw(f: ReactorFrame): void {
    const { ctx, size, pal } = this;
    if (size <= 0) return;
    const sp = this.ensureSprites();
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, size, size);
    if (f.energy <= 0.002 && f.intro >= 1 && f.state === "idle") return;

    const c = size / 2;
    // The ring system uses 82 % of the radius; the rest is room for the bloom (never clipped square).
    const R = (size / 2) * 0.82 * f.scale;
    const e = Math.max(0, Math.min(1, f.energy));
    const light = pal.light;
    const grey = (c: Rgb) => mix(c, pal.muted, f.muted * 0.85);
    const main = grey(mix(mix(pal.accent, pal.danger, f.alarm), pal.violet, f.state === "thinking" ? 0.35 : 0));
    const strong = grey(mix(pal.accentStrong, pal.danger, f.alarm));
    const second = grey(mix(pal.violet, pal.danger, f.alarm * 0.7));
    const small = R < 50;
    const add = light ? "source-over" : "lighter";
    const A = (a: number) => a * e * (light ? 1.15 : 1);

    // Lens backdrop: keeps the reactor legible over any desktop.
    ctx.globalCompositeOperation = "source-over";
    const lens = ctx.createRadialGradient(c, c, R * 0.1, c, c, R);
    lens.addColorStop(0, rgba(pal.bg, (light ? 0.86 : 0.92) * Math.min(1, e * 1.4)));
    lens.addColorStop(0.72, rgba(pal.bg, (light ? 0.74 : 0.82) * Math.min(1, e * 1.4)));
    lens.addColorStop(1, rgba(pal.bg, 0));
    ctx.fillStyle = lens;
    ctx.beginPath();
    ctx.arc(c, c, R, 0, TAU);
    ctx.fill();

    ctx.globalCompositeOperation = add;
    // Outer bloom.
    const bloom = (size / 2) * Math.min(1, 0.86 + f.level * 0.14) * Math.min(1, f.scale + 0.1);
    ctx.globalAlpha = A(0.35 + f.level * 0.35);
    ctx.drawImage(f.alarm > 0.5 ? sp.alarm : sp.glow, c - bloom, c - bloom, bloom * 2, bloom * 2);
    ctx.globalAlpha = 1;

    // Wake flash: an expanding ring.
    if (f.intro < 1) {
      const p = f.intro;
      ctx.strokeStyle = rgba(strong, (1 - p) * 0.9);
      ctx.lineWidth = 2 + (1 - p) * 4;
      ctx.beginPath();
      ctx.arc(c, c, R * (0.3 + p * 0.85), 0, TAU);
      ctx.stroke();
    }

    // Interrupted (barge-in): a ring collapsing inwards.
    if (f.interrupt > 0.01) {
      ctx.strokeStyle = rgba(strong, f.interrupt * 0.85);
      ctx.lineWidth = 1.5 + f.interrupt * 3;
      ctx.beginPath();
      ctx.arc(c, c, R * (1.05 - 0.25 * f.interrupt), 0, TAU);
      ctx.stroke();
    }
    // Follow-up window: a thin ring counting down clockwise from the top.
    if (f.countdown >= 0) {
      const r = R * 0.985;
      ctx.lineWidth = small ? 1.5 : 2;
      ctx.strokeStyle = rgba(main, A(0.16));
      ctx.beginPath();
      ctx.arc(c, c, r, 0, TAU);
      ctx.stroke();
      if (f.countdown > 0) {
        ctx.strokeStyle = rgba(strong, A(0.75));
        ctx.beginPath();
        ctx.arc(c, c, r, -Math.PI / 2, -Math.PI / 2 + TAU * f.countdown);
        ctx.stroke();
      }
    }

    // Outer tick ring.
    const tickR = R * 0.93;
    ctx.save();
    ctx.translate(c, c);
    ctx.rotate(f.rot[0]);
    ctx.lineCap = "butt";
    const ticks = small ? 48 : 120;
    for (let i = 0; i < ticks; i++) {
      const major = small ? i % 8 === 0 : i % 10 === 0;
      const mid = small ? i % 4 === 0 : i % 5 === 0;
      const len = major ? R * 0.07 : mid ? R * 0.045 : R * 0.025;
      const a = (i / ticks) * TAU;
      ctx.strokeStyle = rgba(major ? strong : main, A(major ? 0.85 : mid ? 0.5 : 0.28));
      ctx.lineWidth = major ? 1.6 : 1;
      const cs = Math.cos(a);
      const sn = Math.sin(a);
      ctx.beginPath();
      ctx.moveTo(cs * tickR, sn * tickR);
      ctx.lineTo(cs * (tickR - len), sn * (tickR - len));
      ctx.stroke();
    }
    // Two bright HUD brackets on the tick ring.
    ctx.strokeStyle = rgba(strong, A(0.9));
    ctx.lineWidth = 2;
    for (const base of [0, Math.PI]) {
      ctx.beginPath();
      ctx.arc(0, 0, tickR + 2, base - 0.32, base + 0.32);
      ctx.stroke();
    }
    ctx.restore();

    // Radial spectrum (mirrored: left and right halves meet at the top).
    const inner = R * 0.64;
    const maxLen = R * 0.2;
    const bars = small ? BANDS : BANDS * 2;
    ctx.lineCap = "round";
    ctx.lineWidth = Math.max(1.4, (TAU * inner) / bars / 2.1);
    for (let i = 0; i < bars; i++) {
      const band = small ? (i < BANDS / 2 ? i * 2 : (bars - 1 - i) * 2) : i < BANDS ? i : bars - 1 - i;
      const v = f.bands[band] ?? 0;
      const a = -Math.PI / 2 + ((i + 0.5) / bars) * TAU;
      const len = R * 0.025 + v * maxLen;
      const cs = Math.cos(a);
      const sn = Math.sin(a);
      ctx.strokeStyle = rgba(mix(main, strong, v), A(0.35 + v * 0.65));
      ctx.beginPath();
      ctx.moveTo(c + cs * inner, c + sn * inner);
      ctx.lineTo(c + cs * (inner + len), c + sn * (inner + len));
      ctx.stroke();
    }

    // Segmented arc ring (4 arcs) — counter-rotating.
    ctx.lineCap = "butt";
    const segR = R * 0.585;
    ctx.lineWidth = Math.max(2, R * 0.035);
    for (let i = 0; i < 4; i++) {
      const a0 = f.rot[1] + (i / 4) * TAU + 0.12;
      ctx.strokeStyle = rgba(i % 2 ? second : main, A(0.75));
      ctx.beginPath();
      ctx.arc(c, c, segR, a0, a0 + TAU / 4 - 0.38);
      ctx.stroke();
    }
    // Approval waiting: a pulsing amber segment (and bracket).
    if (f.approval > 0.01) {
      ctx.strokeStyle = rgba(pal.warning, f.approval);
      ctx.lineWidth = Math.max(2.5, R * 0.06);
      ctx.beginPath();
      ctx.arc(c, c, segR, -Math.PI / 2 - 0.55, -Math.PI / 2 + 0.55);
      ctx.stroke();
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(c, c, tickR + 2, -Math.PI / 2 - 0.4, -Math.PI / 2 + 0.4);
      ctx.stroke();
    }
    // Thin full ring under it.
    ctx.lineWidth = 1;
    ctx.strokeStyle = rgba(main, A(0.3));
    ctx.beginPath();
    ctx.arc(c, c, segR - R * 0.04, 0, TAU);
    ctx.stroke();

    // Dashed ring.
    ctx.save();
    ctx.translate(c, c);
    ctx.rotate(f.rot[2]);
    ctx.setLineDash([R * 0.05, R * 0.035]);
    ctx.lineWidth = 1.4;
    ctx.strokeStyle = rgba(strong, A(0.55));
    ctx.beginPath();
    ctx.arc(0, 0, R * 0.5, 0, TAU);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.restore();

    // Chaser ring: 18 segments, a bright head running around.
    const chR = R * 0.43;
    const head = ((f.rot[3] % TAU) + TAU) % TAU;
    ctx.lineWidth = Math.max(2, R * 0.03);
    for (let i = 0; i < 18; i++) {
      const a0 = (i / 18) * TAU;
      let d = (head - a0) % TAU;
      if (d < 0) d += TAU;
      const glow = Math.max(0, 1 - d / (TAU * 0.4));
      ctx.strokeStyle = rgba(mix(main, strong, glow), A(0.22 + glow * 0.78));
      ctx.beginPath();
      ctx.arc(c, c, chR, a0 + 0.04, a0 + TAU / 18 - 0.06);
      ctx.stroke();
    }

    // Thinking: scanning sweep wedge + orbiting dots.
    if (f.sweepAlpha > 0.01) {
      const grad = ctx.createConicGradient(f.sweep, c, c);
      grad.addColorStop(0, rgba(strong, 0.55 * f.sweepAlpha * e));
      grad.addColorStop(0.12, rgba(main, 0));
      grad.addColorStop(1, rgba(main, 0));
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.arc(c, c, R * 0.9, 0, TAU);
      ctx.arc(c, c, R * 0.36, 0, TAU, true);
      ctx.fill();
      for (let k = 0; k < 3; k++) {
        const a = f.sweep * (k % 2 ? -1.3 : 1) + (k * TAU) / 3;
        const rr = R * (0.72 + k * 0.05);
        ctx.fillStyle = rgba(strong, f.sweepAlpha * e);
        ctx.beginPath();
        ctx.arc(c + Math.cos(a) * rr, c + Math.sin(a) * rr, Math.max(1.6, R * 0.022), 0, TAU);
        ctx.fill();
      }
    }

    // Speaking: ripples out of the core.
    if (f.rippleAlpha > 0.01) {
      for (let k = 0; k < 3; k++) {
        const p = (f.ripple + k / 3) % 1;
        ctx.strokeStyle = rgba(strong, (1 - p) * 0.55 * f.rippleAlpha * e * (0.5 + f.level));
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(c, c, R * (0.3 + p * 0.6), 0, TAU);
        ctx.stroke();
      }
    }

    // Particles: drift outwards, more with a louder voice.
    this.stepParticles(f, R);
    for (const p of this.particles) {
      const alpha = Math.sin(Math.min(1, p.life) * Math.PI) * 0.8 * e;
      ctx.fillStyle = rgba(strong, alpha);
      ctx.beginPath();
      ctx.arc(c + Math.cos(p.a) * p.r, c + Math.sin(p.a) * p.r, p.size, 0, TAU);
      ctx.fill();
    }

    // Arc-reactor core: ring of coils + hot glow.
    const coreR = R * (0.3 + f.level * 0.03);
    ctx.lineWidth = Math.max(1.2, R * 0.018);
    ctx.strokeStyle = rgba(strong, A(0.85));
    ctx.beginPath();
    ctx.arc(c, c, coreR, 0, TAU);
    ctx.stroke();
    const coils = small ? 8 : 10;
    for (let i = 0; i < coils; i++) {
      const a0 = -f.rot[1] * 0.3 + (i / coils) * TAU;
      ctx.fillStyle = rgba(i % 2 ? main : strong, A(0.55 + f.level * 0.4));
      ctx.beginPath();
      ctx.arc(c, c, coreR * 0.9, a0 + 0.06, a0 + TAU / coils - 0.08);
      ctx.arc(c, c, coreR * 0.66, a0 + TAU / coils - 0.12, a0 + 0.1, true);
      ctx.closePath();
      ctx.fill();
    }
    ctx.lineWidth = 1;
    ctx.strokeStyle = rgba(main, A(0.6));
    ctx.beginPath();
    ctx.arc(c, c, coreR * 0.58, 0, TAU);
    ctx.stroke();
    const g = coreR * (1.15 + f.level * 0.6 + (f.intro < 1 ? (1 - f.intro) * 0.8 : 0));
    ctx.globalAlpha = A(0.75 + f.level * 0.25);
    ctx.drawImage(sp.core, c - g, c - g, g * 2, g * 2);
    ctx.globalAlpha = 1;
    // Centre lens.
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = rgba(light ? strong : mix(strong, [255, 255, 255], 0.65), A(0.95));
    ctx.beginPath();
    ctx.arc(c, c, coreR * (0.24 + f.level * 0.08), 0, TAU);
    ctx.fill();
    ctx.globalCompositeOperation = "source-over";
  }

  private stepParticles(f: ReactorFrame, R: number): void {
    const dt = this.lastT ? Math.max(0, Math.min(0.1, f.t - this.lastT)) : 0.016;
    this.lastT = f.t;
    const active = f.state !== "idle" && f.state !== "done";
    const spawn = active ? (0.6 + f.level * 5) * dt * 30 : 0;
    let n = Math.floor(spawn) + (Math.random() < spawn % 1 ? 1 : 0);
    const cap = R < 50 ? 14 : 48;
    while (n-- > 0 && this.particles.length < cap) {
      this.particles.push({ a: Math.random() * TAU, r: R * (0.34 + Math.random() * 0.2), v: R * (0.25 + Math.random() * 0.5 + f.level * 0.6), life: 0, size: 0.6 + Math.random() * 1.2 });
    }
    for (const p of this.particles) {
      p.r += p.v * dt;
      p.a += dt * 0.15;
      p.life += dt * 0.9;
    }
    this.particles = this.particles.filter((p) => p.life < 1 && p.r < R * 0.98);
  }

  /** Drops particles (still frames, state resets). */
  clearParticles(): void {
    this.particles = [];
  }
}
