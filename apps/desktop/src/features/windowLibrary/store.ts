/**
 * Window Library state: open flag, active category, and the per-device
 * favorites / recently opened / sidebar pins (localStorage — UI convenience,
 * not business data, like the palette's recent commands).
 */
import { create } from "zustand";

const FAVORITES_KEY = "orkestr.windowLibrary.favorites";
const RECENT_KEY = "orkestr.windowLibrary.recent";
const PINS_KEY = "orkestr.windowLibrary.pins";
export const MAX_RECENT = 8;
export const MAX_PINS = 8;

/** Category filter: a panel category, or one of the virtual lists. */
export type LibraryFilter = "all" | "favorites" | "recent" | string;

function readList(key: string, max: number): string[] {
  try {
    const raw = window.localStorage.getItem(key);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string").slice(0, max) : [];
  } catch {
    return [];
  }
}

function writeList(key: string, ids: readonly string[]): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(ids));
  } catch {
    /* storage unavailable: kept for this session only */
  }
}

/** Moves `id` to the front, de-duplicated and capped. */
export function pushFront(list: readonly string[], id: string, max: number): string[] {
  return [id, ...list.filter((x) => x !== id)].slice(0, max);
}

/** Adds `id` (at the end) or removes it. */
export function toggleIn(list: readonly string[], id: string, max = Number.POSITIVE_INFINITY): string[] {
  return list.includes(id) ? list.filter((x) => x !== id) : [...list, id].slice(-max);
}

export interface WindowLibraryState {
  open: boolean;
  filter: LibraryFilter;
  favorites: string[];
  recent: string[];
  pins: string[];
  setOpen: (open: boolean) => void;
  toggle: () => void;
  setFilter: (filter: LibraryFilter) => void;
  toggleFavorite: (id: string) => void;
  togglePin: (id: string) => void;
  noteOpened: (id: string) => void;
}

export const useWindowLibraryStore = create<WindowLibraryState>()((set, get) => ({
  open: false,
  filter: "all",
  favorites: readList(FAVORITES_KEY, 64),
  recent: readList(RECENT_KEY, MAX_RECENT),
  pins: readList(PINS_KEY, MAX_PINS),
  setOpen: (open) => set({ open }),
  toggle: () => set({ open: !get().open }),
  setFilter: (filter) => set({ filter }),
  toggleFavorite: (id) => {
    const favorites = toggleIn(get().favorites, id, 64);
    writeList(FAVORITES_KEY, favorites);
    set({ favorites });
  },
  togglePin: (id) => {
    const pins = toggleIn(get().pins, id, MAX_PINS);
    writeList(PINS_KEY, pins);
    set({ pins });
  },
  noteOpened: (id) => {
    const recent = pushFront(get().recent, id, MAX_RECENT);
    writeList(RECENT_KEY, recent);
    set({ recent });
  },
}));

/** Opens the library (title bar, rail, palette, Ctrl+Shift+L). */
export function openWindowLibrary(): void {
  useWindowLibraryStore.getState().setOpen(true);
}
