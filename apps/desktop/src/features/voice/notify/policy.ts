/**
 * Voice notifications (§33): which core events deserve a short spoken
 * phrase and a sound cue, and whether one may be spoken right now
 * (master switch, per-event toggles, Do Not Disturb, quiet hours, burst
 * limiting). Pure and unit-tested; `speakers.ts` does the talking.
 */
import type { Event, QuietHours, VoiceNotificationConfig } from "@orkestr/protocol";

import type { SoundCue } from "../../sound/recipes";

/** Event families a user can toggle (fields of `VoiceNotificationConfig`). */
export type VoiceEventKey = "taskCompleted" | "taskFailed" | "approvalNeeded" | "buildFailed" | "budgetThreshold" | "deployFinished";

export const VOICE_EVENT_KEYS: readonly VoiceEventKey[] = [
  "taskCompleted",
  "taskFailed",
  "approvalNeeded",
  "buildFailed",
  "budgetThreshold",
  "deployFinished",
];

/** A notification worth a sound and, maybe, a phrase. */
export interface VoiceNotice {
  /** Source event id (de-duplication). */
  id: string;
  key: VoiceEventKey;
  /** Phrase variant inside the family (`deployFinished` → `ok` / `failed`, `budgetThreshold` → `warn` / `over`). */
  variant: "default" | "ok" | "failed" | "warn" | "over";
  cue: SoundCue;
  /** Critical notices still speak in Do Not Disturb and quiet hours (emergency-grade only). */
  critical: boolean;
  projectId: string | null;
  agentId: string | null;
  /** Interpolation values for the phrase. */
  vars: { percent?: number };
}

/** Maps a core event to a voice notice, or `null`. */
export function noticeFromEvent(ev: Event): VoiceNotice | null {
  const base = { id: ev.id, projectId: ev.projectId, agentId: ev.agentId, critical: false, vars: {} };
  switch (ev.kind) {
    case "TaskStatusChanged":
      if (ev.payload.to === "Completed") return { ...base, key: "taskCompleted", variant: "default", cue: "completion" };
      if (ev.payload.to === "Failed") return { ...base, key: "taskFailed", variant: "default", cue: "error" };
      return null;
    case "ApprovalRequested":
      return { ...base, key: "approvalNeeded", variant: "default", cue: "approval" };
    case "BuildFailed":
      return { ...base, key: "buildFailed", variant: "default", cue: "error" };
    case "BudgetThreshold": {
      const over = ev.payload.fraction >= 1;
      return {
        ...base,
        key: "budgetThreshold",
        variant: over ? "over" : "warn",
        cue: "alert",
        critical: over,
        vars: { percent: Math.round(ev.payload.fraction * 100) },
      };
    }
    case "DeploymentFinished": {
      const s = ev.payload.deployment.status;
      if (s === "Succeeded") return { ...base, key: "deployFinished", variant: "ok", cue: "deploy" };
      if (s === "Failed" || s === "RolledBack") return { ...base, key: "deployFinished", variant: "failed", cue: "error" };
      return null;
    }
    default:
      return null;
  }
}

function minutes(hhmm: string): number | null {
  const m = /^(\d{2}):(\d{2})$/.exec(hhmm);
  if (!m) return null;
  const h = Number(m[1]);
  const mm = Number(m[2]);
  return h < 24 && mm < 60 ? h * 60 + mm : null;
}

/** Whether `now` (local time) falls inside quiet hours; ranges may wrap past midnight. */
export function inQuietHours(q: QuietHours, now: Date): boolean {
  if (!q.enabled) return false;
  const start = minutes(q.start);
  const end = minutes(q.end);
  if (start === null || end === null || start === end) return false;
  const t = now.getHours() * 60 + now.getMinutes();
  return start < end ? t >= start && t < end : t >= start || t < end;
}

export interface SpeakContext {
  config: VoiceNotificationConfig;
  /** Voice output allowed at all (voice enabled, TTS usable). */
  voiceEnabled: boolean;
  doNotDisturb: boolean;
  now: Date;
}

/** Why a notice is (not) spoken — `speak` or the first blocking reason. */
export type SpeakDecision = "speak" | "disabled" | "eventOff" | "dnd" | "quietHours";

export function speakDecision(n: VoiceNotice, ctx: SpeakContext): SpeakDecision {
  if (!ctx.voiceEnabled || !ctx.config.enabled) return "disabled";
  if (!ctx.config[n.key]) return "eventOff";
  if (n.critical) return "speak";
  if (ctx.doNotDisturb) return "dnd";
  if (inQuietHours(ctx.config.quietHours, ctx.now)) return "quietHours";
  return "speak";
}

/**
 * Burst limiter: event storms (ten tasks finishing at once) must not turn
 * into ten sentences. Allows `max` phrases per `windowMs`; the rest are
 * summarized by the caller ("and 7 more").
 */
export class PhraseLimiter {
  private stamps: number[] = [];
  private dropped = 0;
  private readonly seen = new Set<string>();

  constructor(
    private readonly max = 3,
    private readonly windowMs = 20_000,
  ) {}

  /** `true` when the notice may be spoken now. Duplicates (same id) are always refused. */
  admit(id: string, now: number): boolean {
    if (this.seen.has(id)) return false;
    this.seen.add(id);
    if (this.seen.size > 500) this.seen.delete(this.seen.values().next().value as string);
    this.stamps = this.stamps.filter((s) => now - s < this.windowMs);
    if (this.stamps.length >= this.max) {
      this.dropped += 1;
      return false;
    }
    this.stamps.push(now);
    return true;
  }

  /** Number of phrases dropped since the last call (and resets it). */
  takeDropped(): number {
    const n = this.dropped;
    this.dropped = 0;
    return n;
  }
}

/** Phrase id of a notice (key under `voice:notify.phrases`). */
export type PhraseId =
  | "taskCompleted"
  | "taskFailed"
  | "approvalNeeded"
  | "buildFailed"
  | "budgetWarn"
  | "budgetOver"
  | "deployOk"
  | "deployFailed";

export function phraseId(n: VoiceNotice): PhraseId {
  switch (n.key) {
    case "budgetThreshold":
      return n.variant === "over" ? "budgetOver" : "budgetWarn";
    case "deployFinished":
      return n.variant === "failed" ? "deployFailed" : "deployOk";
    default:
      return n.key;
  }
}
