/**
 * The Jarvis reactor circle (canvas). Generic: the host passes the visual
 * state, a level and — optionally — a live {@link JarvisSignal} (mic /
 * TTS spectrum). Frame rate is capped by `fps` (0 = one still frame:
 * Battery Saver, reduced motion); drawing pauses while the page is hidden
 * or the canvas is off-screen.
 */
import { useEffect, useRef } from "react";

import { BANDS, type JarvisSignal, type JarvisState, type ReactorAmbient, ReactorDynamics } from "./model";
import { ReactorRenderer, readPalette } from "./renderer";

export interface JarvisReactorProps {
  state: JarvisState;
  /** Fallback level 0..1 when there is no live signal. */
  level: number;
  /** Diameter, CSS px. */
  size: number;
  /** Frame-rate cap; 0 = still frame. */
  fps: number;
  signal?: JarvisSignal | null;
  /** Changes when the theme changes (colours are re-read). */
  themeKey?: string;
  /** Accent override (theme orb accent). */
  accent?: string | null;
  className?: string;
  /** Always lit at rest (the desktop orb), with the agents' mood. */
  presence?: boolean;
  ambient?: ReactorAmbient;
  workers?: number;
}

export function JarvisReactor({ state, level, size, fps, signal = null, themeKey = "", accent = null, className, presence = false, ambient = "calm", workers = 0 }: JarvisReactorProps) {
  const ref = useRef<HTMLCanvasElement | null>(null);
  const live = useRef({ state, level, signal, opts: { presence, ambient, workers } });
  live.current = { state, level, signal, opts: { presence, ambient, workers } };
  const renderer = useRef<ReactorRenderer | null>(null);
  const dyn = useRef(new ReactorDynamics());

  // Renderer + palette.
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    try {
      renderer.current ??= new ReactorRenderer(canvas);
    } catch {
      renderer.current = null; // no 2D context (tests): nothing to draw
      return;
    }
    renderer.current.resize(size, window.devicePixelRatio || 1);
    renderer.current.setPalette(readPalette(accent));
  }, [size, themeKey, accent]);

  // Loop.
  useEffect(() => {
    const r = renderer.current;
    if (!r) return undefined;
    const bands = new Float32Array(BANDS);
    if (fps <= 0) {
      r.clearParticles();
      r.draw(dyn.current.still(state, level, { presence, ambient, workers }));
      return undefined;
    }
    let raf = 0;
    let last = 0;
    let visible = true;
    let io: IntersectionObserver | null = null;
    if (typeof IntersectionObserver === "function" && ref.current) {
      io = new IntersectionObserver((entries) => (visible = entries.some((en) => en.isIntersecting)));
      io.observe(ref.current);
    }
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (!visible || document.hidden) {
        last = now;
        return;
      }
      const dt = last ? now - last : 16;
      if (dt < 1000 / fps - 2) return;
      last = now;
      const cur = live.current;
      const s = cur.signal?.sample(bands, now) ?? null;
      r.draw(dyn.current.step(now, dt, cur.state, s ? s.level : cur.level, s ? bands : null, cur.opts));
    };
    raf = requestAnimationFrame(loop);
    return () => {
      cancelAnimationFrame(raf);
      io?.disconnect();
    };
    // `state` / `level` are read live through the ref; a still frame redraws on change.
  }, [fps, size, themeKey, accent, fps <= 0 ? `${state}|${ambient}|${presence}` : "", fps <= 0 ? Math.round(level * 10) : 0]);

  return <canvas ref={ref} className={className ?? "jv-reactor"} style={{ width: size, height: size }} aria-hidden="true" data-jarvis-state={state} data-jarvis-ambient={presence ? ambient : undefined} />;
}
