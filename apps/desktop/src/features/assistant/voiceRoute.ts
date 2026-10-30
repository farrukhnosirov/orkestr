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
import { currentLanguage, t } from "../../i18n";
import { openCursorPanel, startCursor, useCursorStore } from "../cursor/store";
import { parseCursorTrigger } from "../cursor/trigger";
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
  /** Speech-recognition confidence 0..1 (`null`: the engine reports none, typed text: omit). */
  confidence?: number | null;
  /** The utterance came in the follow-up window without a wake word (ADR-0054): free text must be addressed. */
  followUp?: boolean;
}

/** Below this recognizer confidence free text never reaches an AI / the Manager (ADR-0057). */
export const FREE_TEXT_MIN_CONFIDENCE = 0.5;
/** Very short free text («смещая музыка») needs this much — noise turns into one or two words. */
export const SHORT_TEXT_MIN_CONFIDENCE = 0.8;
/** Unaddressed statements in the follow-up window need this much (TV, people talking nearby). */
export const FOLLOW_UP_MIN_CONFIDENCE = 0.9;
/** Assumed confidence of engines that report none (Vosk, sherpa, cloud). */
const UNKNOWN_CONFIDENCE = 0.6;

const REQUEST = /^(?:(?:а|и|ну|так|слушай|скажи-ка|and|so|ok|okay)[\s,]+)?(?:что|чем|чего|как|какой|какая|какое|какие|каков|сколько|где|куда|откуда|когда|почему|зачем|кто|кого|кому|чей|чья|чьё|ли|можешь|можно|могу|расскажи|объясни|подскажи|скажи|напомни|найди|покажи|придумай|переведи|посчитай|напиши|сделай|давай|помоги|what|how|why|when|where|who|whom|which|whose|can|could|would|will|is|are|do|does|tell|explain|show|find|write|help|please)(?=[\s,.!?]|$)/i;

/**
 * Whether recognized free text (nothing local matched it) may go to an AI /
 * the Manager (ADR-0057). Noise turned into words is short and uncertain;
 * a real request is either clear (confidence) or long enough / phrased as a
 * question or a request. In the follow-up window (no wake word) a TV or a
 * conversation nearby produces long, confident sentences: there free text
 * must be addressed (wake phrase), a question / request, or very clear.
 */
export function freeTextAllowed(text: string, confidence: number | null | undefined, wakePhrases: readonly string[] = [], followUp = false): boolean {
  if (confidence === undefined) return true; // typed text
  const c = confidence ?? UNKNOWN_CONFIDENCE;
  if (c < FREE_TEXT_MIN_CONFIDENCE) return false;
  let clean = text.trim().toLowerCase();
  let addressed = false;
  for (const w of wakePhrases) {
    const p = w.trim().toLowerCase();
    if (p && clean.startsWith(p)) {
      clean = clean.slice(p.length);
      addressed = true;
    }
  }
  clean = clean.replace(/^[\s,.!?:;—-]+/, "");
  const words = clean.split(/[\s,.!?:;—-]+/).filter(Boolean);
  if (words.length === 0) return false;
  const asks = clean.endsWith("?") || REQUEST.test(clean);
  if (words.length <= 2 && !asks && c < SHORT_TEXT_MIN_CONFIDENCE) return false;
  if (followUp && !addressed && !asks && c < FOLLOW_UP_MIN_CONFIDENCE) return false;
  return true;
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
  if (!isWeakIntent(intent)) {
    // an Orkestr intent wins unless the Assistant UI has a strong computer command
    if (!assistantMode) return null;
    const score = await bestScore(text);
    if (score < STRONG_MATCH) return null;
    return runAssistant(text, "voice", false);
  }
  const allowFreeText = freeTextAllowed(text, o.confidence, wakePhrases, o.followUp ?? false);
  const turn = await jarvisSession.send(text, {
    source: "voice",
    ...(o.speak ? { speak: o.speak } : {}),
    ...(o.onPartial ? { onPartial: o.onPartial } : {}),
    ...(allowFreeText ? {} : { allowFreeText: false }),
  });
  if (turn?.kind === "unheard") return { outcome: "cancelled", reply: "", tone: "info", speak: [], spoken: true, unheard: true };
  return turn ? turnToRun(turn) : null;
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
