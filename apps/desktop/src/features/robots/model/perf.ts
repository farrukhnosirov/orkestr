/**
 * Performance tiers of the robots (§31, §70, ADR-0016): visuals never slow
 * the AI work down. Frame rate caps per Performance Mode, a static pose in
 * Battery Saver and with `prefers-reduced-motion`, idle robots pause unless
 * hovered, and expensive effects (glow filters, 3D) only in the richer modes.
 */
import type { PerformanceMode } from "@orkestr/protocol";

import { PERF_MODES, resolvePolicy } from "../../performance/policy";
import type { RobotState } from "./states";

const STEADY = { reducedMotion: false, hidden: false };

/** Robot frame-rate caps — the central Performance Mode table (`policy.ts`). */
export const FPS_BY_MODE: Readonly<Record<PerformanceMode, number>> = Object.fromEntries(
  PERF_MODES.map((m) => [m, resolvePolicy(m, STEADY).robotFps]),
) as Record<PerformanceMode, number>;

export interface FpsInput {
  mode: PerformanceMode;
  reducedMotion: boolean;
  state: RobotState;
  asleep: boolean;
  hovered: boolean;
  /** Animation intensity of the design (0 = still). */
  intensity: number;
  /** The window is hidden / occluded (document.hidden). */
  hidden: boolean;
}

/** Frame rate cap for a robot; 0 = draw one static frame. */
export function robotFps(i: FpsInput): number {
  const base = FPS_BY_MODE[i.mode];
  if (base === 0 || i.reducedMotion || i.hidden || i.intensity <= 0) return 0;
  const resting = i.asleep || i.state === "Idle";
  if (resting && !i.hovered) {
    // Gentle breathing only in the visual modes; otherwise a still robot.
    return i.mode === "UltraVisual" || i.mode === "Balanced" ? Math.min(base, 12) : 0;
  }
  return base;
}

export interface RobotEffects {
  /** SVG glow filters, soft shadows. */
  glow: boolean;
  /** Particles (confetti, sparks, bubbles). */
  particles: boolean;
  /** three.js renderer allowed. */
  threeD: boolean;
}

export function robotEffects(mode: PerformanceMode, reducedMotion: boolean): RobotEffects {
  const p = resolvePolicy(mode, STEADY);
  return {
    glow: p.glow,
    particles: !reducedMotion && p.motion !== "off",
    threeD: p.robots3d && !reducedMotion,
  };
}

/** Minimal frame limiter: whether a frame at `now` should render given the last one. */
export function shouldRender(now: number, last: number, fps: number): boolean {
  if (fps <= 0) return false;
  // Allow 1 ms of jitter so 60 fps on a 60 Hz display does not drop frames.
  return now - last >= 1000 / fps - 1;
}
