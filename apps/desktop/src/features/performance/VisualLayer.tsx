/**
 * Mounted once by the main window: runs the Performance Mode machinery and
 * the Ultra Visual background, and loads the shell polish styles
 * (`visual.css`: density, background dim, perf gating, motion, focus).
 */
import { lazy, Suspense } from "react";

import { usePerfPolicy } from "./perfStore";
import { usePerformanceRuntime } from "./usePerformanceRuntime";
import "./visual.css";

const AmbientField = lazy(() => import("./AmbientField").then((m) => ({ default: m.AmbientField })));

export function VisualLayer({ connected }: { connected: boolean }) {
  usePerformanceRuntime(connected);
  const policy = usePerfPolicy();
  return policy.gpuEffects && !policy.paused ? (
    <Suspense fallback={null}>
      <AmbientField />
    </Suspense>
  ) : null;
}
