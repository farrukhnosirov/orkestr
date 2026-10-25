/**
 * Voice pipeline hook (ADR-0035): decides whether an utterance is a
 * computer command (assistant) or an Orkestr command (orchestrator), and
 * answers the assistant's own confirmation / plan prompts by voice.
 *
 * - A pending risky run: "yes" / "no" confirm or reject it.
 * - A plan under review: "yes" runs it, "no" dismisses it, "исправь …" /
 *   "correct …" re-plans with the correction.
 * - Orchestra mode: Orkestr intents win; utterances the orchestrator did
 *   not understand go to the local matcher (no AI plan — those still reach
 *   the AI Manager as before).
 * - Assistant mode: computer commands win (strong match), Orkestr intents
 *   still work, everything else gets an AI plan.
 *
 * The STT / TTS engines stay in the voice feature; this only routes text.
 */
import { parseConfirmation } from "@orkestr/client";
import type { Intent } from "@orkestr/client";

import { getClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { openCursorPanel, startCursor, useCursorStore } from "../cursor/store";
import { parseCursorTrigger } from "../cursor/trigger";
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

/**
 * Routes an utterance. Returns the assistant's run when it handled the text,
 * `null` when the orchestrator should handle it.
 */
export async function routeUtterance(text: string, intent: Intent, wakePhrases: readonly string[] = []): Promise<AssistantRun | null> {
  // Cursor capture mode (ADR-0050): «возьми управление …» / the task asked for right after it.
  const cursor = await routeCursor(text);
  if (cursor) return cursor;
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
  if (!assistantMode) {
    if (!isWeakIntent(intent)) return null;
    const score = await bestScore(text);
    if (score < MATCH) return null;
    return runAssistant(text, "voice", false);
  }
  if (!isWeakIntent(intent)) {
    const score = await bestScore(text);
    if (score < STRONG_MATCH) return null;
  }
  return runAssistant(text, "voice", true);
}

/** «Джарвис, возьми управление и …» starts cursor mode; without a task the next utterance is the task. */
export async function routeCursor(text: string): Promise<AssistantRun | null> {
  const cs = useCursorStore.getState();
  let task = parseCursorTrigger(text);
  if (task === null && cs.awaitingTask && !cs.active) task = text.trim();
  if (task === null) return null;
  if (!task) {
    openCursorPanel();
    useCursorStore.getState().patch({ awaitingTask: true });
    return { outcome: "ok", reply: t("cursor:say.whatToDo"), tone: "info", speak: [] };
  }
  openCursorPanel();
  const r = await startCursor(task, "voice");
  if (r?.status === "Started") return { outcome: "ok", reply: "", tone: "ok", speak: [] };
  if (r?.status === "NeedsConsent") return { outcome: "needsConfirmation", reply: t("cursor:say.needsConsent", { provider: r.backend.label }), tone: "warn", speak: [] };
  return { outcome: "failed", reply: useCursorStore.getState().error ?? t("cursor:say.noBackend"), tone: "warn", speak: [] };
}
