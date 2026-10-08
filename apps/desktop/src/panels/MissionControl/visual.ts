/**
 * Visual budget of Mission Control (§31, §70): visuals scale with the
 * Performance Mode and `prefers-reduced-motion`, and never at the cost of
 * the AI workload.
 *
 * `useVisualBudget` follows the central Performance Mode store (effective
 * mode after battery / frame-budget auto-switching, reduced motion, paused
 * windows) — the same policy the theme backgrounds, robots and HUD use.
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { usePerfStore } from "../../features/performance/perfStore";

export interface VisualBudget {
  mode: PerformanceMode;
  /** Layout/enter animations and state pulses. */
  animate: boolean;
  /** Glow, gradients under charts, blur-heavy glass. */
  rich: boolean;
  /** Chart points kept on screen (fewer = cheaper SVG). */
  chartPoints: number;
}

/** Pure mapping (unit-tested). */
export function visualBudget(mode: PerformanceMode, reducedMotion: boolean): VisualBudget {
  switch (mode) {
    case "UltraVisual":
      return { mode, animate: !reducedMotion, rich: true, chartPoints: 150 };
    case "Balanced":
      return { mode, animate: !reducedMotion, rich: true, chartPoints: 90 };
    case "Performance":
      return { mode, animate: false, rich: false, chartPoints: 60 };
    case "BatterySaver":
      return { mode, animate: false, rich: false, chartPoints: 30 };
    default:
      return { mode, animate: !reducedMotion, rich: true, chartPoints: 90 };
  }
}

function reducedMotionQuery(): MediaQueryList | null {
  try {
    return typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-reduced-motion: reduce)")
      : null;
  } catch {
    return null;
  }
}

export function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => reducedMotionQuery()?.matches ?? false);
  useEffect(() => {
    const mq = reducedMotionQuery();
    if (!mq) return undefined;
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);
  return reduced;
}

export function useVisualBudget(): VisualBudget {
  const mode = usePerfStore((s) => s.mode);
  const paused = usePerfStore((s) => s.policy.paused || s.policy.idle);
  const reduced = usePrefersReducedMotion();
  const budget = visualBudget(mode, reduced);
  return paused ? { ...budget, animate: false } : budget;
}
