/**
 * Jarvis state (ADR-0049): the persona settings (from the core,
 * `jarvis.getConfig`), the conversation mode (commands / chat, remembered on
 * this device) and the conversation shown in the Assistant surface.
 */
import type { JarvisBankStats, JarvisConfig, JarvisMessage } from "@orkestr/protocol";
import { create } from "zustand";

export type ConversationMode = "commands" | "chat";
export type ChatSource = "user" | "template" | "ai" | "command";
export type ChatTone = "ok" | "info" | "warn" | "error";

/** One line of the conversation. */
export interface ChatEntry {
  id: string;
  role: "user" | "assistant";
  text: string;
  source: ChatSource;
  /** Still streaming. */
  streaming?: boolean;
  tone?: ChatTone;
}

const MODE_KEY = "orkestr.jarvis.mode";
/** Entries kept in memory for the UI. */
export const MAX_ENTRIES = 100;

function loadMode(): ConversationMode {
  try {
    return window.localStorage.getItem(MODE_KEY) === "chat" ? "chat" : "commands";
  } catch {
    return "commands";
  }
}

export interface JarvisState {
  config: JarvisConfig | null;
  bank: JarvisBankStats | null;
  mode: ConversationMode;
  entries: ChatEntry[];
  /** Reply being streamed. */
  streamingId: string | null;
  /** The last thing Jarvis said (for «повтори»). */
  lastReply: string;
  setMode: (mode: ConversationMode) => void;
  setConfig: (config: JarvisConfig, bank?: JarvisBankStats | null) => void;
  setHistory: (messages: JarvisMessage[]) => void;
  push: (entry: ChatEntry) => void;
  update: (id: string, patch: Partial<ChatEntry>) => void;
  clear: () => void;
  patch: (p: Partial<Pick<JarvisState, "streamingId" | "lastReply">>) => void;
}

export const useJarvisStore = create<JarvisState>((set) => ({
  config: null,
  bank: null,
  mode: loadMode(),
  entries: [],
  streamingId: null,
  lastReply: "",
  setMode: (mode) => {
    try {
      window.localStorage.setItem(MODE_KEY, mode);
    } catch {
      /* private mode: not remembered */
    }
    set({ mode });
  },
  setConfig: (config, bank) => set((s) => ({ config, bank: bank === undefined ? s.bank : bank })),
  setHistory: (messages) =>
    set({
      entries: messages.slice(-MAX_ENTRIES).map((m) => ({
        id: m.id,
        role: m.role === "User" ? "user" : "assistant",
        text: m.text,
        source: (["template", "ai", "command"].includes(m.source) ? m.source : "user") as ChatSource,
      })),
    }),
  push: (entry) => set((s) => ({ entries: [...s.entries, entry].slice(-MAX_ENTRIES) })),
  update: (id, patch) => set((s) => ({ entries: s.entries.map((e) => (e.id === id ? { ...e, ...patch } : e)) })),
  clear: () => set({ entries: [], streamingId: null }),
  patch: (p) => set(p),
}));
