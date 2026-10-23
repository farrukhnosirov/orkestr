/**
 * The open context menu of this window (at most one) and the window's menu
 * environment (actions that only some windows offer, e.g. "Search in
 * Orkestr" needs the command palette of the main window).
 */
import { create } from "zustand";

import type { MenuEntry } from "./model";

export interface OpenMenu {
  /** Increments per open: remounts the view (fresh focus / position). */
  nonce: number;
  x: number;
  y: number;
  items: MenuEntry[];
  /** Accessible name of the menu. */
  label?: string;
  /**
   * Keep keyboard focus where it is (a text field: moving focus would commit
   * or close inline editors). The menu is then driven by a window-level key
   * listener.
   */
  keepFocus?: boolean;
  /** Element to refocus when the menu closes. */
  returnFocus?: HTMLElement | null;
}

interface ContextMenuState {
  menu: OpenMenu | null;
  open: (menu: Omit<OpenMenu, "nonce">) => void;
  close: () => void;
}

let nonce = 0;

export const useContextMenuStore = create<ContextMenuState>()((set) => ({
  menu: null,
  open: (menu) => set({ menu: { ...menu, nonce: ++nonce } }),
  close: () => set({ menu: null }),
}));

/** Opens a menu at viewport coordinates (e.g. from a "…" button or Shift+F10). */
export function openContextMenu(menu: Omit<OpenMenu, "nonce">): void {
  useContextMenuStore.getState().open(menu);
}

export function closeContextMenu(): void {
  useContextMenuStore.getState().close();
}

/** Window-specific actions offered by the default menus. */
export interface ContextMenuEnvironment {
  /** "Search in Orkestr" for the selected text (main window: global search). */
  searchInOrkestr?: (text: string) => void;
  /** "Ask Manager about selection". */
  askManager?: (text: string) => void;
}

let environment: ContextMenuEnvironment = {};

/** Sets this window's environment; returns a disposer restoring the previous one. */
export function setContextMenuEnvironment(env: ContextMenuEnvironment): () => void {
  const prev = environment;
  environment = { ...prev, ...env };
  return () => {
    environment = prev;
  };
}

export function getContextMenuEnvironment(): ContextMenuEnvironment {
  return environment;
}
