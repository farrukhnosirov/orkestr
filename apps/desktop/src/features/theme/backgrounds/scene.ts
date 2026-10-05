/**
 * Procedural backgrounds (§29 backgrounds): nebula clouds, perspective grid
 * and drifting particles drawn on a 2D canvas. Cheap by construction:
 * - rendered at a reduced resolution (`resolution`, 0.5 in Balanced),
 * - frame-capped (`fps`; 0 = one static frame),
 * - paused while the window is hidden (`visibilitychange`),
 * - deterministic (seeded) so a static frame is stable across redraws.
 * The WebGL nebula (Ultra Visual only) lives in `webglNebula.ts`.
 */
import type { ThemeBackground } from "@orkestr/protocol";
import type { Rgba } from "@orkestr/protocol";
import { parseColor } from "@orkestr/ui";

export interface SceneParams {
  kind: ThemeBackground["kind"];
  colors: Rgba[];
  intensity: number;
  speed: number;
  density: number;
  fps: number;
  resolution: number;
}

/** Whether a frame is due under an fps cap (always true for the first frame). */
export function frameDue(now: number, last: number | null, fps: number): boolean {
  if (last === null) return true;
  if (fps <= 0) return false;
  return now - last >= 1000 / fps - 1;
}

/** Number of particles for a canvas area (CSS px²) and density (0..1); bounded for perf. */
export function particleCount(areaPx: number, density: number): number {
  const perMegapixel = 40 + density * 160;
  return Math.max(12, Math.min(260, Math.round((areaPx / 1_000_000) * perMegapixel)));
}

/** Palette: the theme's background colors, else the accent tokens; always ≥ 1 color. */
export function scenePalette(colors: readonly string[], fallback: readonly string[]): Rgba[] {
  const parse = (list: readonly string[]) => list.map((c) => parseColor(c)).filter((c): c is Rgba => c !== null);
  const own = parse(colors);
  if (own.length > 0) return own;
  const fb = parse(fallback);
  return fb.length > 0 ? fb : [{ r: 34, g: 211, b: 238, a: 1 }];
}

/** Deterministic PRNG (mulberry32). */
export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rgba = (c: Rgba, a: number) => `rgba(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)}, ${Math.max(0, Math.min(1, a))})`;
const pick = (p: readonly Rgba[], i: number): Rgba => p[i % p.length] ?? { r: 34, g: 211, b: 238, a: 1 };

interface Particle {
  x: number;
  y: number;
  r: number;
  vx: number;
  vy: number;
  phase: number;
  color: number;
}

/** Draws one frame at time `t` (seconds). Pure apart from the context. */
export function drawScene(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, p: SceneParams, particles: Particle[]): void {
  ctx.clearRect(0, 0, w, h);
  const k = p.intensity;
  const time = t * p.speed;
  if (p.kind === "Nebula") {
    ctx.globalCompositeOperation = "lighter";
    const rand = rng(7);
    const blobs = 6;
    for (let i = 0; i < blobs; i += 1) {
      const bx = rand();
      const by = rand();
      const rr = 0.35 + rand() * 0.4;
      const ph = rand() * Math.PI * 2;
      const x = (bx + 0.08 * Math.sin(time * 0.21 + ph)) * w;
      const y = (by + 0.06 * Math.cos(time * 0.17 + ph * 1.3)) * h;
      const radius = rr * Math.max(w, h) * (0.9 + 0.1 * Math.sin(time * 0.3 + ph));
      const c = pick(p.colors, i);
      const g = ctx.createRadialGradient(x, y, 0, x, y, radius);
      g.addColorStop(0, rgba(c, 0.32 * k));
      g.addColorStop(0.45, rgba(c, 0.12 * k));
      g.addColorStop(1, rgba(c, 0));
      ctx.fillStyle = g;
      ctx.fillRect(0, 0, w, h);
    }
    ctx.globalCompositeOperation = "source-over";
    return;
  }
  if (p.kind === "Grid") {
    const horizon = h * 0.52;
    const c = pick(p.colors, 0);
    const c2 = pick(p.colors, 1);
    const vx = w / 2;
    const cols = Math.round(10 + p.density * 30);
    ctx.lineWidth = Math.max(1, w / 1600);
    // Converging verticals.
    for (let i = -cols; i <= cols; i += 1) {
      const xb = vx + (i / cols) * w * 1.6;
      const grad = ctx.createLinearGradient(0, horizon, 0, h);
      grad.addColorStop(0, rgba(c, 0));
      grad.addColorStop(1, rgba(c, 0.45 * k));
      ctx.strokeStyle = grad;
      ctx.beginPath();
      ctx.moveTo(vx, horizon);
      ctx.lineTo(xb, h);
      ctx.stroke();
    }
    // Receding horizontals (scroll toward the viewer).
    const rows = Math.round(8 + p.density * 16);
    const offset = (time * 0.35) % 1;
    for (let j = 0; j < rows; j += 1) {
      const z = (j + 1 - offset) / rows; // 0..1 (far..near)
      const y = horizon + (h - horizon) * z * z;
      ctx.strokeStyle = rgba(c2, 0.5 * k * z);
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }
    // Horizon glow.
    const glow = ctx.createLinearGradient(0, horizon - h * 0.18, 0, horizon + 4);
    glow.addColorStop(0, rgba(c2, 0));
    glow.addColorStop(1, rgba(c2, 0.22 * k));
    ctx.fillStyle = glow;
    ctx.fillRect(0, horizon - h * 0.18, w, h * 0.18 + 4);
    return;
  }
  if (p.kind === "Particles") {
    for (const q of particles) {
      const x = ((q.x + q.vx * time) % 1 + 1) % 1;
      const y = ((q.y + q.vy * time) % 1 + 1) % 1;
      const tw = 0.55 + 0.45 * Math.sin(time * 1.7 + q.phase);
      const c = pick(p.colors, q.color);
      const px = x * w;
      const py = y * h;
      const r = q.r * (w / 1400 + 0.6);
      const g = ctx.createRadialGradient(px, py, 0, px, py, r * 4);
      g.addColorStop(0, rgba(c, 0.85 * k * tw));
      g.addColorStop(1, rgba(c, 0));
      ctx.fillStyle = g;
      ctx.fillRect(px - r * 4, py - r * 4, r * 8, r * 8);
    }
  }
}

export function makeParticles(count: number, palette: number): Particle[] {
  const rand = rng(42);
  return Array.from({ length: count }, () => ({
    x: rand(),
    y: rand(),
    r: 0.6 + rand() * 1.6,
    vx: (rand() - 0.5) * 0.02,
    vy: -0.008 - rand() * 0.02,
    phase: rand() * Math.PI * 2,
    color: Math.floor(rand() * Math.max(1, palette)),
  }));
}

export interface SceneHandle {
  update: (p: SceneParams) => void;
  dispose: () => void;
}

/**
 * Runs a scene on `canvas`: sizes it with a ResizeObserver, animates with
 * rAF under the fps cap, pauses while hidden and redraws a static frame when
 * `fps` is 0.
 */
export function runScene(canvas: HTMLCanvasElement, initial: SceneParams): SceneHandle {
  const ctx = canvas.getContext("2d", { alpha: true });
  let params = initial;
  let raf = 0;
  let last: number | null = null;
  let particles: Particle[] = [];
  let disposed = false;
  const start = performance.now();

  const size = () => {
    const dpr = Math.min(2, window.devicePixelRatio || 1) * params.resolution;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    particles = makeParticles(particleCount(canvas.clientWidth * canvas.clientHeight, params.density), params.colors.length);
  };

  const draw = (now: number) => {
    if (!ctx) return;
    drawScene(ctx, canvas.width, canvas.height, (now - start) / 1000, params, particles);
  };

  const loop = (now: number) => {
    raf = 0;
    if (disposed || document.hidden) return;
    if (frameDue(now, last, params.fps)) {
      last = now;
      draw(now);
    }
    if (params.fps > 0) raf = requestAnimationFrame(loop);
  };

  const kick = () => {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    last = null;
    if (params.fps > 0) raf = requestAnimationFrame(loop);
    else draw(performance.now());
  };

  const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => {
    size();
    if (params.fps <= 0) draw(performance.now());
  });
  ro?.observe(canvas);
  const onVisibility = () => {
    if (!document.hidden) kick();
  };
  document.addEventListener("visibilitychange", onVisibility);
  size();
  kick();

  return {
    update: (p) => {
      const resized = p.resolution !== params.resolution || p.density !== params.density || p.colors.length !== params.colors.length;
      params = p;
      if (resized) size();
      kick();
    },
    dispose: () => {
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      ro?.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
    },
  };
}
