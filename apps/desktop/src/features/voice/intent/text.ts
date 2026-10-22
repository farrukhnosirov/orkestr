/** Moved to `@orkestr/client` (packages/client/src/voice/text.ts) for sharing with mobile. */
export {
  MATCH_THRESHOLD,
  WAKE_ALIASES,
  bestMatch,
  levenshtein,
  normalize,
  phoneticKey,
  similarity,
  stripWakePhrase,
  transliterate,
} from "@orkestr/client";
export type { EntityMatch, Named } from "@orkestr/client";
