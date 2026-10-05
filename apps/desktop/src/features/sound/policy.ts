/**
 * When the sound engine may play (pure, unit-tested): master switch,
 * volume, Do Not Disturb, quiet hours and the performance mode (§31, §70).
 */
import type { PerformanceMode, SoundConfig } from "@orkestr/protocol";

import type { SoundCue, SoundQuality } from "./recipes";

export interface SoundContext {
  config: SoundConfig;
  doNotDisturb: boolean;
  performanceMode: PerformanceMode;
  /** Explicit test from the settings screen: ignores DND / battery saver. */
  test?: boolean;
}

/** Cues that still play in Do Not Disturb (when muting is on) and in Battery Saver. */
export const CRITICAL_CUES: ReadonlySet<SoundCue> = new Set<SoundCue>(["alert"]);
/** Interaction feedback the user triggered themselves (wake/push-to-talk): kept in Battery Saver. */
const FEEDBACK_CUES: ReadonlySet<SoundCue> = new Set<SoundCue>(["wake", "listenStart", "listenEnd", "error", "approval"]);

export function shouldPlaySound(cue: SoundCue, ctx: SoundContext): boolean {
  const { config } = ctx;
  if (!config.enabled || config.volumePct <= 0) return false;
  if (ctx.test) return true;
  if (ctx.doNotDisturb && config.muteInDnd && !CRITICAL_CUES.has(cue)) return false;
  if (ctx.performanceMode === "BatterySaver" && !CRITICAL_CUES.has(cue) && !FEEDBACK_CUES.has(cue)) return false;
  return true;
}

/** Rendering quality for a performance mode: reverb/partials only when effects are welcome. */
export function soundQuality(mode: PerformanceMode): SoundQuality {
  return mode === "UltraVisual" || mode === "Balanced" ? "full" : "lite";
}

/** Master gain from a 0..100 volume (perceptual curve). */
export function masterGain(volumePct: number): number {
  const v = Math.min(100, Math.max(0, volumePct)) / 100;
  return v * v;
}
