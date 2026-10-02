/**
 * Command palette helpers: open with a preset query (e.g. `/` for global
 * search) and remember recently used commands (per-device convenience,
 * localStorage — not business data).
 */
import { create } from "zustand";

import { useAppStore } from "../../store/app";

const RECENT_KEY = "orkestr.palette.recent";
export const MAX_RECENT = 6;
/** Prefix switching the palette into global search mode. */
export const SEARCH_PREFIX = "/";

function readRecent(): string[] {
  try {
    const raw = window.localStorage.getItem(RECENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string").slice(0, MAX_RECENT) : [];
  } catch {
    return [];
  }
}

function writeRecent(ids: string[]): void {
  try {
    window.localStorage.setItem(RECENT_KEY, JSON.stringify(ids));
  } catch {
    /* storage unavailable */
  }
}

/** Moves `id` to the front, de-duplicated and capped. */
export function pushRecent(list: readonly string[], id: string, max = MAX_RECENT): string[] {
  return [id, ...list.filter((x) => x !== id)].slice(0, max);
}

export interface PaletteState {
  initialQuery: string;
  recent: string[];
  openPalette: (query?: string) => void;
  noteRecent: (id: string) => void;
}

export const usePaletteStore = create<PaletteState>()((set, get) => ({
  initialQuery: "",
  recent: readRecent(),
  openPalette: (query = "") => {
    set({ initialQuery: query });
    useAppStore.getState().setPaletteOpen(true);
  },
  noteRecent: (id) => {
    const recent = pushRecent(get().recent, id);
    writeRecent(recent);
    set({ recent });
  },
}));
