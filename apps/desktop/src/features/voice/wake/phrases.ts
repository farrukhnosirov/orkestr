/**
 * Wake phrases (ADR-0056): several phrases may wake the assistant
 * («Джарвис», «Эй, Джарвис», «Окей, Джарвис», «Jarvis»). The audio spotter
 * matches the user's recorded samples; this module covers the text side —
 * recognizing a wake phrase in a transcript (to strip it from a command, or
 * to notice that the user called the assistant again), including the usual
 * speech-recognition misspellings («джервис», «жарвис», «дарвис»…). Pure.
 */
import { levenshtein, normalize } from "../intent/text";

/** Phrases offered as one-click additions in Settings. */
export const SUGGESTED_WAKE_PHRASES: readonly string[] = ["Джарвис", "Эй, Джарвис", "Окей, Джарвис", "Jarvis"];
/** Max wake phrases (mirrors `MAX_WAKE_PHRASES` in the protocol). */
export const MAX_WAKE_PHRASES = 8;
/** Max length of one phrase. */
export const MAX_WAKE_PHRASE_LEN = 40;

/** How speech recognition tends to spell «Джарвис» / «Jarvis». */
const JARVIS_SPELLINGS: readonly string[] = [
  "джарвис",
  "джервис",
  "жарвис",
  "дарвис",
  "джарвиз",
  "джарвес",
  "джаврис",
  "чарвис",
  "джарви",
  "джарвисс",
  "jarvis",
  "jervis",
  "javis",
  "jarvi",
];
/** Interchangeable lead-ins. */
const LEAD_GROUPS: readonly (readonly string[])[] = [
  ["эй", "хей", "hey"],
  ["окей", "ок", "ok", "okay"],
];

function isJarvisLike(word: string): boolean {
  if (JARVIS_SPELLINGS.includes(word)) return true;
  if (word.length < 5) return false;
  return levenshtein(word, "джарвис") <= 1 || levenshtein(word, "jarvis") <= 1;
}

/** Spelling alternatives of one word of a wake phrase. */
function wordVariants(word: string): readonly string[] {
  if (isJarvisLike(word)) return JARVIS_SPELLINGS;
  const lead = LEAD_GROUPS.find((g) => g.includes(word));
  return lead ?? [word];
}

/**
 * Every spelling of the configured phrases a transcript may contain
 * (normalized, unique, longest first) — what `stripWakePhrase` /
 * `parseIntent` get as wake phrases.
 */
export function wakeVariants(phrases: readonly string[]): string[] {
  const out = new Set<string>();
  for (const p of phrases) {
    const words = normalize(p).split(" ").filter(Boolean);
    if (words.length === 0) continue;
    let acc: string[] = [""];
    for (const w of words) {
      const next: string[] = [];
      for (const prefix of acc) for (const v of wordVariants(w)) next.push(prefix ? `${prefix} ${v}` : v);
      acc = next.slice(0, 256); // bounded: a phrase has a few words
    }
    acc.forEach((v) => out.add(v));
  }
  return [...out].sort((a, b) => b.length - a.length);
}

export interface WakeMatch {
  /** The spelling that matched (normalized). */
  heard: string;
  /** What follows the wake phrase (normalized; empty when the phrase was said alone). */
  rest: string;
}

/**
 * A wake phrase at the start of `text` («Джарвис, открой ютуб» → rest
 * «открой ютуб»), tolerant to one-letter recognition slips in longer words.
 */
export function matchWakePhrase(text: string, phrases: readonly string[]): WakeMatch | null {
  const norm = normalize(text);
  if (!norm) return null;
  const words = norm.split(" ");
  for (const v of wakeVariants(phrases)) {
    if (norm === v) return { heard: v, rest: "" };
    if (norm.startsWith(`${v} `)) return { heard: v, rest: norm.slice(v.length + 1).trim() };
  }
  // Fuzzy: the leading words match a phrase word by word (≤ 1 edit in words of 5+ letters).
  for (const p of phrases) {
    const pw = normalize(p).split(" ").filter(Boolean);
    if (pw.length === 0 || words.length < pw.length) continue;
    const ok = pw.every((w, i) => {
      const got = words[i] ?? "";
      if (got === w || wordVariants(w).includes(got)) return true;
      return w.length >= 5 && levenshtein(got, w) <= 1;
    });
    if (ok) return { heard: words.slice(0, pw.length).join(" "), rest: words.slice(pw.length).join(" ") };
  }
  return null;
}

/** Cleans a phrase list from Settings: trimmed, ≤ 40 chars, unique (any case), ≤ 8, never empty. */
export function cleanWakePhrases(list: readonly string[], fallback = "Джарвис"): string[] {
  const out: string[] = [];
  for (const raw of list) {
    const p = raw.trim().replace(/\s+/g, " ").slice(0, MAX_WAKE_PHRASE_LEN);
    if (p && !out.some((x) => x.toLowerCase() === p.toLowerCase())) out.push(p);
  }
  if (out.length === 0) out.push(fallback);
  return out.slice(0, MAX_WAKE_PHRASES);
}

/** The phrases of a config (older documents have only `phrase`). */
export function configWakePhrases(wake: { phrase: string; phrases?: readonly string[] | null }): string[] {
  const list = wake.phrases && wake.phrases.length > 0 ? wake.phrases : [wake.phrase];
  return cleanWakePhrases(list, wake.phrase || "Джарвис");
}

/** Storage key of a phrase (case / «ё» / punctuation-insensitive). */
export function phraseKey(phrase: string): string {
  return normalize(phrase);
}
