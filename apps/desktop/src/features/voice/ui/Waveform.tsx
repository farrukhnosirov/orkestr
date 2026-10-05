/**
 * Listening visual of the voice HUD: a glowing orb whose rim follows the
 * live microphone waveform (AnalyserNode) or, without a mic, a slow idle
 * breathing curve. Canvas 2D at a frame rate capped by the visual budget
 * (static in Battery Saver / reduced motion); stops drawing when hidden.
 */
import { useEffect, useRef } from "react";

import type { VoicePhase } from "../store";

export interface WaveformProps {
  analyser: AnalyserNode | null;
  phase: VoicePhase;
  /** 0..1 level (fallback when there is no analyser). */
  level: number;
  fps: number;
  size?: number;
}

/** `color` with alpha `a` (hex / rgb()); unknown formats are returned unchanged. */
export function withAlpha(color: string, a: number): string {
  const c = color.trim();
  let m = /^#([0-9a-f]{3})$/i.exec(c);
  if (m?.[1]) {
    const [r, g, b] = m[1].split("").map((h) => parseInt(h + h, 16));
    return `rgba(${r}, ${g}, ${b}, ${a})`;
  }
  m = /^#([0-9a-f]{6})(?:[0-9a-f]{2})?$/i.exec(c);
  if (m?.[1]) {
    const n = parseInt(m[1], 16);
    return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
  }
  m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(c);
  if (m) return `rgba(${m[1]}, ${m[2]}, ${m[3]}, ${a})`;
  return c;
}

function cssVar(el: Element, name: string, fallback: string): string {
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  return v || fallback;
}

export function Waveform({ analyser, phase, level, fps, size = 132 }: WaveformProps) {
  const ref = useRef<HTMLCanvasElement | null>(null);
  const levelRef = useRef(level);
  levelRef.current = level;

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    ctx.scale(dpr, dpr);
    const accent = cssVar(canvas, "--ok-color-accent", "#5eead4");
    const violet = cssVar(canvas, "--ok-color-violet", "#a78bfa");
    const danger = cssVar(canvas, "--ok-color-danger", "#f87171");
    const data = analyser ? new Uint8Array(analyser.fftSize) : null;
    const points = 96;
    let raf = 0;
    let last = 0;
    let phaseT = 0;

    const draw = (now: number) => {
      const c = size / 2;
      const r0 = size * 0.3;
      ctx.clearRect(0, 0, size, size);
      if (data && analyser) analyser.getByteTimeDomainData(data);
      const lvl = levelRef.current;
      phaseT = now / 1000;
      const listening = phase === "listening";
      const busy = phase === "transcribing" || phase === "thinking";
      const color = phase === "error" ? danger : listening ? accent : violet;

      // Core glow.
      const g = ctx.createRadialGradient(c, c, r0 * 0.1, c, c, r0 * 1.5);
      g.addColorStop(0, withAlpha(color, 0.33));
      g.addColorStop(0.55, withAlpha(color, 0.1));
      g.addColorStop(1, withAlpha(color, 0));
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(c, c, r0 * 1.5, 0, Math.PI * 2);
      ctx.fill();

      // Rim: two offset strokes (accent + violet) for depth.
      for (let layer = 0; layer < 2; layer++) {
        ctx.beginPath();
        for (let i = 0; i <= points; i++) {
          const a = (i / points) * Math.PI * 2;
          let amp: number;
          if (data && listening) {
            const idx = Math.floor((i / points) * (data.length - 1));
            amp = ((data[idx] ?? 128) - 128) / 128;
            amp *= 0.9 + lvl;
          } else if (busy) {
            amp = 0.12 * Math.sin(a * 3 + phaseT * 4 + layer);
          } else {
            amp = 0.05 * Math.sin(a * 2 + phaseT * 1.2 + layer * 1.7) + lvl * 0.2 * Math.sin(a * 5 + phaseT * 3);
          }
          const r = r0 + amp * size * 0.11 + layer * 3;
          const x = c + Math.cos(a) * r;
          const y = c + Math.sin(a) * r;
          if (i === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        }
        ctx.closePath();
        ctx.lineWidth = layer === 0 ? 2 : 1.25;
        ctx.strokeStyle = layer === 0 ? color : withAlpha(violet, 0.66);
        ctx.shadowColor = color;
        ctx.shadowBlur = layer === 0 ? 14 : 0;
        ctx.stroke();
        ctx.shadowBlur = 0;
      }

      // Busy: orbiting spark.
      if (busy) {
        const a = phaseT * 3.2;
        ctx.fillStyle = accent;
        ctx.beginPath();
        ctx.arc(c + Math.cos(a) * (r0 + 8), c + Math.sin(a) * (r0 + 8), 3, 0, Math.PI * 2);
        ctx.fill();
      }
    };

    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (now - last < 1000 / fps - 1) return;
      last = now;
      draw(now);
    };
    if (fps > 0) raf = requestAnimationFrame(loop);
    else draw(0);
    return () => cancelAnimationFrame(raf);
  }, [analyser, phase, fps, size]);

  return <canvas ref={ref} className="vx-wave" style={{ width: size, height: size }} aria-hidden="true" />;
}
