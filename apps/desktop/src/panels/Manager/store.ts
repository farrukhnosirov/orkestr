/**
 * View state of the Manager panel (zustand). Business data is NOT stored
 * here — conversations, plans and runs come from the core through the hook
 * in `data.ts`. This holds the mode toggle, the composer prefill requested by
 * other panels (`orkestr:ask-manager`), focus requests and what the commands
 * ("Approve Plan") act on.
 */
import { create } from "zustand";

import { ASK_MANAGER_EVENT } from "../../features/files/actions";
import type { AskManagerRequest } from "./model";

export type ManagerMode = "natural" | "expert";

const MODE_KEY = "orkestr.manager.mode";

function loadMode(): ManagerMode {
  try {
    return window.localStorage.getItem(MODE_KEY) === "expert" ? "expert" : "natural";
  } catch {
    return "natural";
  }
}

export interface ManagerToast {
  id: number;
  tone: "danger" | "success" | "info";
  message: string;
}

export interface ManagerUiState {
  mode: ManagerMode;
  /** A request from another panel, consumed by the composer of `projectId`. */
  prefill: AskManagerRequest | null;
  /** Incremented to ask the composer to take focus. */
  focusNonce: number;
  /** Newest proposed plan of the visible project (for "Approve Plan"). */
  proposedPlan: { projectId: string; planId: string } | null;
  /** Projects whose resume banner was dismissed this session. */
  dismissedResume: string[];
  toasts: ManagerToast[];
  setMode: (mode: ManagerMode) => void;
  setPrefill: (r: AskManagerRequest | null) => void;
  requestFocus: () => void;
  setProposedPlan: (p: { projectId: string; planId: string } | null) => void;
  dismissResume: (projectId: string) => void;
  toast: (tone: ManagerToast["tone"], message: string) => void;
  dismissToast: (id: number) => void;
}

let seq = 0;

export const useManagerUi = create<ManagerUiState>()((set, get) => ({
  mode: loadMode(),
  prefill: null,
  focusNonce: 0,
  proposedPlan: null,
  dismissedResume: [],
  toasts: [],
  setMode: (mode) => {
    try {
      window.localStorage.setItem(MODE_KEY, mode);
    } catch {
      /* storage unavailable: the toggle still works for this session */
    }
    set({ mode });
  },
  setPrefill: (prefill) => set({ prefill }),
  requestFocus: () => set({ focusNonce: get().focusNonce + 1 }),
  setProposedPlan: (proposedPlan) => {
    const cur = get().proposedPlan;
    if (cur?.planId === proposedPlan?.planId && cur?.projectId === proposedPlan?.projectId) return;
    set({ proposedPlan });
  },
  dismissResume: (projectId) => set({ dismissedResume: [...get().dismissedResume, projectId] }),
  toast: (tone, message) => {
    const id = ++seq;
    set({ toasts: [...get().toasts, { id, tone, message }].slice(-4) });
    window.setTimeout(() => get().dismissToast(id), tone === "danger" ? 6000 : 3500);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
}));

function isAskRequest(v: unknown): v is AskManagerRequest {
  return typeof v === "object" && v !== null && typeof (v as { text?: unknown }).text === "string";
}

/**
 * The Code panel dispatches `orkestr:ask-manager` before the Manager panel
 * may be mounted, so the request is captured here (module scope) and kept
 * until the composer consumes it.
 */
if (typeof window !== "undefined") {
  window.addEventListener(ASK_MANAGER_EVENT, (e: Event) => {
    const detail = (e as CustomEvent<unknown>).detail;
    if (!isAskRequest(detail)) return;
    const ui = useManagerUi.getState();
    ui.setPrefill(detail);
    ui.requestFocus();
  });
}
