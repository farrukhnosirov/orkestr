/**
 * Follow-up policy (ADR-0056, refines ADR-0054): after a reply the assistant
 * listens once, for a short window, without the wake word — like a person
 * who waits a moment for an answer, not one who listens forever.
 *
 * - one window after the assistant finishes speaking (default 6 s);
 * - noise, filler sounds and recognizer hallucinations in the window close it
 *   (no reply, no new window);
 * - only a real command/question that got a real answer re-opens it, at most
 *   `maxTurns` times in a row; then the wake word is needed again.
 * Pure, unit-tested.
 */
import type { CommandOutcome } from "./executor";

/** Sounds and fillers that are not an utterance. */
const FILLERS = new Set([
  "а", "э", "эм", "ээ", "эээ", "м", "мм", "ммм", "хм", "хмм", "ну", "ага", "угу", "ой", "ах", "ох", "о", "и", "да ну",
  "uh", "um", "umm", "hmm", "mm", "oh", "ah", "eh", "you", "the",
]);

/** What speech recognizers "hear" in noise or silence (Whisper in particular). */
const HALLUCINATIONS: readonly string[] = [
  "продолжение следует",
  "субтитры",
  "спасибо за просмотр",
  "подписывайтесь на канал",
  "редактор субтитров",
  "корректор",
  "thank you for watching",
  "thanks for watching",
  "subtitles by",
  "music",
  "музыка",
  "аплодисменты",
  "смех",
];

function words(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/ё/g, "е")
    .replace(/[^\p{L}\p{N}\s]+/gu, " ")
    .split(/\s+/)
    .filter(Boolean);
}

/**
 * Whether a transcript heard in the follow-up window is something the user
 * actually said to the assistant (not noise, a filler or a hallucination).
 * With a pending yes/no question, short answers («да», «нет») count.
 */
export function isMeaningfulUtterance(text: string, o: { expectingAnswer?: boolean } = {}): boolean {
  const w = words(text);
  if (w.length === 0) return false;
  const joined = w.join(" ");
  if (FILLERS.has(joined)) return false;
  if (w.every((x) => FILLERS.has(x))) return false;
  if (HALLUCINATIONS.some((h) => joined === h || (joined.startsWith(h) && w.length <= h.split(" ").length + 3))) return false;
  const letters = joined.replace(/[^\p{L}\p{N}]/gu, "").length;
  return letters >= (o.expectingAnswer ? 2 : 3);
}

/** Outcomes after which the conversation may go on without the wake word. */
const CONTINUES: ReadonlySet<CommandOutcome> = new Set<CommandOutcome>(["ok", "sentToManager", "needsConfirmation"]);

export interface FollowUpTurn {
  /** The turn started from a follow-up window (no wake word). */
  fromFollowUp: boolean;
  outcome: CommandOutcome;
}

/**
 * Counts follow-up turns in a row. A turn started with the wake word or
 * push-to-talk starts a new conversation (the count resets).
 */
export class FollowUpChain {
  private turns = 0;

  get count(): number {
    return this.turns;
  }

  /** The conversation ended (silence, an ending phrase, a stop, cancel). */
  reset(): void {
    this.turns = 0;
  }

  /** A turn finished: may the follow-up window open after its reply? */
  afterTurn(t: FollowUpTurn, maxTurns: number): boolean {
    if (t.fromFollowUp) this.turns += 1;
    else this.turns = 0;
    if (!CONTINUES.has(t.outcome)) return false;
    return this.turns < Math.max(1, maxTurns);
  }
}
