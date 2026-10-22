/**
 * Window Library catalog (Wave 8): every registered panel plus the dialog
 * windows (Settings, Theme Editor, Layout Editor), grouped by category and
 * searchable in English and Russian at once. Pure apart from reading the
 * registries and the bundled translations; unit-tested.
 */
import { Brush, Settings, SlidersHorizontal, type LucideIcon } from "lucide-react";

import { i18n, type UiLanguage } from "../../i18n";
import { PANELS, splitTags } from "../../panels/registry";
import { PANEL_CATEGORIES, type PanelCategory } from "../../panels/types";
import { commands } from "../commands/registry";
import { fuzzyMatch } from "../search/fuzzy";

export type EntryKind = "panel" | "window";

/** One card of the library. Texts are localized getters (current language). */
export interface LibraryEntry {
  id: string;
  kind: EntryKind;
  category: PanelCategory;
  icon: LucideIcon;
  /** Roadmap phase when it becomes functional (0 = available). */
  phase: number;
  /** Registry command whose shortcut opens this entry, if any. */
  commandId?: string;
  readonly title: string;
  readonly summary: string;
  readonly details: string;
  readonly tags: readonly string[];
}

/** Dialog windows that are not dockable panels but belong in the library. */
export const WINDOW_IDS = ["settings", "themeEditor", "layoutEditor"] as const;
export type WindowId = (typeof WINDOW_IDS)[number];

const WINDOW_ICONS: Record<WindowId, LucideIcon> = {
  settings: Settings,
  themeEditor: Brush,
  layoutEditor: SlidersHorizontal,
};

/** Commands whose shortcut opens a library entry (shown on the card). */
const ENTRY_COMMANDS: Readonly<Record<string, string>> = {
  manager: "manager.ask",
  terminal: "terminal.open",
  missionControl: "view.missionControl",
  extensions: "extensions.open",
  settings: "settings.open",
};

/** Translation in a fixed language (keys are built at runtime, hence the loose signature). */
export function tr(lang: UiLanguage, key: string): string {
  // Dynamic keys: the guard test checks every one of them exists in both languages.
  const fixed = i18n.getFixedT(lang) as unknown as (k: string) => string;
  return fixed(key);
}

/** Translation keys of an entry's texts. */
export function entryKeys(id: string, kind: EntryKind): { title: string; summary: string; details: string; tags: string } {
  const base = kind === "panel" ? `windowLibrary:panels.${id}` : `windowLibrary:windows.${id}`;
  return {
    title: kind === "panel" ? `shell:panels.${id}.title` : `${base}.title`,
    summary: `${base}.summary`,
    details: `${base}.details`,
    tags: `${base}.tags`,
  };
}

function windowEntry(id: WindowId): LibraryEntry {
  const keys = entryKeys(id, "window");
  const cur = () => i18n.language as UiLanguage;
  const entry: LibraryEntry = {
    id,
    kind: "window",
    category: "system",
    icon: WINDOW_ICONS[id],
    phase: 0,
    get title() {
      return tr(cur(), keys.title);
    },
    get summary() {
      return tr(cur(), keys.summary);
    },
    get details() {
      return tr(cur(), keys.details);
    },
    get tags() {
      return splitTags(tr(cur(), keys.tags));
    },
  };
  const cmd = ENTRY_COMMANDS[id];
  if (cmd) entry.commandId = cmd;
  return entry;
}

/** All library entries in registry order (panels first, then dialog windows). */
export function libraryEntries(): LibraryEntry[] {
  const panels: LibraryEntry[] = PANELS.map((p) => {
    const entry: LibraryEntry = {
      id: p.id,
      kind: "panel",
      category: p.category,
      icon: p.icon,
      phase: p.phase,
      get title() {
        return p.title;
      },
      get summary() {
        return p.summary;
      },
      get details() {
        return p.details;
      },
      get tags() {
        return p.tags;
      },
    };
    const cmd = ENTRY_COMMANDS[p.id];
    if (cmd) entry.commandId = cmd;
    return entry;
  });
  return [...panels, ...WINDOW_IDS.map(windowEntry)];
}

/** Shortcut of the entry's command (`Ctrl+Shift+M`), if any. */
export function entryShortcut(entry: LibraryEntry): string | undefined {
  if (!entry.commandId) return undefined;
  return commands.find((c) => c.id === entry.commandId)?.shortcut;
}

/** Categories in display order that have at least one entry, with counts. */
export function categoryCounts(entries: readonly LibraryEntry[]): Array<{ id: PanelCategory; count: number }> {
  return PANEL_CATEGORIES.map((id) => ({ id, count: entries.filter((e) => e.category === id).length })).filter((c) => c.count > 0);
}

// ── search ───────────────────────────────────────────────────────────────────

const EN_KEYS = "`qwertyuiop[]asdfghjkl;'zxcvbnm,.";
const RU_KEYS = "ёйцукенгшщзхъфывапролджэячсмитьбю";

/**
 * The same keystrokes on the other keyboard layout (`rjl` ↔ `код`), so a
 * query typed with the wrong layout still finds the window.
 */
export function swapLayout(text: string): string {
  let out = "";
  for (const ch of text.toLowerCase()) {
    const en = EN_KEYS.indexOf(ch);
    const ru = RU_KEYS.indexOf(ch);
    out += en >= 0 ? (RU_KEYS[en] ?? ch) : ru >= 0 ? (EN_KEYS[ru] ?? ch) : ch;
  }
  return out;
}

interface Haystack {
  /** Titles in every language (current language first). */
  titles: string[];
  /** Tags, category names and the id. */
  tags: string[];
  /** Summaries and long descriptions. */
  texts: string[];
}

const LANGS: readonly UiLanguage[] = ["en", "ru"];

function haystack(entry: LibraryEntry, current: UiLanguage): Haystack {
  const keys = entryKeys(entry.id, entry.kind);
  const langs = [current, ...LANGS.filter((l) => l !== current)];
  return {
    titles: langs.map((l) => tr(l, keys.title)),
    tags: [
      entry.id,
      ...langs.flatMap((l) => splitTags(tr(l, keys.tags))),
      ...langs.map((l) => tr(l, `windowLibrary:categories.${entry.category}`)),
    ],
    texts: langs.flatMap((l) => [tr(l, keys.summary), tr(l, keys.details)]),
  };
}

/** Best score of one query word against an entry (`null` = no match). Indices are for the current-language title. */
function scoreWord(word: string, h: Haystack): { score: number; indices: number[] } | null {
  let best: { score: number; indices: number[] } | null = null;
  const consider = (score: number, indices: number[]) => {
    if (!best || score > best.score) best = { score, indices };
  };
  h.titles.forEach((title, i) => {
    const m = fuzzyMatch(word, title);
    if (m) consider(m.score + 300, i === 0 ? m.indices : []);
  });
  for (const tag of h.tags) {
    const m = fuzzyMatch(word, tag);
    // Tags only count as substring/word-start hits; scattered letters are noise.
    if (m && m.score >= 1000) consider(m.score - 100, []);
  }
  for (const text of h.texts) {
    const at = text.toLowerCase().indexOf(word);
    if (at >= 0) consider(500 - Math.min(at, 200), []);
  }
  return best;
}

export interface SearchHit {
  entry: LibraryEntry;
  score: number;
  /** Matched character indices in the current-language title (for highlighting). */
  indices: number[];
}

function scoreQuery(words: readonly string[], h: Haystack): { score: number; indices: number[] } | null {
  let score = 0;
  const indices = new Set<number>();
  for (const w of words) {
    const m = scoreWord(w, h);
    if (!m) return null;
    score += m.score;
    for (const i of m.indices) indices.add(i);
  }
  return { score, indices: [...indices].sort((a, b) => a - b) };
}

/**
 * Ranks entries for `query` (every word must match a title, tag or
 * description in English or Russian; the other keyboard layout is tried
 * too). An empty query keeps the given order.
 */
export function searchEntries(entries: readonly LibraryEntry[], query: string, current: UiLanguage): SearchHit[] {
  const words = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return entries.map((entry) => ({ entry, score: 0, indices: [] }));
  const swapped = words.map(swapLayout);
  const hits: SearchHit[] = [];
  for (const entry of entries) {
    const h = haystack(entry, current);
    const direct = scoreQuery(words, h);
    const viaLayout = direct ? null : scoreQuery(swapped, h);
    const m = direct ?? (viaLayout ? { score: viaLayout.score - 150, indices: viaLayout.indices } : null);
    if (!m) continue;
    hits.push({ entry, score: entry.phase > 0 ? m.score - 200 : m.score, indices: m.indices });
  }
  return hits.sort((a, b) => b.score - a.score);
}

/** Entries of a filter: a category, favorites, recently opened (in that order), or all. */
export function filterEntries(
  entries: readonly LibraryEntry[],
  filter: string,
  lists: { favorites: readonly string[]; recent: readonly string[] },
): LibraryEntry[] {
  const byId = new Map(entries.map((e) => [e.id, e]));
  const pick = (ids: readonly string[]) => ids.map((id) => byId.get(id)).filter((e): e is LibraryEntry => !!e);
  if (filter === "all") return [...entries];
  if (filter === "favorites") return pick(lists.favorites);
  if (filter === "recent") return pick(lists.recent);
  return entries.filter((e) => e.category === filter);
}

// ── keyboard navigation ──────────────────────────────────────────────────────

/**
 * Next selected index in a grid of `count` cards laid out in `columns`
 * columns for a navigation key; `null` when the key is not a navigation key.
 */
export function moveSelection(index: number, key: string, columns: number, count: number): number | null {
  if (count <= 0) return null;
  const cols = Math.max(1, columns);
  const clamp = (i: number) => Math.min(count - 1, Math.max(0, i));
  switch (key) {
    case "ArrowRight":
      return clamp(index + 1);
    case "ArrowLeft":
      return clamp(index - 1);
    case "ArrowDown":
      return index + cols < count ? index + cols : index;
    case "ArrowUp":
      return index - cols >= 0 ? index - cols : index;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/** Next filter for PageUp / PageDown (cycles through `filters`). */
export function cycleFilter(filters: readonly string[], current: string, step: 1 | -1): string {
  if (filters.length === 0) return current;
  const i = filters.indexOf(current);
  const next = (i < 0 ? 0 : i + step + filters.length) % filters.length;
  return filters[next] ?? current;
}
