/**
 * Animation clock and environment hooks for robot renderers. The clock is a
 * single `requestAnimationFrame` loop capped to the robot's fps tier; with
 * fps 0 it returns a fixed representative frame and schedules nothing.
 */
import { useEffect, useState } from "react";

import { shouldRender } from "../model/perf";

/** Milliseconds clock at most `fps` times per second (0 = static `staticMs`). */
export function useRobotClock(fps: number, staticMs: number): number {
  const [t, setT] = useState(() => (fps > 0 ? performance.now() : staticMs));
  useEffect(() => {
    if (fps <= 0) {
      setT(staticMs);
      return undefined;
    }
    let raf = 0;
    let last = 0;
    const loop = (now: number) => {
      if (shouldRender(now, last, fps)) {
        last = now;
        setT(now);
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [fps, staticMs]);
  return t;
}

function mediaQuery(q: string): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia(q) : null;
}

/** `prefers-reduced-motion: reduce`. */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => mediaQuery("(prefers-reduced-motion: reduce)")?.matches ?? false);
  useEffect(() => {
    const mq = mediaQuery("(prefers-reduced-motion: reduce)");
    if (!mq) return undefined;
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}

/** `document.hidden` (minimized / occluded windows stop animating). */
export function usePageHidden(): boolean {
  const [hidden, setHidden] = useState(() => (typeof document !== "undefined" ? document.hidden : false));
  useEffect(() => {
    const on = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return hidden;
}

/** Whether WebGL is available (3D falls back to 2D otherwise). */
export function webglAvailable(): boolean {
  try {
    const c = document.createElement("canvas");
    return Boolean(c.getContext("webgl2") ?? c.getContext("webgl"));
  } catch {
    return false;
  }
}
