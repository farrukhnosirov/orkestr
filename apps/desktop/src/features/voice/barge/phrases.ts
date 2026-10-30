/**
 * Stop phrases and conversation endings on the client side (ADR-0054),
 * matching the core's classifier (`crates/voice/src/barge.rs`): used on
 * finished transcripts — «стоп, открой ютуб» (strip the stop phrase, keep the
 * command), a follow-up that only says «спасибо, всё» (end the
 * conversation), self-echo of the reply that was just spoken. Pure.
 */
import type { BargeInConfig, StopKind } from "@orkestr/protocol";

/** Utterances longer than this are conversation, not a stop command. */
const MAX_WORDS = 5;
/** Words besides the phrase tolerated in a stop utterance («ну хватит уже»). */
const MAX_EXTRA = 2;
const MIN_PREFIX = 4;
const MIN_PREFIX_PCT = 60;

/** Lower-case word tokens (`ё` → `е`, punctuation dropped). */
export function tokens(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/ё/g, "е")
    .split(/\s+/)
    .map((w) => w.replace(/[^\p{L}\p{N}']/gu, ""))
    .filter(Boolean);
}

export interface StopMatch {
  phrase: string;
  kind: StopKind;
}

interface Entry {
  phrase: string;
  words: string[];
  kind: StopKind;
}

function entries(c: Pick<BargeInConfig, "hardPhrases" | "softPhrases">): Entry[] {
  const out: Entry[] = [];
  const add = (list: readonly string[], kind: StopKind) => {
    for (const p of list) {
      const words = tokens(p);
      if (words.length === 0 || out.some((e) => e.words.join(" ") === words.join(" "))) continue;
      out.push({ phrase: p.trim(), words, kind });
    }
  };
  add(c.hardPhrases, "Hard");
  add(c.softPhrases, "Soft");
  return out;
}

function wordMatches(heard: string, word: string, allowPrefix: boolean): boolean {
  if (heard === word) return true;
  if (!allowPrefix) return false;
  return heard.length >= MIN_PREFIX && heard.length < word.length && heard.length * 100 >= word.length * MIN_PREFIX_PCT && word.startsWith(heard);
}

function containsSeq(hay: readonly string[], needle: readonly string[]): boolean {
  if (needle.length === 0 || needle.length > hay.length) return false;
  for (let i = 0; i + needle.length <= hay.length; i++) if (needle.every((w, k) => hay[i + k] === w)) return true;
  return false;
}

/** Where a phrase matches inside `words` (start index), or -1. */
function findAt(words: readonly string[], e: Entry, allowPrefixLast: boolean): number {
  for (let s = 0; s + e.words.length <= words.length; s++) {
    if (e.words.every((w, k) => wordMatches(words[s + k] ?? "", w, allowPrefixLast && s + k === words.length - 1))) return s;
  }
  return -1;
}

/**
 * Whether `heard` is a stop utterance. `spoken` is what the assistant is
 * saying: a phrase contained in it is self-echo and never matches.
 */
export function classifyStop(heard: string, c: Pick<BargeInConfig, "hardPhrases" | "softPhrases">, spoken?: string | null): StopMatch | null {
  const words = tokens(heard);
  if (words.length === 0 || words.length > MAX_WORDS) return null;
  const echo = spoken ? tokens(spoken) : [];
  let best: Entry | null = null;
  for (const e of entries(c)) {
    if (findAt(words, e, true) < 0 || words.length - e.words.length > MAX_EXTRA) continue;
    if (containsSeq(echo, e.words) || containsSeq(echo, words)) continue;
    if (!best || (best.kind === "Soft" && e.kind === "Hard") || (best.kind === e.kind && e.words.length > best.words.length)) best = e;
  }
  return best ? { phrase: best.phrase, kind: best.kind } : null;
}

/**
 * «Стоп, открой ютуб» → `{ stop, rest: "открой ютуб" }`: a stop phrase at the
 * start of an utterance (with up to one filler word before it) and the
 * command after it. `null` when the utterance does not start with one.
 */
export function splitLeadingStop(text: string, c: Pick<BargeInConfig, "hardPhrases" | "softPhrases">): { stop: StopMatch; rest: string } | null {
  const raw = text.trim().split(/\s+/).filter(Boolean);
  const words = raw.map((w) => tokens(w)[0] ?? "");
  let best: { e: Entry; end: number } | null = null;
  for (const e of entries(c)) {
    for (const start of [0, 1]) {
      if (start + e.words.length > words.length) continue;
      if (e.words.every((w, k) => words[start + k] === w)) {
        const end = start + e.words.length;
        if (!best || end > best.end || (end === best.end && best.e.kind === "Soft" && e.kind === "Hard")) best = { e, end };
      }
    }
  }
  if (!best) return null;
  const rest = raw.slice(best.end).join(" ").replace(/^[\s,.;:!?—–-]+/, "").trim();
  return { stop: { phrase: best.e.phrase, kind: best.e.kind }, rest };
}

/** Phrases that end a continuous conversation (ru/en). */
export const END_PHRASES: readonly string[] = [
  "спасибо",
  "спасибо всё",
  "всё спасибо",
  "спасибо это всё",
  "спасибо больше ничего",
  "больше ничего",
  "ничего",
  "ничего не надо",
  "пока",
  "до свидания",
  "на этом всё",
  "это всё",
  "отбой",
  "свободен",
  "можешь идти",
  "thanks",
  "thank you",
  "that's all",
  "thats all",
  "that is all",
  "nothing",
  "nothing else",
  "bye",
  "goodbye",
  "never mind",
];

/** Whether a follow-up utterance only ends the conversation («спасибо, всё», «пока», «that's all»). */
export function isConversationEnd(text: string): boolean {
  const words = tokens(text).filter((w) => w !== "джарвис" && w !== "jarvis" && w !== "сэр" && w !== "sir");
  if (words.length === 0 || words.length > 5) return false;
  const joined = words.join(" ");
  return END_PHRASES.some((p) => tokens(p).join(" ") === joined);
}

/**
 * Self-echo of a reply: what was heard is (almost) entirely words the
 * assistant just said — the microphone picked up the speaker.
 */
export function isEchoOf(heard: string, spoken: string | null | undefined): boolean {
  if (!spoken) return false;
  const h = tokens(heard);
  if (h.length === 0) return false;
  const s = new Set(tokens(spoken));
  const shared = h.filter((w) => s.has(w)).length;
  return h.length >= 2 ? shared / h.length >= 0.8 : containsSeq(tokens(spoken), h) && h[0]!.length > 3;
}
