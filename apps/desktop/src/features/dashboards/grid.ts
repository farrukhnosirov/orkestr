/**
 * 12-column grid logic of the Dashboard Builder. Items carry explicit
 * `x, y, w, h`; every edit (add, move, resize) resolves collisions by pushing
 * the other items down and then compacts the layout upwards, so a layout is
 * always overlap-free (core rejects overlaps on `dashboards.save`).
 *
 * Width presets reuse Mission Control's size scale (`SIZE_COLUMNS`).
 */
import type { WidgetInstance } from "@orkestr/protocol";

import { SIZE_COLUMNS, WIDGET_SIZES, type WidgetSize } from "../../panels/MissionControl/arrangement";

export const GRID_COLUMNS = 12;
export const MAX_ROWS = 24;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function overlaps(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

function clampRect<T extends Rect>(r: T): T {
  const w = Math.max(1, Math.min(GRID_COLUMNS, Math.round(r.w)));
  const h = Math.max(1, Math.min(MAX_ROWS, Math.round(r.h)));
  const x = Math.max(0, Math.min(GRID_COLUMNS - w, Math.round(r.x)));
  const y = Math.max(0, Math.round(r.y));
  return { ...r, x, y, w, h };
}

/** Moves every item upwards as far as it goes (stable top-to-bottom order). */
export function compact(items: readonly WidgetInstance[]): WidgetInstance[] {
  const sorted = [...items].sort((a, b) => a.y - b.y || a.x - b.x);
  const placed: WidgetInstance[] = [];
  for (const item of sorted) {
    let y = item.y;
    while (y > 0 && !placed.some((p) => overlaps({ ...item, y: y - 1 }, p))) y -= 1;
    placed.push({ ...item, y });
  }
  return placed;
}

/**
 * Resolves collisions with `moved`: when the item moved down onto another one,
 * that one first tries to hop above it (swap); otherwise colliding items are
 * pushed below (recursively).
 */
function resolve(items: WidgetInstance[], moved: WidgetInstance, fromY: number): WidgetInstance[] {
  let out = items.map((i) => (i.instanceId === moved.instanceId ? moved : i));
  const queue: WidgetInstance[] = [moved];
  let guard = 0;
  while (queue.length > 0 && guard < 10_000) {
    guard += 1;
    const cur = queue.shift()!;
    for (const other of out) {
      if (other.instanceId === cur.instanceId || !overlaps(cur, other)) continue;
      let next: WidgetInstance = { ...other, y: cur.y + cur.h };
      if (cur === moved && moved.y > fromY && other.y <= moved.y) {
        const up = { ...other, y: Math.max(0, moved.y - other.h) };
        if (!out.some((i) => i.instanceId !== other.instanceId && overlaps(up, i))) next = up;
      }
      out = out.map((i) => (i.instanceId === other.instanceId ? next : i));
      if (next.y > other.y) queue.push(next);
    }
  }
  return out;
}

function sameOrder(items: readonly WidgetInstance[], next: WidgetInstance[]): WidgetInstance[] {
  const byId = new Map(next.map((i) => [i.instanceId, i]));
  return items.map((i) => byId.get(i.instanceId) ?? i);
}

function apply(items: readonly WidgetInstance[], id: string, patch: (i: WidgetInstance) => WidgetInstance): WidgetInstance[] {
  const target = items.find((i) => i.instanceId === id);
  if (!target) return [...items];
  const moved = clampRect(patch(target));
  return sameOrder(items, compact(resolve([...items], moved, target.y)));
}

/** Moves an item to cell `(x, y)`. */
export function moveItem(items: readonly WidgetInstance[], id: string, x: number, y: number): WidgetInstance[] {
  return apply(items, id, (i) => ({ ...i, x, y }));
}

/** Resizes an item to `w × h` cells. */
export function resizeItem(items: readonly WidgetInstance[], id: string, w: number, h: number): WidgetInstance[] {
  return apply(items, id, (i) => ({ ...i, w, h }));
}

/** First free spot (top-left first) for a `w × h` item. */
export function findSpot(items: readonly Rect[], w: number, h: number): { x: number; y: number } {
  const width = Math.min(GRID_COLUMNS, Math.max(1, w));
  for (let y = 0; y < 1000; y += 1) {
    for (let x = 0; x + width <= GRID_COLUMNS; x += 1) {
      if (!items.some((i) => overlaps({ x, y, w: width, h }, i))) return { x, y };
    }
  }
  return { x: 0, y: items.reduce((m, i) => Math.max(m, i.y + i.h), 0) };
}

/** Adds a new item at the first free spot. */
export function addItem(items: readonly WidgetInstance[], item: Omit<WidgetInstance, "x" | "y">): WidgetInstance[] {
  const r = clampRect({ ...item, x: 0, y: 0 });
  const spot = findSpot(items, r.w, r.h);
  return [...items, { ...r, ...spot }];
}

export function removeItem(items: readonly WidgetInstance[], id: string): WidgetInstance[] {
  return compact(items.filter((i) => i.instanceId !== id)).sort((a, b) => a.y - b.y || a.x - b.x);
}

/** Next Mission Control width preset after `w` (wraps around). */
export function nextWidthPreset(w: number): number {
  const widths = WIDGET_SIZES.map((s: WidgetSize) => SIZE_COLUMNS[s]);
  return widths.find((c) => c > w) ?? widths[0] ?? GRID_COLUMNS;
}

/** Total rows used. */
export function rowCount(items: readonly Rect[]): number {
  return items.reduce((m, i) => Math.max(m, i.y + i.h), 0);
}

/** Random-enough instance id (`[a-z0-9]`, unique within the dashboard). */
export function newInstanceId(items: readonly { instanceId: string }[]): string {
  for (;;) {
    const id = `w${Math.random().toString(36).slice(2, 10)}`;
    if (!items.some((i) => i.instanceId === id)) return id;
  }
}
