/**
 * Ultra Visual background (§28/§31): slow drifting particles on a canvas
 * behind the workspace. Mounted only while the policy allows GPU effects
 * (Ultra Visual) and the window is visible; capped at 30 fps and a 1.5
 * device-pixel ratio, so it never competes with the AI workload.
 */
import { useEffect, useRef } from "react";

import { getPerfPolicy } from "./perfStore";

interface Particle {
  x: number;
  y: number;
  r: number;
  vx: number;
  vy: number;
  hue: 0 | 1 | 2;
  phase: number;
}

const COUNT = 70;
const FRAME_MS = 1000 / 30;

/** Seeded PRNG (stable layout per mount, no Math.random dependency in tests). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function makeParticles(count: number, rand: () => number): Particle[] {
  return Array.from({ length: count }, () => ({
    x: rand(),
    y: rand(),
    r: 0.4 + rand() * 1.4,
    vx: (rand() - 0.5) * 0.004,
    vy: (rand() - 0.5) * 0.003,
    hue: Math.floor(rand() * 3) as 0 | 1 | 2,
    phase: rand() * Math.PI * 2,
  }));
}

function cssColor(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

export function AmbientField() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return undefined;
    const particles = makeParticles(COUNT, mulberry32(0x0c0ffee));
    const colors = [cssColor("--ok-color-accent", "#22d3ee"), cssColor("--ok-color-violet", "#a78bfa"), cssColor("--ok-color-text", "#e8ecff")];
    let raf = 0;
    let last = 0;
    let w = 0;
    let h = 0;

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    const ro = typeof ResizeObserver === "function" ? new ResizeObserver(resize) : null;
    ro?.observe(canvas);

    const frame = (now: number) => {
      raf = window.requestAnimationFrame(frame);
      const policy = getPerfPolicy();
      if (policy.paused || !policy.gpuEffects) return;
      if (now - last < FRAME_MS) return;
      const dt = last ? Math.min(now - last, 100) / 16.7 : 1;
      last = now;
      ctx.clearRect(0, 0, w, h);
      for (const p of particles) {
        p.x = (p.x + p.vx * dt * 0.1 + 1) % 1;
        p.y = (p.y + p.vy * dt * 0.1 + 1) % 1;
        const twinkle = 0.45 + 0.35 * Math.sin(now / 1400 + p.phase);
        ctx.globalAlpha = twinkle;
        ctx.fillStyle = colors[p.hue] ?? "#fff";
        ctx.beginPath();
        ctx.arc(p.x * w, p.y * h, p.r, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.globalAlpha = 1;
    };
    raf = window.requestAnimationFrame(frame);
    return () => {
      window.cancelAnimationFrame(raf);
      ro?.disconnect();
    };
  }, []);

  return <canvas ref={ref} className="ambient-field" aria-hidden="true" />;
}
