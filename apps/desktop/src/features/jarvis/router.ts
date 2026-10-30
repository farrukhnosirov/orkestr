/**
 * Jarvis routing (ADR-0049) for an utterance that is not an Orkestr intent
 * (voice) or anything typed in the Assistant surface:
 *
 *   0. Jarvis actions from the template bank — «режим разговора», «режим
 *      команд», «забудь разговор», «называй меня босс», «замолчи», «повтори».
 *   1. A computer command matched locally → executed, Jarvis reply.
 *   2. Small talk / FAQ matched locally → instant template answer (no tokens).
 *   3. Conversation mode → AI conversation (streamed, spoken sentence by
 *      sentence); a command the model hands off (`<<run: …>>`) still runs.
 *   4. Command mode → the AI planner as before (Assistant UI); in the
 *      Orchestra UI the utterance goes back to the Orkestr pipeline
 *      (Manager), `null`.
 *
 * Every side effect goes through `JarvisRouteDeps` (unit-tested with fakes).
 */
import type { JarvisAddress, JarvisTemplateReply } from "@orkestr/protocol";

import type { AssistantRun } from "../assistant/api";
import type { ChatReply } from "./api";
import { jarvisSay } from "./persona";
import { SentenceSpeaker, visibleText } from "./speech";
import { type ChatTone, type ConversationMode, useJarvisStore } from "./store";

/** Score of an accepted computer command (core `MATCH_THRESHOLD`). */
export const COMMAND_MATCH = 0.74;
/** A template action beats a command match from this score on. */
export const ACTION_SCORE = 0.85;
/** A near miss of a computer command (core `NEAR_MISS_THRESHOLD`): the
 * assistant asks «Вы имели в виду: …?» instead of chatting / planning. */
export const NEAR_MATCH = 0.6;

export interface JarvisRouteDeps {
  matchTemplate: (text: string) => Promise<JarvisTemplateReply | null>;
  /** Best computer command score (0 when none / unavailable). */
  commandScore: (text: string) => Promise<number>;
  /** Runs an utterance as a computer command (`planUnknown`: AI plan when no command matches). */
  runCommand: (text: string, planUnknown: boolean) => Promise<AssistantRun>;
  chat: (text: string, o: { spoken: boolean; onDelta: (delta: string, reset: boolean) => void }) => Promise<ChatReply>;
  record: (user: string, reply: string, source: "template" | "command") => Promise<void>;
  forget: () => Promise<void>;
  setAddress: (address: JarvisAddress) => Promise<void>;
  stopSpeaking: () => void;
}

export interface JarvisRouteOptions {
  source: "voice" | "text";
  /** Orchestra (main UI) or Assistant (compact) mode. */
  uiMode: "orchestra" | "assistant";
  /** Conversation mode (default: the store's). */
  mode?: ConversationMode;
  /** Speaks one sentence (TTS); when set, AI replies are spoken while streaming. */
  speak?: (sentence: string) => Promise<void>;
  /** The reply so far (streaming HUD / circle). */
  onPartial?: (text: string) => void;
  /** `false`: the speech recognizer was unsure (ADR-0057) — free text that
   * nothing local matched must not reach an AI; the turn is `unheard`. */
  allowFreeText?: boolean;
}

export type JarvisTurnKind = "action" | "command" | "template" | "ai" | "plan" | "unheard";

/** The result of a routed utterance. */
export interface JarvisTurn {
  kind: JarvisTurnKind;
  reply: string;
  tone: ChatTone;
  /** The reply was already spoken (streamed TTS) or must stay silent («замолчи»). */
  spoken: boolean;
  /** The computer assistant's run (commands, plans, hand-offs). */
  run?: AssistantRun;
  /** Template intent / action. */
  intent?: string;
  action?: string;
  /** Command handed off by the AI. */
  command?: string | null;
}

let entrySeq = 0;
function entryId(): string {
  entrySeq += 1;
  return `e-${Date.now().toString(36)}-${entrySeq}`;
}

function show(user: string, reply: string, source: "template" | "ai" | "command", tone: ChatTone = "info"): void {
  const s = useJarvisStore.getState();
  s.push({ id: entryId(), role: "user", text: user, source: "user" });
  s.push({ id: entryId(), role: "assistant", text: reply, source, tone });
  s.patch({ lastReply: reply });
}

const ADDRESS: Record<string, JarvisAddress> = { "address.sir": "Sir", "address.boss": "Boss", "address.none": "None" };

async function applyAction(text: string, tpl: JarvisTemplateReply, deps: JarvisRouteDeps): Promise<JarvisTurn> {
  const store = useJarvisStore.getState();
  const action = tpl.action ?? "";
  let reply = tpl.text;
  let spoken = false;
  switch (action) {
    case "mode.chat":
      store.setMode("chat");
      break;
    case "mode.commands":
      store.setMode("commands");
      break;
    case "memory.forget":
      await deps.forget().catch(() => undefined);
      break;
    case "speech.stop":
      deps.stopSpeaking();
      spoken = true; // silence is the answer
      break;
    case "repeat":
      if (!store.lastReply) reply = jarvisSay("nothingToRepeat");
      break;
    default:
      if (ADDRESS[action]) await deps.setAddress(ADDRESS[action]).catch(() => undefined);
  }
  const s = useJarvisStore.getState();
  s.push({ id: entryId(), role: "user", text, source: "user" });
  s.push({ id: entryId(), role: "assistant", text: reply, source: "template", tone: "info" });
  if (action !== "repeat") s.patch({ lastReply: reply });
  return { kind: "action", reply, tone: "info", spoken, intent: tpl.intent, action };
}

function isNoModel(e: unknown): boolean {
  const msg = e instanceof Error ? e.message : String(e ?? "");
  return /no chat model|connect a provider/i.test(msg);
}

function errorText(e: unknown): string {
  return (e instanceof Error ? e.message : String(e ?? "")).replace(/^model call failed:\s*/i, "").slice(0, 200);
}

async function aiTurn(text: string, o: JarvisRouteOptions, deps: JarvisRouteDeps): Promise<JarvisTurn> {
  const store = useJarvisStore.getState();
  const userId = entryId();
  const replyId = entryId();
  store.push({ id: userId, role: "user", text, source: "user" });
  store.push({ id: replyId, role: "assistant", text: "", source: "ai", streaming: true });
  const speaker = o.speak ? new SentenceSpeaker(o.speak) : null;
  let raw = "";
  try {
    const r = await deps.chat(text, {
      spoken: o.source === "voice",
      onDelta: (delta, reset) => {
        raw = reset ? "" : raw + delta;
        speaker?.push(delta, reset);
        const shown = visibleText(raw);
        useJarvisStore.getState().update(replyId, { text: shown });
        o.onPartial?.(shown);
      },
    });
    let reply = r.text || jarvisSay("handoff");
    let run: AssistantRun | undefined;
    if (r.command) {
      run = await deps.runCommand(r.command, true);
      if (run.outcome !== "ok") reply = `${reply} ${run.reply}`.trim();
    }
    useJarvisStore.getState().update(replyId, { text: reply, streaming: false, tone: run && run.outcome !== "ok" ? "warn" : "info" });
    useJarvisStore.getState().patch({ lastReply: reply });
    o.onPartial?.(reply);
    let spoken = false;
    if (speaker) {
      // the streamed sentences are being spoken; say the rest (and a failed hand-off)
      if (!raw && r.text) speaker.push(r.text);
      if (run && run.outcome !== "ok") speaker.push(` ${run.reply}`);
      await speaker.finish();
      spoken = speaker.spoken.length > 0;
    }
    return { kind: "ai", reply, tone: run && run.outcome !== "ok" ? "warn" : "info", spoken, run, command: r.command };
  } catch (e) {
    speaker?.cancel();
    const reply = isNoModel(e) ? jarvisSay("noModel") : jarvisSay("aiFailed", { error: errorText(e) });
    useJarvisStore.getState().update(replyId, { text: reply, streaming: false, tone: "error" });
    useJarvisStore.getState().patch({ lastReply: reply });
    return { kind: "ai", reply, tone: "error", spoken: false };
  }
}

/**
 * Routes an utterance (see module docs). Returns `null` only in the
 * Orchestra UI in command mode when nothing local matched (the Orkestr
 * pipeline / Manager takes it).
 */
export async function routeJarvis(text: string, o: JarvisRouteOptions, deps: JarvisRouteDeps): Promise<JarvisTurn | null> {
  const clean = text.trim();
  if (!clean) return null;
  const mode = o.mode ?? useJarvisStore.getState().mode;
  const [tpl, score] = await Promise.all([deps.matchTemplate(clean).catch(() => null), deps.commandScore(clean).catch(() => 0)]);

  // 0. Jarvis actions
  if (tpl?.action && (tpl.score >= ACTION_SCORE || score < COMMAND_MATCH)) return applyAction(clean, tpl, deps);

  // 1. computer command
  if (score >= COMMAND_MATCH) {
    const run = await deps.runCommand(clean, false);
    const tone: ChatTone = run.outcome === "ok" ? "ok" : run.outcome === "failed" ? "error" : "warn";
    if (run.reply) {
      show(clean, run.reply, "command", tone);
      if (mode === "chat" && run.outcome === "ok") void deps.record(clean, run.reply, "command");
    }
    return { kind: "command", reply: run.reply, tone, spoken: false, run };
  }

  // 2. small talk from the local bank
  if (tpl) {
    show(clean, tpl.text, "template");
    void deps.record(clean, tpl.text, "template");
    return { kind: "template", reply: tpl.text, tone: "info", spoken: false, intent: tpl.intent };
  }

  // 2b. near miss of a computer command (speech-recognition noise): «Вы имели в виду …?»
  if (score >= NEAR_MATCH) {
    const run = await deps.runCommand(clean, false);
    if (run.outcome !== "notUnderstood") {
      const tone: ChatTone = run.outcome === "ok" ? "ok" : run.outcome === "failed" ? "error" : "warn";
      if (run.reply) show(clean, run.reply, "command", tone);
      return { kind: "command", reply: run.reply, tone, spoken: false, run };
    }
  }

  // Uncertain speech (noise, music, TV) never reaches the AI or the Manager (ADR-0057).
  if (o.allowFreeText === false) return { kind: "unheard", reply: "", tone: "info", spoken: true };

  // 3. conversation with the AI
  if (mode === "chat") return aiTurn(clean, o, deps);

  // 4. command mode: the AI planner (Assistant UI) or the Orkestr pipeline
  if (o.uiMode !== "assistant") return null;
  const run = await deps.runCommand(clean, true);
  const tone: ChatTone = run.outcome === "ok" ? "ok" : run.outcome === "failed" ? "error" : "warn";
  if (run.reply) show(clean, run.reply, "command", tone);
  return { kind: "plan", reply: run.reply, tone, spoken: false, run };
}
