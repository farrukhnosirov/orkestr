/**
 * The conversation session (ADR-0049) — the API other features use
 * (the Jarvis voice circle, cursor-capture mode, the Assistant surface):
 *
 *   jarvisSession.send("открой ютуб", { source: "voice", speak })
 *   jarvisSession.mode() / setMode("chat")
 *   jarvisSession.cancel() / forget() / entries() / subscribe(fn)
 *
 * `send` runs the full routing (Jarvis actions → computer command → local
 * template → AI conversation → AI planner) and returns the turn.
 */
import { getClient } from "../../core/client";
import { currentLanguage } from "../../i18n";
import { runAssistant } from "../assistant/api";
import { useAssistantStore } from "../assistant/store";
import { stopSpeaking } from "../voice/tts/speaker";
import { cancelChat, chatTurn, forgetConversation, matchTemplate, recordExchange, setAddress } from "./api";
import { type JarvisRouteDeps, type JarvisTurn, routeJarvis } from "./router";
import { type ChatEntry, type ConversationMode, useJarvisStore } from "./store";

/** Real dependencies of the router. */
export function realJarvisDeps(source: "voice" | "text"): JarvisRouteDeps {
  return {
    matchTemplate,
    commandScore: async (text) => {
      const r = await getClient().call("computer.match", { text, lang: currentLanguage(), limit: 1 });
      return r.matches[0]?.score ?? 0;
    },
    runCommand: (text, planUnknown) => runAssistant(text, source, planUnknown),
    chat: (text, o) => chatTurn(text, o),
    record: recordExchange,
    forget: forgetConversation,
    setAddress,
    stopSpeaking,
  };
}

export interface SendOptions {
  source: "voice" | "text";
  /** Speaks one sentence; AI replies are spoken while they stream. */
  speak?: (sentence: string) => Promise<void>;
  /** The reply so far (streaming). */
  onPartial?: (text: string) => void;
  /** Force a conversation mode for this turn. */
  mode?: ConversationMode;
}

export interface ConversationSession {
  /** Routes and answers an utterance. `null`: not handled here (Orchestra UI, command mode, unknown text). */
  send(text: string, o: SendOptions): Promise<JarvisTurn | null>;
  mode(): ConversationMode;
  setMode(mode: ConversationMode): void;
  /** Stops the reply being streamed. */
  cancel(): Promise<void>;
  /** «Забудь разговор». */
  forget(): Promise<void>;
  /** The conversation shown in the UI (oldest first). */
  entries(): readonly ChatEntry[];
  /** Notifies on every change of mode / entries / streaming. */
  subscribe(listener: () => void): () => void;
}

export const jarvisSession: ConversationSession = {
  send: (text, o) =>
    routeJarvis(
      text,
      {
        source: o.source,
        uiMode: useAssistantStore.getState().mode,
        ...(o.mode ? { mode: o.mode } : {}),
        ...(o.speak ? { speak: o.speak } : {}),
        ...(o.onPartial ? { onPartial: o.onPartial } : {}),
      },
      realJarvisDeps(o.source),
    ),
  mode: () => useJarvisStore.getState().mode,
  setMode: (mode) => useJarvisStore.getState().setMode(mode),
  cancel: cancelChat,
  forget: forgetConversation,
  entries: () => useJarvisStore.getState().entries,
  subscribe: (listener) => useJarvisStore.subscribe(() => listener()),
};
