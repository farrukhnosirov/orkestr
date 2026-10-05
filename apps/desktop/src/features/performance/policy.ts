/**
 * Performance Mode policy (§31, ADR-0015 "Rendering").
 *
 * One table decides what every visual feature may do in each mode, so
 * robots, charts, backgrounds and motion degrade together:
 *
 * | mode          | blur/glow | motion  | bg animation | robots        | charts  | GPU effects |
 * |---------------|-----------|---------|--------------|---------------|---------|-------------|
 * | UltraVisual   | strong    | full    | yes          | 60 fps, 3D    | 1 s     | yes         |
 * | Balanced      | normal    | full    | subtle       | 30 fps, 2D    | 2 s     | no          |
 * | Performance   | off       | reduced | no           | 15 fps, 2D    | 5 s     | no          |
 * | BatterySaver  | off       | off     | no           | 10 fps, 2D    | 10 s    | no          |
 *
 * `prefers-reduced-motion` caps motion at "reduced"; a hidden or minimized
 * window pauses everything (visuals never compete with the AI workload, §70).
 * Pure functions — unit-tested in `performance.test.ts`.
 */
import type { PerformanceMode } from "@orkestr/protocol";

export const PERF_MODES: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];

export type MotionLevel = "full" | "reduced" | "off";

export interface PerfPolicy {
  /** The effective mode (after auto-switching). */
  mode: PerformanceMode;
  /** Backdrop blur allowed. */
  blur: boolean;
  /** Multiplier on the theme's blur radius. */
  blurScale: number;
  /** Glow allowed. */
  glow: boolean;
  /** Multiplier on the theme's glow intensity. */
  glowScale: number;
  /** Motion level. */
  motion: MotionLevel;
  /** Multiplier on motion durations (0 = instant). */
  durationScale: number;
  /** Animated background allowed. */
  backgroundAnimation: boolean;
  /** Max robot animation frame rate (0 = frozen). */
  robotFps: number;
  /** 3D (three.js / WebGL) robots allowed. */
  robots3d: boolean;
  /** Minimum refresh interval of live charts, in ms. */
  chartRefreshMs: number;
  /** GPU-heavy effects (WebGL backgrounds, particles) allowed. */
  gpuEffects: boolean;
  /** Everything paused (window hidden / minimized). */
  paused: boolean;
}

type ModeTable = Omit<PerfPolicy, "mode" | "paused">;

const TABLE: Readonly<Record<PerformanceMode, ModeTable>> = {
  UltraVisual: {
    blur: true,
    blurScale: 1.25,
    glow: true,
    glowScale: 1.2,
    motion: "full",
    durationScale: 1,
    backgroundAnimation: true,
    robotFps: 60,
    robots3d: true,
    chartRefreshMs: 1000,
    gpuEffects: true,
  },
  Balanced: {
    blur: true,
    blurScale: 1,
    glow: true,
    glowScale: 1,
    motion: "full",
    durationScale: 1,
    backgroundAnimation: true,
    robotFps: 30,
    robots3d: false,
    chartRefreshMs: 2000,
    gpuEffects: false,
  },
  Performance: {
    blur: false,
    blurScale: 0,
    glow: false,
    glowScale: 0,
    motion: "reduced",
    durationScale: 0.5,
    backgroundAnimation: false,
    robotFps: 15,
    robots3d: false,
    chartRefreshMs: 5000,
    gpuEffects: false,
  },
  BatterySaver: {
    blur: false,
    blurScale: 0,
    glow: false,
    glowScale: 0,
    motion: "off",
    durationScale: 0,
    backgroundAnimation: false,
    robotFps: 0,
    robots3d: false,
    chartRefreshMs: 10000,
    gpuEffects: false,
  },
};

export interface PolicyEnv {
  /** `prefers-reduced-motion: reduce`. */
  reducedMotion: boolean;
  /** The window is hidden or minimized. */
  hidden: boolean;
}

const MOTION_RANK: Record<MotionLevel, number> = { full: 2, reduced: 1, off: 0 };

/** The policy for a mode in an environment. */
export function resolvePolicy(mode: PerformanceMode, env: PolicyEnv): PerfPolicy {
  const base = TABLE[mode];
  const policy: PerfPolicy = { ...base, mode, paused: false };
  if (env.reducedMotion && MOTION_RANK[policy.motion] > MOTION_RANK.reduced) {
    policy.motion = "reduced";
    policy.durationScale = Math.min(policy.durationScale, 0.5);
    policy.backgroundAnimation = false;
  }
  if (env.hidden) {
    policy.paused = true;
    policy.backgroundAnimation = false;
    policy.robotFps = 0;
    policy.gpuEffects = false;
    policy.chartRefreshMs = Math.max(policy.chartRefreshMs, 30000);
  }
  return policy;
}

export type ModeReason = "user" | "battery" | "frameBudget";

export interface ModeInputs {
  /** The user's choice (`AppSettings.performanceMode`). */
  chosen: PerformanceMode;
  /** Switch to Battery Saver while on battery. */
  autoBattery: boolean;
  /** `true` = on battery, `false` = on AC, `null` = unknown. */
  onBattery: boolean | null;
  /** Step down automatically when frames miss their budget. */
  autoDegrade: boolean;
  /** Steps taken down so far (reset when the user picks a mode). */
  degradeSteps: number;
}

/** Most the frame-budget degrade goes: never to Battery Saver on its own. */
const DEGRADE_FLOOR: PerformanceMode = "Performance";

/** The mode actually in effect, and why. */
export function effectiveMode(i: ModeInputs): { mode: PerformanceMode; reason: ModeReason } {
  if (i.autoBattery && i.onBattery === true && i.chosen !== "BatterySaver") {
    return { mode: "BatterySaver", reason: "battery" };
  }
  if (i.autoDegrade && i.degradeSteps > 0) {
    const from = PERF_MODES.indexOf(i.chosen);
    const floor = PERF_MODES.indexOf(DEGRADE_FLOOR);
    if (from < floor) {
      const to = Math.min(from + i.degradeSteps, floor);
      const mode = PERF_MODES[to] ?? i.chosen;
      return { mode, reason: "frameBudget" };
    }
  }
  return { mode: i.chosen, reason: "user" };
}

/** Whether another degrade step would change anything. */
export function canDegrade(i: ModeInputs): boolean {
  const current = effectiveMode(i).mode;
  return PERF_MODES.indexOf(current) < PERF_MODES.indexOf(DEGRADE_FLOOR);
}

export interface FrameMonitorOptions {
  /** Frame budget in ms (default 1000/50: a frame slower than 20 ms is "janky"). */
  budgetMs?: number;
  /** Frames per evaluation window. */
  windowFrames?: number;
  /** Fraction of janky frames that makes a window "bad". */
  jankRatio?: number;
  /** Consecutive bad windows before degrading. */
  badWindows?: number;
}

/**
 * Detects sustained jank from frame deltas. Deltas longer than 250 ms are
 * ignored (tab switches, sleeps, debugger pauses are not rendering cost).
 */
export class FrameBudgetMonitor {
  private readonly budgetMs: number;
  private readonly windowFrames: number;
  private readonly jankRatio: number;
  private readonly badWindows: number;
  private frames = 0;
  private janky = 0;
  private bad = 0;

  constructor(opts: FrameMonitorOptions = {}) {
    this.budgetMs = opts.budgetMs ?? 20;
    this.windowFrames = opts.windowFrames ?? 120;
    this.jankRatio = opts.jankRatio ?? 0.25;
    this.badWindows = opts.badWindows ?? 2;
  }

  /** Feeds one frame delta; returns `"degrade"` when the budget is missed persistently. */
  push(deltaMs: number): "degrade" | null {
    if (!Number.isFinite(deltaMs) || deltaMs <= 0 || deltaMs > 250) return null;
    this.frames += 1;
    if (deltaMs > this.budgetMs) this.janky += 1;
    if (this.frames < this.windowFrames) return null;
    const isBad = this.janky / this.frames > this.jankRatio;
    this.frames = 0;
    this.janky = 0;
    this.bad = isBad ? this.bad + 1 : 0;
    if (this.bad >= this.badWindows) {
      this.bad = 0;
      return "degrade";
    }
    return null;
  }

  reset(): void {
    this.frames = 0;
    this.janky = 0;
    this.bad = 0;
  }
}

/** Root attributes / variables for a policy (applied by `applyPolicyToRoot`). */
export function rootAttributes(p: PerfPolicy): { data: Record<string, string>; vars: Record<string, string> } {
  return {
    data: {
      okPerformance: p.mode,
      okMotion: p.motion,
      okBlur: p.blur ? "on" : "off",
      okGlow: p.glow ? "on" : "off",
      okBgAnim: p.backgroundAnimation ? "on" : "off",
      okGpu: p.gpuEffects ? "on" : "off",
      okPaused: p.paused ? "on" : "off",
    },
    vars: {
      "--ok-motion-scale": String(p.durationScale),
      "--ok-blur-scale": String(p.blurScale),
      "--ok-glow-scale": String(p.glowScale),
    },
  };
}

/** Applies a policy to the document root (data attributes + CSS variables). */
export function applyPolicyToRoot(p: PerfPolicy, root: HTMLElement = document.documentElement): void {
  const { data, vars } = rootAttributes(p);
  for (const [k, v] of Object.entries(data)) root.dataset[k] = v;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
}
