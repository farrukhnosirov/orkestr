/**
 * Voice command pipeline state machine (pure, unit-tested):
 *
 *   idle ──ptt/wake──▶ listening ──release/silence──▶ transcribing ──text──▶ thinking
 *   thinking ──needs confirmation──▶ confirm ──yes──▶ thinking ──done──▶ result
 *   any active ──cancel──▶ idle;  failures ──▶ error;  result/error ──dismiss/timeout──▶ idle
 *
 * A typed command enters at `thinking`. A new push-to-talk or wake word
 * from `result`/`error`/`confirm` starts listening again (in `confirm`, the
 * next utterance is the spoken answer).
 */
import type { VoicePhase } from "../store";

export type MachineEvent =
  | { type: "listen" }
  | { type: "speechEnd" }
  | { type: "transcript"; text: string }
  | { type: "typed"; text: string }
  | { type: "needsConfirm" }
  | { type: "confirmed" }
  | { type: "done" }
  | { type: "fail" }
  | { type: "cancel" }
  | { type: "dismiss" };

/** Next phase, or `null` when the event is not valid in `phase` (ignored). */
export function nextPhase(phase: VoicePhase, ev: MachineEvent): VoicePhase | null {
  switch (ev.type) {
    case "listen":
      return phase === "idle" || phase === "result" || phase === "error" || phase === "confirm" ? "listening" : null;
    case "speechEnd":
      return phase === "listening" ? "transcribing" : null;
    case "transcript":
      return phase === "transcribing" ? "thinking" : null;
    case "typed":
      return phase === "idle" || phase === "result" || phase === "error" || phase === "confirm" ? "thinking" : null;
    case "needsConfirm":
      return phase === "thinking" ? "confirm" : null;
    case "confirmed":
      return phase === "confirm" ? "thinking" : null;
    case "done":
      return phase === "thinking" ? "result" : null;
    case "fail":
      return phase === "idle" ? null : "error";
    case "cancel":
      return phase === "idle" ? null : "idle";
    case "dismiss":
      return phase === "result" || phase === "error" || phase === "confirm" ? "idle" : null;
  }
}

/** Phases in which the microphone is recording a command. */
export function isCapturing(phase: VoicePhase): boolean {
  return phase === "listening";
}

/** Phases in which a new command cannot start (busy). */
export function isBusy(phase: VoicePhase): boolean {
  return phase === "transcribing" || phase === "thinking";
}
