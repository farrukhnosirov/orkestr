/**
 * Jarvis persona & conversation mode (ADR-0049) — public API for other
 * features (voice circle UI, cursor-capture mode, notifications):
 *
 * - `jarvisSay(kind, vars)`: a persona phrase in the UI language, with the
 *   configured form of address, never the same variant twice in a row.
 * - `{{addr}}` in any translation = the form of address (`applyPersona`).
 * - `greeting(now)` / `timeOfDay(now)`: «Доброе утро, сэр.»
 * - `jarvisSession`: `send(text, {source, speak, onPartial})` routes and
 *   answers an utterance (Jarvis actions → computer command → instant
 *   template → AI conversation / AI planner); `mode()`/`setMode()`,
 *   `cancel()`, `forget()`, `entries()`, `subscribe()`.
 * - `useJarvisStore`: mode (`commands` / `chat`), transcript, `streamingId`,
 *   `lastReply` (what the circle UI shows while a reply streams).
 */
export { addressSuffix, addressWord, applyPersona, greeting, jarvisSay, type JarvisSayKind, timeOfDay } from "./persona";
export { type ConversationSession, jarvisSession, type SendOptions } from "./session";
export type { JarvisTurn, JarvisTurnKind } from "./router";
export { type ChatEntry, type ConversationMode, useJarvisStore } from "./store";
export { ChatTranscript, ConversationModeSwitch } from "./ChatPanel";
export { visibleText } from "./speech";
