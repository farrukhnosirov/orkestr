/**
 * Layout feature view state (zustand). Layout documents themselves live in
 * the core per device (`ui.layouts*`); this mirrors the last `LayoutsState`
 * plus editor/window state of this webview.
 */
import type { LayoutsState } from "@orkestr/protocol";
import { create } from "zustand";

import type { MonitorInfo, WindowInfo } from "./selection";

export type LayoutDialog = { kind: "saveAs" } | { kind: "rename"; id: string; name: string } | { kind: "import" } | null;

export interface LayoutStore {
  /** Last state from the core (null until loaded or when the core lacks `ui.layouts*`). */
  state: LayoutsState | null;
  /** Layout currently applied in this window. */
  activeId: string;
  loaded: boolean;
  /** Persisting to the core failed (the layout still works for this session). */
  persistError: string | null;
  locked: boolean;
  editorOpen: boolean;
  dialog: LayoutDialog;
  monitors: MonitorInfo[];
  /** Open detached panel windows. */
  popouts: WindowInfo[];
  /** Panel ids currently in the dock (grid + floating). */
  dockPanels: string[];
  /** Floating panel ids. */
  floatingPanels: string[];
  /** The main window is pinned to a monitor in this layout. */
  mainPinned: boolean;
  mainMonitor: string | null;

  setState: (state: LayoutsState) => void;
  patch: (p: Partial<Omit<LayoutStore, "setState" | "patch">>) => void;
}

export const useLayoutStore = create<LayoutStore>()((set) => ({
  state: null,
  activeId: "coding",
  loaded: false,
  persistError: null,
  locked: false,
  editorOpen: false,
  dialog: null,
  monitors: [],
  popouts: [],
  dockPanels: [],
  floatingPanels: [],
  mainPinned: false,
  mainMonitor: null,

  setState: (state) => set({ state, persistError: null }),
  patch: (p) => set(p),
}));
