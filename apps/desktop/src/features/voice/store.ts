/**
 * Voice state (zustand): the core-owned configuration (`voice.getConfig` /
 * `voice.setConfig` / `voice.setConsent`) and the runtime state of the
 * command pipeline that the HUD, the mic indicator and Settings render.
 * Business data comes from the core over RPC only; wake-word templates
 * (derived from the user's voice) stay on this device (`wake/spotter.ts`).
 */
import type { SttEngine, VoiceCloudProvider, VoiceCommandSource, VoiceConfig, VoiceConsentScope } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import type { CommandOutcome, ReplyTone } from "./pipeline/executor";

/** Pipeline phase shown by the HUD. */
export type VoicePhase = "idle" | "listening" | "transcribing" | "thinking" | "confirm" | "result" | "error";

/** Why the microphone is open (the indicator names it). */
export type MicPurpose = "wake" | "command" | "enroll" | "meter";

/** Wake-word listener status. */
export type WakeStatus = "off" | "needsEnrollment" | "starting" | "listening" | "pausedBattery" | "pausedBusy" | "error";

export interface VoiceReply {
  text: string;
  tone: ReplyTone;
  outcome: CommandOutcome;
}

export interface HistoryItem {
  id: number;
  at: number;
  transcript: string;
  reply: string;
  tone: ReplyTone;
  source: VoiceCommandSource;
}

export interface VoiceState {
  config: VoiceConfig | null;
  cloudProviders: VoiceCloudProvider[];
  loading: boolean;
  saving: boolean;
  error: string | null;

  hudOpen: boolean;
  phase: VoicePhase;
  source: VoiceCommandSource | null;
  transcript: string;
  reply: VoiceReply | null;
  /** Confirmation question while `phase === "confirm"`. */
  confirmText: string | null;
  speaking: boolean;
  /** Open mic purposes (several can overlap: wake listening + command). */
  mic: MicPurpose[];
  /** 0..1 input level (smoothed) for meters. */
  level: number;
  wakeStatus: WakeStatus;
  wakeError: string | null;
  /** Measured detector load, percent of one core. */
  wakeLoadPct: number;
  history: HistoryItem[];

  load: () => Promise<void>;
  save: (next: VoiceConfig) => Promise<boolean>;
  patch: (fn: (c: VoiceConfig) => VoiceConfig) => Promise<boolean>;
  setConsent: (scope: VoiceConsentScope, granted: boolean, engine?: SttEngine) => Promise<boolean>;
  setHudOpen: (open: boolean) => void;
  setRuntime: (patch: Partial<Pick<VoiceState, "phase" | "source" | "transcript" | "reply" | "confirmText" | "speaking" | "level" | "wakeStatus" | "wakeError" | "wakeLoadPct">>) => void;
  micOn: (p: MicPurpose) => void;
  micOff: (p: MicPurpose) => void;
  pushHistory: (item: Omit<HistoryItem, "id" | "at">) => void;
}

let historySeq = 0;

export const useVoiceStore = create<VoiceState>()((set, get) => ({
  config: null,
  cloudProviders: [],
  loading: false,
  saving: false,
  error: null,

  hudOpen: false,
  phase: "idle",
  source: null,
  transcript: "",
  reply: null,
  confirmText: null,
  speaking: false,
  mic: [],
  level: 0,
  wakeStatus: "off",
  wakeError: null,
  wakeLoadPct: 0,
  history: [],

  load: async () => {
    const client = maybeClient();
    if (!client) return;
    set({ loading: true, error: null });
    try {
      const r = await client.call("voice.getConfig", {});
      set({ config: r.config, cloudProviders: r.cloudProviders, loading: false });
    } catch (e) {
      set({ loading: false, error: describeError(e) });
    }
  },

  save: async (next) => {
    const client = maybeClient();
    if (!client) return false;
    const prev = get().config;
    set({ config: next, saving: true, error: null }); // optimistic
    try {
      const r = await client.call("voice.setConfig", { config: next });
      set({ config: r.config, cloudProviders: r.cloudProviders, saving: false });
      return true;
    } catch (e) {
      set({ config: prev, saving: false, error: describeError(e) });
      return false;
    }
  },

  patch: async (fn) => {
    const c = get().config;
    return c ? get().save(fn(c)) : false;
  },

  setConsent: async (scope, granted, engine) => {
    const client = maybeClient();
    if (!client) return false;
    set({ saving: true, error: null });
    try {
      const r = await client.call("voice.setConsent", { scope, granted, ...(engine ? { engine } : {}) });
      set({ config: r.config, cloudProviders: r.cloudProviders, saving: false });
      return true;
    } catch (e) {
      set({ saving: false, error: describeError(e) });
      return false;
    }
  },

  setHudOpen: (open) => set({ hudOpen: open }),
  setRuntime: (patch) => set(patch),
  micOn: (p) => set((s) => (s.mic.includes(p) ? s : { mic: [...s.mic, p] })),
  micOff: (p) => set((s) => ({ mic: s.mic.filter((x) => x !== p), level: s.mic.length <= 1 ? 0 : s.level })),
  pushHistory: (item) => {
    historySeq += 1;
    set((s) => ({ history: [{ ...item, id: historySeq, at: Date.now() }, ...s.history].slice(0, 20) }));
  },
}));
