/**
 * Startup sequence logic (pure, unit-tested; ADR-0053).
 *
 * The animation runs WHILE the core starts and never holds the app back:
 *
 *   intro ──▶ [gate] ──▶ online ──▶ exit ──▶ done
 *                │
 *                └─ holding (core not ready yet): the clock stops at the
 *                   gate and an idle «systems coming online» loop plays
 *
 * The animation clock (`t`, seconds) runs with real time until the gate; it
 * only passes the gate when the core is ready (and, for the full version,
 * the other startup steps reported or a short grace ran out). After the gate
 * the rest (greeting, fly-to-title-bar) plays out. A skip (click / key) or a
 * failed start jumps straight to a short exit.
 */
import type { PerformanceMode } from "@orkestr/protocol";

import type { BootWhen } from "./prefs";

/** full = first launch after install / update; short = regular start; static = reduced motion. */
export type BootVariant = "full" | "short" | "static" | "none";

export interface Timeline {
  /** Animation time (s) the sequence waits at until the core is ready. */
  gate: number;
  /** Animation time (s) the exit (fly to the title bar + fade) starts. */
  exitAt: number;
  /** Exit duration (s). */
  exitDur: number;
  /** Full version: also wait (up to `settleGraceMs` after core readiness) for the other startup steps. */
  settleGraceMs: number;
}

export const TIMELINES: Readonly<Record<Exclude<BootVariant, "none">, Timeline>> = {
  full: { gate: 2.9, exitAt: 4.05, exitDur: 0.8, settleGraceMs: 600 },
  short: { gate: 0.85, exitAt: 1.45, exitDur: 0.5, settleGraceMs: 0 },
  static: { gate: 0.35, exitAt: 0.75, exitDur: 0.35, settleGraceMs: 0 },
};

/** Exit length after a skip / failure. */
export const SKIP_EXIT_S = 0.3;

/** Total minimum length (s) when the core is ready from the start. */
export function minDuration(v: Exclude<BootVariant, "none">): number {
  const tl = TIMELINES[v];
  return tl.exitAt + tl.exitDur;
}

export interface VariantInput {
  pref: BootWhen;
  firstLaunch: boolean;
  reducedMotion: boolean;
  performanceMode: PerformanceMode;
  /** Window hidden at start (autostart to tray). */
  hidden: boolean;
}

/** Which version plays on this start. */
export function chooseVariant(i: VariantInput): BootVariant {
  if (i.hidden || i.pref === "off") return "none";
  if (i.pref === "firstLaunch" && !i.firstLaunch) return "none";
  if (i.reducedMotion || i.performanceMode === "Performance" || i.performanceMode === "BatterySaver") return "static";
  return i.firstLaunch ? "full" : "short";
}

export type BootPhase = "intro" | "holding" | "online" | "exit" | "done";

export interface BootFrameState {
  phase: BootPhase;
  /** Animation time, s (frozen at the gate while holding). */
  t: number;
  /** Seconds spent holding at the gate. */
  hold: number;
  /** Exit progress 0..1 (exit phase). */
  exitP: number;
  /** Exit flies the reactor to the title bar (false after a skip before readiness / a failure). */
  fly: boolean;
}

interface ExitPlan {
  at: number;
  dur: number;
  fly: boolean;
}

/** The sequencer: feed it readiness and time, read the frame state. */
export class BootSequencer {
  private readyAt: number | null = null;
  private settledAt: number | null = null;
  private gateOpenAt: number | null = null;
  private exit: ExitPlan | null = null;
  private tl: Timeline;

  constructor(
    public variant: Exclude<BootVariant, "none">,
    private readonly startMs: number,
  ) {
    this.tl = TIMELINES[variant];
  }

  get timeline(): Timeline {
    return this.tl;
  }

  /** Switches the version early in the intro (the core's startup check answered late). */
  upgrade(variant: Exclude<BootVariant, "none">, nowMs: number): boolean {
    if (variant === this.variant || this.exit || this.gateOpenAt !== null) return false;
    if ((nowMs - this.startMs) / 1000 > 0.5) return false;
    this.variant = variant;
    this.tl = TIMELINES[variant];
    return true;
  }

  /** The core connection is open. */
  markReady(nowMs: number): void {
    if (this.readyAt === null) this.readyAt = nowMs;
  }

  /** The other startup steps (providers, voice, nodes, plugins) reported. */
  markSettled(nowMs: number): void {
    if (this.settledAt === null) this.settledAt = nowMs;
  }

  get isReady(): boolean {
    return this.readyAt !== null;
  }

  /** Click / Esc / any key: leave now (flying only when the app is usable). */
  skip(nowMs: number): void {
    if (this.exit) {
      // Already leaving: just make sure it is quick.
      const left = this.exit.at + this.exit.dur * 1000 - nowMs;
      if (left > SKIP_EXIT_S * 1000) this.exit = { at: nowMs, dur: SKIP_EXIT_S, fly: this.exit.fly && this.isReady };
      return;
    }
    this.exit = { at: nowMs, dur: SKIP_EXIT_S, fly: this.isReady };
  }

  /** The start failed (connection closed): fade out, the connection screen shows the error. */
  fail(nowMs: number): void {
    if (!this.exit) this.exit = { at: nowMs, dur: SKIP_EXIT_S, fly: false };
  }

  /** Real time (ms) the gate condition was met, or null. */
  private openTime(nowMs: number): number | null {
    if (this.readyAt === null) return null;
    if (this.tl.settleGraceMs <= 0) return this.readyAt;
    if (this.settledAt !== null) return Math.max(this.readyAt, this.settledAt);
    const graceEnd = this.readyAt + this.tl.settleGraceMs;
    return nowMs >= graceEnd ? graceEnd : null;
  }

  state(nowMs: number): BootFrameState {
    const tl = this.tl;
    if (!this.exit && this.gateOpenAt === null) {
      const elapsed = (nowMs - this.startMs) / 1000;
      if (elapsed < tl.gate) return { phase: "intro", t: Math.max(0, elapsed), hold: 0, exitP: 0, fly: true };
      const open = this.openTime(nowMs);
      if (open === null) return { phase: "holding", t: tl.gate, hold: elapsed - tl.gate, exitP: 0, fly: true };
      this.gateOpenAt = Math.max(this.startMs + tl.gate * 1000, open);
    }
    if (!this.exit && this.gateOpenAt !== null) {
      const t = tl.gate + (nowMs - this.gateOpenAt) / 1000;
      if (t < tl.exitAt) return { phase: "online", t, hold: 0, exitP: 0, fly: true };
      this.exit = { at: this.gateOpenAt + (tl.exitAt - tl.gate) * 1000, dur: tl.exitDur, fly: true };
    }
    const ex = this.exit!;
    const p = (nowMs - ex.at) / 1000 / ex.dur;
    const t = this.gateOpenAt !== null ? tl.gate + (nowMs - this.gateOpenAt) / 1000 : Math.min(tl.gate, (nowMs - this.startMs) / 1000);
    if (p >= 1) return { phase: "done", t, hold: 0, exitP: 1, fly: ex.fly };
    return { phase: "exit", t, hold: 0, exitP: Math.max(0, p), fly: ex.fly };
  }
}

/** `text` typed out from `start` (s) at `cps` characters per second. */
export function typed(text: string, t: number, start: number, cps = 38): string {
  if (t <= start) return "";
  const n = Math.floor((t - start) * cps);
  return n >= text.length ? text : text.slice(0, n);
}

/** Smoothstep-ish ease (0..1). */
export function ease(x: number): number {
  const c = Math.min(1, Math.max(0, x));
  return c * c * (3 - 2 * c);
}

/** Progress of a `[start, start + dur]` window at time `t`, 0..1. */
export function span(t: number, start: number, dur: number): number {
  return dur <= 0 ? (t >= start ? 1 : 0) : Math.min(1, Math.max(0, (t - start) / dur));
}
