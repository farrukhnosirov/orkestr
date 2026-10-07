/**
 * Requests to show something in the Memory panel from elsewhere (global
 * search): the tab, a search query and the item to expand. The panel
 * switches tab; the list on that tab applies the query and clears the request.
 */
import { create } from "zustand";

export type MemoryTab = "project" | "global" | "decisions" | "knowledge";

export interface MemoryFocus {
  tab: MemoryTab;
  query: string;
  /** Memory item to expand (memory tabs). */
  expand: string | null;
}

interface MemoryFocusState {
  focus: MemoryFocus | null;
  request: (focus: MemoryFocus) => void;
  clear: () => void;
}

export const useMemoryFocus = create<MemoryFocusState>()((set) => ({
  focus: null,
  request: (focus) => set({ focus }),
  clear: () => set({ focus: null }),
}));
