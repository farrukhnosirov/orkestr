/**
 * Computer assistant state (ADR-0035): the UI mode (Orchestra / Assistant,
 * remembered on this device), the last command, the reply, a pending
 * confirmation, an AI plan under review and lists the last command showed.
 * Business data (catalogue, sets, history) comes from the core (`computer.*`).
 */
import type { ComputerStatus, HistoryEntry, PendingConfirmation, PlanProposal } from "@orkestr/protocol";
import { create } from "zustand";

export type AssistantUiMode = "orchestra" | "assistant";
export type AssistantTone = "ok" | "info" | "warn" | "error";

export interface AssistantReply {
  text: string;
  tone: AssistantTone;
}

export interface ShownList {
  title: string;
  lines: string[];
}

const MODE_KEY = "orkestr.assistant.mode";

function loadMode(): AssistantUiMode {
  try {
    return window.localStorage.getItem(MODE_KEY) === "assistant" ? "assistant" : "orchestra";
  } catch {
    return "orchestra";
  }
}

export interface AssistantState {
  mode: AssistantUiMode;
  busy: boolean;
  /** Last utterance / typed command. */
  input: string;
  reply: AssistantReply | null;
  pending: PendingConfirmation | null;
  plan: PlanProposal | null;
  /** Plan id that ran successfully (can be remembered). */
  ranPlanId: string | null;
  shown: ShownList | null;
  history: HistoryEntry[];
  status: ComputerStatus | null;
  setMode: (mode: AssistantUiMode) => void;
  toggleMode: () => void;
  patch: (p: Partial<Omit<AssistantState, "setMode" | "toggleMode" | "patch">>) => void;
}

export const useAssistantStore = create<AssistantState>((set, get) => ({
  mode: loadMode(),
  busy: false,
  input: "",
  reply: null,
  pending: null,
  plan: null,
  ranPlanId: null,
  shown: null,
  history: [],
  status: null,
  setMode: (mode) => {
    try {
      window.localStorage.setItem(MODE_KEY, mode);
    } catch {
      /* private mode: not remembered */
    }
    set({ mode });
  },
  toggleMode: () => get().setMode(get().mode === "assistant" ? "orchestra" : "assistant"),
  patch: (p) => set(p),
}));
