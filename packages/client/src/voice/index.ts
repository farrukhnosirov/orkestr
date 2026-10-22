/** Shared voice intent grammar (desktop + mobile; ADR-0017, ADR-0032). */
export { intentId, needsConfirmation, parseConfirmation, parseIntent } from "./parser";
export type { Intent, IntentContext, IntentKind, PanelTarget, ProjectRef } from "./parser";
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
} from "./text";
export type { EntityMatch, Named } from "./text";
