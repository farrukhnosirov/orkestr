/**
 * Visual budget for voice UI (§31, §70): animation frame rate and effects
 * from the effective performance mode and `prefers-reduced-motion` (central
 * perf store, shared with the theme, robots, HUD and Mission Control).
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { usePerfStore } from "../../performance/perfStore";

export interface VisualBudget {
  /** Waveform redraw rate; 0 = static. */
  fps: number;
  /** Glass blur / glow allowed. */
  effects: boolean;
  /** Enter/exit motion allowed. */
  motion: boolean;
}

export function visualBudget(mode: PerformanceMode, reducedMotion: boolean): VisualBudget {
  if (reducedMotion) return { fps: 0, effects: mode !== "BatterySaver" && mode !== "Performance", motion: false };
  switch (mode) {
    case "UltraVisual":
      return { fps: 60, effects: true, motion: true };
    case "Balanced":
      return { fps: 30, effects: true, motion: true };
    case "Performance":
      return { fps: 15, effects: false, motion: true };
    case "BatterySaver":
      return { fps: 0, effects: false, motion: false };
  }
}

/** Live budget from the central Performance Mode store (effective mode, reduced motion, paused window). */
export function useVisualBudget(): VisualBudget {
  const mode = usePerfStore((s) => s.mode);
  const reduced = usePerfStore((s) => s.reducedMotion);
  const paused = usePerfStore((s) => s.policy.paused);
  const budget = visualBudget(mode, reduced);
  return paused ? { ...budget, fps: 0 } : budget;
}
