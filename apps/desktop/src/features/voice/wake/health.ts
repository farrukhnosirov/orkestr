/**
 * Wake listener health (ADR-0056): the background listener must never die
 * silently. The controller feeds this pure state machine with heartbeats
 * (every message from the microphone worklet), the listener status and the
 * pipeline phase; `check()` (every couple of seconds) answers what to repair:
 *
 * - **audio stall** — the listener runs but no audio arrived for a while
 *   (device unplugged, sleep/resume, the audio graph suspended) → restart;
 * - **error** — opening the microphone failed → retry with backoff;
 * - **stuck phase** — the pipeline stayed in one phase far longer than it
 *   can (a recognition or a reply that never settled) → reset to idle, so the
 *   wake word is heard again;
 * - **resume** — the clock jumped (the computer slept) → restart.
 */
import type { VoicePhase, WakeStatus } from "../store";

/** No audio for this long while listening = the stream is dead. */
export const AUDIO_STALL_MS = 6000;
/** A gap between checks this long means the computer slept (or the page was frozen). */
export const RESUME_GAP_MS = 45_000;
/** Retry delays after failed restarts (the last one repeats). */
export const RESTART_BACKOFF_MS: readonly number[] = [1000, 3000, 10_000, 30_000, 60_000];
/** Longest a phase may last before it is considered stuck, ms. */
export const PHASE_LIMIT_MS: Readonly<Partial<Record<VoicePhase, number>>> = {
  listening: 30_000, // a capture ends on silence or after 15 s at most
  transcribing: 75_000, // speech recognition has a 60 s timeout
  confirm: 120_000,
  result: 180_000,
  error: 60_000,
  thinking: 300_000, // AI replies and computer commands may work a while
};

export type RestartReason = "stall" | "error" | "resume" | "device" | "ended";
export interface RestartAction {
  type: "restartWake";
  reason: RestartReason;
}
export type HealthAction = RestartAction | { type: "resetPhase"; phase: VoicePhase; stuckMs: number };

export interface HealthSnapshot {
  /** The listener should run (enabled, enrolled, not paused). */
  wanted: boolean;
  status: WakeStatus;
  phase: VoicePhase;
}

export class WakeHealth {
  private lastAudio = 0;
  private lastCheck = 0;
  private phase: VoicePhase = "idle";
  private phaseSince = 0;
  private failures = 0;
  private nextRetryAt = 0;
  private pending: RestartAction | null = null;
  private status: WakeStatus = "off";
  private statusSince = 0;
  /** Restarts so far (shown in the debug state). */
  restarts = 0;
  lastReason: string | null = null;

  /** A message from the microphone worklet. */
  beat(now: number): void {
    this.lastAudio = now;
  }

  /** The listener (re)started successfully: reset the backoff and the stall clock. */
  started(now: number): void {
    this.failures = 0;
    this.nextRetryAt = 0;
    this.lastAudio = now;
  }

  /** A restart attempt failed (mic error): back off. */
  failed(now: number): void {
    const delay = RESTART_BACKOFF_MS[Math.min(this.failures, RESTART_BACKOFF_MS.length - 1)] ?? 60_000;
    this.failures += 1;
    this.nextRetryAt = now + delay;
  }

  /** An external signal (device change, track ended): restart at the next check. */
  signal(reason: "device" | "ended"): void {
    this.pending = { type: "restartWake", reason };
  }

  /** What to repair now (at most one listener restart and one phase reset). */
  check(now: number, s: HealthSnapshot): HealthAction[] {
    const out: HealthAction[] = [];
    const gap = this.lastCheck > 0 ? now - this.lastCheck : 0;
    this.lastCheck = now;
    // Phase watchdog.
    if (s.phase !== this.phase) {
      this.phase = s.phase;
      this.phaseSince = now;
    } else if (gap >= RESUME_GAP_MS) {
      // Slept: a phase that "lasted" through sleep restarts its clock.
      this.phaseSince = now;
    }
    const limit = PHASE_LIMIT_MS[s.phase];
    if (limit !== undefined && now - this.phaseSince > limit) {
      out.push({ type: "resetPhase", phase: s.phase, stuckMs: now - this.phaseSince });
      this.phaseSince = now;
    }
    // Listener watchdog.
    if (!s.wanted) {
      this.pending = null;
      this.failures = 0;
      return out;
    }
    if (s.status !== this.status) {
      this.status = s.status;
      this.statusSince = now;
    }
    let restart: RestartAction | null = null;
    if (s.status === "starting" || s.status === "restarting") {
      // A start in flight (the microphone opens within 10 s): give it time, then treat it as stalled.
      if (now - this.statusSince > AUDIO_STALL_MS * 3) restart = { type: "restartWake", reason: "stall" };
    } else if (s.status === "error") {
      if (now >= this.nextRetryAt) restart = { type: "restartWake", reason: "error" };
    } else if (gap >= RESUME_GAP_MS) {
      restart = { type: "restartWake", reason: "resume" };
    } else if (this.pending) {
      restart = this.pending;
    } else if (s.status === "off") {
      // Wanted but not running (a sync that never happened or was lost).
      restart = { type: "restartWake", reason: "error" };
    } else if (s.status === "listening" && now - this.lastAudio > AUDIO_STALL_MS) {
      restart = { type: "restartWake", reason: "stall" };
    }
    if (restart) {
      this.pending = null;
      this.restarts += 1;
      this.lastReason = restart.reason;
      // The restarted listener gets fresh clocks.
      this.lastAudio = now;
      this.statusSince = now;
      out.push(restart);
    }
    return out;
  }
}
