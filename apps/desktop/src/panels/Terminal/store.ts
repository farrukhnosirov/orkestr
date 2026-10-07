/**
 * UI state of the Terminal panel. Business data (sessions, output) always
 * comes from the core over RPC; this store only keeps view state: the active
 * tab per project and pending "open a terminal" requests from commands.
 */
import { create } from "zustand";

const ACTIVE_KEY = "orkestr.terminal.active";

function readActive(): Record<string, string> {
  try {
    const raw = localStorage.getItem(ACTIVE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const out: Record<string, string> = {};
      for (const [k, v] of Object.entries(parsed)) if (typeof v === "string") out[k] = v;
      return out;
    }
  } catch {
    // Storage unavailable or corrupt: start fresh.
  }
  return {};
}

/** Scope key of the active-tab map (`__none__` when no project is open). */
export function scopeKey(projectId: string | null): string {
  return projectId ?? "__none__";
}

interface TerminalUiState {
  /** Active terminal id per project scope. */
  active: Record<string, string>;
  /** Incremented by the "Open Terminal" command (kept for observers/tests). */
  openRequest: number;
  /**
   * True while an "Open Terminal" request has not been handled by a panel yet.
   * A pending flag (not a counter snapshot) so a request issued before the
   * panel mounts — the dock mounts panel content asynchronously — is not lost.
   */
  pendingOpen: boolean;
  setActive: (projectId: string | null, terminalId: string | null) => void;
  requestOpen: () => void;
  /** Clears the pending request; returns whether one was pending. */
  consumeOpenRequest: () => boolean;
}

export const useTerminalUi = create<TerminalUiState>((set, get) => ({
  active: readActive(),
  openRequest: 0,
  pendingOpen: false,
  setActive: (projectId, terminalId) =>
    set((s) => {
      const active = { ...s.active };
      const key = scopeKey(projectId);
      if (terminalId) active[key] = terminalId;
      else delete active[key];
      try {
        localStorage.setItem(ACTIVE_KEY, JSON.stringify(active));
      } catch {
        // Non-fatal: the active tab simply isn't remembered.
      }
      return { active };
    }),
  requestOpen: () => set((s) => ({ openRequest: s.openRequest + 1, pendingOpen: true })),
  consumeOpenRequest: () => {
    if (!get().pendingOpen) return false;
    set({ pendingOpen: false });
    return true;
  },
}));
