/**
 * Orkestr context menu — data model and pure helpers (no DOM, no React).
 *
 * A menu is a list of entries: actions, checkbox / radio items, submenus,
 * separators and group labels. Several *sections* (from the element under
 * the pointer and its ancestors, plus the default items for text fields,
 * selections, links and images) are merged into one list with separators
 * between them; `normalizeEntries` drops hidden items and redundant
 * separators.
 */
import type { ReactNode } from "react";

interface EntryBase {
  id: string;
  /** Localized label (pass `t(...)` output). */
  label: string;
  /** Icon element, e.g. `<Copy size={14} />`. */
  icon?: ReactNode;
  disabled?: boolean;
  /** Not rendered at all (convenient for conditional items). */
  hidden?: boolean;
}

/** A plain command. */
export interface ActionEntry extends EntryBase {
  kind?: "action";
  /** Shortcut hint shown on the right ("Ctrl+C"); display only. */
  shortcut?: string;
  /** Destructive action (danger colors). */
  danger?: boolean;
  onSelect: () => void | Promise<void>;
}

/** An item with a check mark that toggles. */
export interface CheckboxEntry extends EntryBase {
  kind: "checkbox";
  checked: boolean;
  shortcut?: string;
  onSelect: (checked: boolean) => void | Promise<void>;
}

/** One choice of a group of mutually exclusive options. */
export interface RadioEntry extends EntryBase {
  kind: "radio";
  checked: boolean;
  onSelect: () => void | Promise<void>;
}

/** Opens a nested menu. */
export interface SubmenuEntry extends EntryBase {
  kind: "submenu";
  items: MenuEntry[];
}

export interface SeparatorEntry {
  kind: "separator";
  id?: string;
  hidden?: boolean;
}

/** A non-interactive group heading. */
export interface LabelEntry {
  kind: "label";
  id: string;
  label: string;
  hidden?: boolean;
}

export type MenuEntry = ActionEntry | CheckboxEntry | RadioEntry | SubmenuEntry | SeparatorEntry | LabelEntry;
/** Entries that can be focused and activated. */
export type InteractiveEntry = ActionEntry | CheckboxEntry | RadioEntry | SubmenuEntry;

export function isSeparator(e: MenuEntry): e is SeparatorEntry {
  return e.kind === "separator";
}

export function isInteractive(e: MenuEntry): e is InteractiveEntry {
  return e.kind !== "separator" && e.kind !== "label";
}

/** Whether keyboard focus may land on the entry. */
export function isFocusable(e: MenuEntry): boolean {
  return isInteractive(e) && !e.disabled;
}

/**
 * Drops hidden entries, empty submenus, and leading / trailing / doubled
 * separators (recursively for submenus). A label directly followed by a
 * separator or the end of the list is dropped too.
 */
export function normalizeEntries(entries: readonly MenuEntry[]): MenuEntry[] {
  const visible: MenuEntry[] = [];
  for (const e of entries) {
    if (e.hidden) continue;
    if (e.kind === "submenu") {
      const items = normalizeEntries(e.items);
      if (items.length === 0) continue;
      visible.push({ ...e, items });
      continue;
    }
    visible.push(e);
  }
  const out: MenuEntry[] = [];
  for (const e of visible) {
    if (isSeparator(e)) {
      const last = out[out.length - 1];
      if (!last || isSeparator(last)) continue;
      if (last.kind === "label") out.pop();
      if (out.length === 0 || isSeparator(out[out.length - 1] as MenuEntry)) continue;
    }
    out.push(e);
  }
  while (out.length > 0) {
    const last = out[out.length - 1] as MenuEntry;
    if (isSeparator(last) || last.kind === "label") out.pop();
    else break;
  }
  return out;
}

/** Joins sections with separators and normalizes the result. */
export function mergeSections(sections: ReadonlyArray<readonly MenuEntry[]>): MenuEntry[] {
  const all: MenuEntry[] = [];
  sections.forEach((section, i) => {
    if (section.length === 0) return;
    if (all.length > 0) all.push({ kind: "separator", id: `section-${i}` });
    all.push(...section);
  });
  return normalizeEntries(all);
}

/**
 * Next focusable index from `from` in direction `dir` (wrapping). `from = -1`
 * with `dir = 1` gives the first focusable entry; `from = entries.length`
 * with `dir = -1` the last. `-1` when nothing is focusable.
 */
export function nextFocusable(entries: readonly MenuEntry[], from: number, dir: 1 | -1): number {
  const n = entries.length;
  if (n === 0) return -1;
  for (let step = 1; step <= n; step++) {
    const i = (((from + dir * step) % n) + n) % n;
    const e = entries[i];
    if (e && isFocusable(e)) return i;
  }
  return -1;
}

/** First / last focusable index (`-1` when none). */
export function edgeFocusable(entries: readonly MenuEntry[], edge: "first" | "last"): number {
  return edge === "first" ? nextFocusable(entries, -1, 1) : nextFocusable(entries, entries.length, -1);
}

/**
 * Type-ahead: the first focusable entry after `from` (wrapping) whose label
 * starts with `query` (case-insensitive, any script). A query of one
 * repeated letter ("ппп") cycles through the entries starting with it.
 */
export function typeaheadMatch(entries: readonly MenuEntry[], from: number, query: string): number {
  const q = query.toLocaleLowerCase();
  if (!q) return -1;
  const single = [...q].every((ch) => ch === q[0]);
  const n = entries.length;
  // A multi-letter query may still match the current entry ("Co" → "Copy" →
  // "Cop" stays on "Copy"); a single letter moves on to the next match.
  const start = single ? 1 : 0;
  const needle = single ? (q[0] as string) : q;
  for (let step = start; step < n + start; step++) {
    const i = (((from + step) % n) + n) % n;
    const e = entries[i];
    if (!e || !isFocusable(e) || !("label" in e)) continue;
    if (e.label.toLocaleLowerCase().startsWith(needle)) return i;
  }
  return -1;
}

/** Whether a key is a printable type-ahead character. */
export function isTypeaheadKey(key: string, ctrl: boolean, alt: boolean, meta: boolean): boolean {
  return !ctrl && !alt && !meta && [...key].length === 1 && key !== " ";
}

export interface Size {
  width: number;
  height: number;
}

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Gap kept between a menu and the viewport edges (px). */
export const VIEWPORT_MARGIN = 4;

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(v, max));
}

/**
 * Top-left of a root menu opened at the pointer: below-right of the point,
 * flipped to the left / above when it would overflow, and clamped into the
 * viewport.
 */
export function placeMenu(point: { x: number; y: number }, size: Size, viewport: Size, margin = VIEWPORT_MARGIN): { left: number; top: number } {
  let left = point.x;
  let top = point.y;
  if (left + size.width > viewport.width - margin && point.x - size.width >= margin) left = point.x - size.width;
  if (top + size.height > viewport.height - margin && point.y - size.height >= margin) top = point.y - size.height;
  return {
    left: clamp(left, margin, Math.max(margin, viewport.width - size.width - margin)),
    top: clamp(top, margin, Math.max(margin, viewport.height - size.height - margin)),
  };
}

/**
 * Top-left of a submenu next to its parent item: to the right of the item,
 * or to the left when there is no room, aligned with the item's top and
 * clamped vertically.
 */
export function placeSubmenu(anchor: Rect, size: Size, viewport: Size, margin = VIEWPORT_MARGIN, overlap = 2): { left: number; top: number; side: "right" | "left" } {
  const right = anchor.right - overlap;
  const fitsRight = right + size.width <= viewport.width - margin;
  const leftSide = anchor.left + overlap - size.width;
  const side = fitsRight || leftSide < margin ? "right" : "left";
  const left = side === "right" ? clamp(right, margin, Math.max(margin, viewport.width - size.width - margin)) : leftSide;
  const top = clamp(anchor.top - margin, margin, Math.max(margin, viewport.height - size.height - margin));
  return { left, top, side };
}
