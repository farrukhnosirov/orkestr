/**
 * Visual budget for voice UI (§31, §70): animation frame rate and effects
 * from the performance mode and `prefers-reduced-motion`. Pure + a hook.
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { useSettingsStore } from "../../settings/settingsStore";

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

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function useVisualBudget(): VisualBudget {
  const mode = useSettingsStore((s) => s.settings?.performanceMode ?? "Balanced");
  const [reduced, setReduced] = useState(prefersReducedMotion);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);
  return visualBudget(mode, reduced);
}
