/**
 * Voice pipeline hook (ADR-0035): decides whether an utterance is a
 * computer command (assistant) or an Orkestr command (orchestrator), and
 * answers the assistant's own confirmation / plan prompts by voice.
 *
 * - A pending risky run: "yes" / "no" confirm or reject it.
 * - A plan under review: "yes" runs it, "no" dismisses it, "исправь …" /
 *   "correct …" re-plans with the correction.
 * - Orchestra mode: Orkestr intents win; utterances the orchestrator did
 *   not understand go through Jarvis (ADR-0049): Jarvis actions, the local
 *   command matcher, the small-talk bank, the AI conversation in
 *   conversation mode — otherwise they reach the AI Manager as before.
 * - Assistant mode: computer commands win (strong match), Orkestr intents
 *   still work, everything else goes through Jarvis (command mode: an AI
 *   plan as before).
 *
 * The STT / TTS engines stay in the voice feature; this only routes text.
 */
import { parseConfirmation } from "@orkestr/client";
import type { Intent } from "@orkestr/client";

import { getClient } from "../../core/client";
import { currentLanguage } from "../../i18n";
import { jarvisSession } from "../jarvis/session";
import type { JarvisTurn } from "../jarvis/router";
import { type AssistantRun, approvePlan, correctPlan, confirmPending, dismissPlan, runAssistant } from "./api";
import { useAssistantStore } from "./store";

/** Score above which a computer command beats an Orkestr intent in Assistant mode. */
export const STRONG_MATCH = 0.9;
/** Score of an accepted computer command (core `MATCH_THRESHOLD`). */
export const MATCH = 0.74;

const CORRECTION = /^(?:исправь|измени|поправь|нет,|не так,|лучше|correct(?: it)?|change(?: it)?|instead|no,)\s*(.+)$/i;

/** Orkestr intents that did not really resolve to anything. */
export function isWeakIntent(intent: Intent): boolean {
  switch (intent.kind) {
    case "askManager":
    case "empty":
      return true;
    case "switchProject":
    case "stopProject":
    case "runQa":
    case "projectStatus":
      return intent.project === null;
    case "agentStatus":
      return intent.agent === null;
    default:
      return false;
  }
}

async function bestScore(text: string): Promise<number> {
  try {
    const r = await getClient().call("computer.match", { text, lang: currentLanguage(), limit: 1 });
    return r.matches[0]?.score ?? 0;
  } catch {
    return 0;
  }
}

/** A Jarvis turn as the assistant's run (voice pipeline result). */
export function turnToRun(turn: JarvisTurn): AssistantRun {
  if (turn.run) return { ...turn.run, reply: turn.reply, spoken: turn.spoken };
  return { outcome: turn.tone === "error" ? "failed" : "ok", reply: turn.reply, tone: turn.tone, speak: [], spoken: turn.spoken };
}

export interface RouteVoiceOptions {
  /** Speaks one sentence (streamed AI replies). */
  speak?: (sentence: string) => Promise<void>;
  /** The reply so far (streaming). */
  onPartial?: (text: string) => void;
}

/**
 * Routes an utterance. Returns the assistant's run when it handled the text,
 * `null` when the orchestrator should handle it.
 */
export async function routeUtterance(
  text: string,
  intent: Intent,
  wakePhrases: readonly string[] = [],
  o: RouteVoiceOptions = {},
): Promise<AssistantRun | null> {
  const s = useAssistantStore.getState();
  const answer = parseConfirmation(text, wakePhrases);
  if (s.pending && answer) return confirmPending(answer === "yes");
  if (s.plan && s.ranPlanId !== s.plan.planId) {
    if (answer === "yes") return approvePlan();
    if (answer === "no") {
      dismissPlan();
      return { outcome: "cancelled", reply: useAssistantStore.getState().reply?.text ?? "", tone: "info", speak: [] };
    }
    const m = CORRECTION.exec(text.trim());
    if (m?.[1]) return correctPlan(m[1]);
  }
  // "запомни это как …" / "remember this as …" right after a plan ran
  if (s.ranPlanId && /^(?:запомни|сохрани|remember|save)/i.test(text.trim())) return runAssistant(text, "voice", false);
  const assistantMode = s.mode === "assistant";
  if (!isWeakIntent(intent)) {
    // an Orkestr intent wins unless the Assistant UI has a strong computer command
    if (!assistantMode) return null;
    const score = await bestScore(text);
    if (score < STRONG_MATCH) return null;
    return runAssistant(text, "voice", false);
  }
  const turn = await jarvisSession.send(text, {
    source: "voice",
    ...(o.speak ? { speak: o.speak } : {}),
    ...(o.onPartial ? { onPartial: o.onPartial } : {}),
  });
  return turn ? turnToRun(turn) : null;
}
