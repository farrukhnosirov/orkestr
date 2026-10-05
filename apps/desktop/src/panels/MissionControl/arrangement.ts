/**
 * Mission Control widget arrangement (§32, §48 Dashboard Builder foundation).
 *
 * An arrangement is the ordered list of widget placements (id, size,
 * hidden). It is plain JSON, validated against the widget registry on load
 * (unknown ids dropped, duplicates removed, new widgets appended), so a
 * stale or hand-edited arrangement can never break the screen.
 *
 * Persistence goes through {@link ArrangementStorage}. Until the layout area
 * (Phase 3, layout-multimonitor) lands, the default implementation stores
 * the arrangement in `localStorage` — per device by construction. The layout
 * area binds its own storage (UI config / per-monitor layouts) with
 * {@link setArrangementStorage} without touching this panel.
 */

/** Widget footprint in the 12-column grid. */
export type WidgetSize = "s" | "m" | "l" | "xl" | "full";

export const WIDGET_SIZES: readonly WidgetSize[] = ["s", "m", "l", "xl", "full"];

/** Grid columns spanned per size (12-column grid). */
export const SIZE_COLUMNS: Readonly<Record<WidgetSize, number>> = { s: 3, m: 4, l: 6, xl: 8, full: 12 };

export interface WidgetPlacement {
  id: string;
  size: WidgetSize;
  hidden: boolean;
}

export interface Arrangement {
  version: 1;
  widgets: WidgetPlacement[];
}

/** What the arrangement logic needs to know about a widget. */
export interface PlacementDefaults {
  id: string;
  size: WidgetSize;
  /** Sizes the widget supports (first = smallest). */
  sizes: readonly WidgetSize[];
  /** Hidden until the user enables it. */
  hiddenByDefault?: boolean;
}

export const ARRANGEMENT_VERSION = 1;

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isSize(v: unknown): v is WidgetSize {
  return typeof v === "string" && (WIDGET_SIZES as readonly string[]).includes(v);
}

/** The arrangement every new device starts with. */
export function defaultArrangement(defaults: readonly PlacementDefaults[]): Arrangement {
  return {
    version: ARRANGEMENT_VERSION,
    widgets: defaults.map((d) => ({ id: d.id, size: d.size, hidden: d.hiddenByDefault ?? false })),
  };
}

/**
 * Validates any value against the registry: keeps known widgets in the saved
 * order with a supported size, appends widgets the saved arrangement does not
 * know yet, and falls back to the default for anything unusable.
 */
export function normalizeArrangement(raw: unknown, defaults: readonly PlacementDefaults[]): Arrangement {
  const byId = new Map(defaults.map((d) => [d.id, d]));
  const list = isRecord(raw) && raw["version"] === ARRANGEMENT_VERSION && Array.isArray(raw["widgets"]) ? raw["widgets"] : null;
  if (!list) return defaultArrangement(defaults);
  const seen = new Set<string>();
  const widgets: WidgetPlacement[] = [];
  for (const item of list) {
    if (!isRecord(item) || typeof item["id"] !== "string") continue;
    const def = byId.get(item["id"]);
    if (!def || seen.has(def.id)) continue;
    seen.add(def.id);
    const size = isSize(item["size"]) && def.sizes.includes(item["size"]) ? item["size"] : def.size;
    widgets.push({ id: def.id, size, hidden: item["hidden"] === true });
  }
  for (const def of defaults) {
    if (!seen.has(def.id)) widgets.push({ id: def.id, size: def.size, hidden: def.hiddenByDefault ?? false });
  }
  return { version: ARRANGEMENT_VERSION, widgets };
}

/** Parses a serialized arrangement (any garbage → default). */
export function parseArrangement(text: string | null, defaults: readonly PlacementDefaults[]): Arrangement {
  if (!text) return defaultArrangement(defaults);
  try {
    return normalizeArrangement(JSON.parse(text) as unknown, defaults);
  } catch {
    return defaultArrangement(defaults);
  }
}

export function serializeArrangement(a: Arrangement): string {
  return JSON.stringify(a);
}

/** Moves widget `id` to the position of `targetId` (drag and drop). */
export function moveWidget(a: Arrangement, id: string, targetId: string): Arrangement {
  if (id === targetId) return a;
  const from = a.widgets.findIndex((w) => w.id === id);
  const to = a.widgets.findIndex((w) => w.id === targetId);
  if (from < 0 || to < 0) return a;
  const widgets = [...a.widgets];
  const [moved] = widgets.splice(from, 1);
  if (!moved) return a;
  widgets.splice(to, 0, moved);
  return { ...a, widgets };
}

/** Moves a widget by `delta` positions among the visible widgets (keyboard). */
export function moveWidgetBy(a: Arrangement, id: string, delta: number): Arrangement {
  const visible = a.widgets.filter((w) => !w.hidden);
  const idx = visible.findIndex((w) => w.id === id);
  if (idx < 0) return a;
  const target = visible[Math.max(0, Math.min(visible.length - 1, idx + delta))];
  return target ? moveWidget(a, id, target.id) : a;
}

/** Next supported size (wraps around). */
export function cycleSize(a: Arrangement, id: string, sizes: readonly WidgetSize[]): Arrangement {
  if (sizes.length === 0) return a;
  return {
    ...a,
    widgets: a.widgets.map((w) => {
      if (w.id !== id) return w;
      const i = sizes.indexOf(w.size);
      const next = sizes[(i + 1) % sizes.length] ?? w.size;
      return { ...w, size: next };
    }),
  };
}

export function setHidden(a: Arrangement, id: string, hidden: boolean): Arrangement {
  return { ...a, widgets: a.widgets.map((w) => (w.id === id ? { ...w, hidden } : w)) };
}

// ── storage binding ────────────────────────────────────────────────────

/**
 * Where arrangements live. INTEGRATOR(P3 layout): bind the layout area's
 * per-device / per-monitor store here with `setArrangementStorage`.
 */
export interface ArrangementStorage {
  load(key: string): string | null;
  save(key: string, value: string): void;
}

export const ARRANGEMENT_KEY = "orkestr.missionControl.arrangement.v1";

const localStorageBackend: ArrangementStorage = {
  load(key) {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  save(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      /* storage unavailable: keep the in-memory arrangement */
    }
  },
};

let storage: ArrangementStorage = localStorageBackend;

export function setArrangementStorage(next: ArrangementStorage | null): void {
  storage = next ?? localStorageBackend;
}

export function arrangementStorage(): ArrangementStorage {
  return storage;
}
