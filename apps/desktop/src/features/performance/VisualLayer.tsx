/**
 * Mounted once by the main window: runs the Performance Mode machinery and
 * loads the shell polish styles (`visual.css`: density, background dim,
 * perf gating, motion, focus). The animated background itself is the theme
 * engine's `ThemeBackdrop`, which follows the same effective policy.
 */
import { useEffect } from "react";

import { startPerfBroadcast } from "./perfBroadcast";
import { usePerformanceRuntime } from "./usePerformanceRuntime";
import "./visual.css";

export function VisualLayer({ connected }: { connected: boolean }) {
  usePerformanceRuntime(connected);
  // Robot windows follow the effective mode derived here (auto Battery Saver, degrade).
  useEffect(() => startPerfBroadcast(), []);
  return null;
}
