/**
 * Layout documents (§29, ADR-0013/0015): validation, sanitising against the
 * panel registry, migration of older shapes, import/export.
 *
 * The core is the authority (`orkestr_protocol::methods::layouts`); the
 * rules here mirror it so the UI can explain problems before a round trip
 * and can repair a stored layout that references panels this build lacks.
 * Pure functions only — no dockview, no React.
 */
import type { JsonValue, LayoutDocument, LayoutRect, PopoutWindowSpec, WindowArrangement } from "@orkestr/protocol";

/** Mirrors `LAYOUT_SCHEMA_VERSION` (Rust). */
export const LAYOUT_SCHEMA_VERSION = 1;
/** Mirrors `MAX_DOCK_BYTES`. */
export const MAX_DOCK_BYTES = 256 * 1024;
export const MAX_POPOUTS = 16;
export const MAX_PANELS = 64;
/** Pre-Phase-3 layout cache (dockview JSON in localStorage); migrated once into the core. */
export const LEGACY_LAYOUT_KEY = "orkestr.layout.v3";
/** Marker of an exported layout file. */
export const EXPORT_FORMAT = "orkestr.layout";

export const BUILTIN_LAYOUT_IDS = ["coding", "mission-control", "monitoring", "focus", "review", "custom"] as const;
export type BuiltinLayoutId = (typeof BUILTIN_LAYOUT_IDS)[number];

export function isBuiltinLayoutId(id: string): id is BuiltinLayoutId {
  return (BUILTIN_LAYOUT_IDS as readonly string[]).includes(id);
}

export const isValidLayoutId = (id: string): boolean => /^[a-z][a-z0-9-]{0,47}$/.test(id);
export const isValidPanelId = (id: string): boolean => /^[a-zA-Z][a-zA-Z0-9_-]{0,47}$/.test(id);
export function isValidLayoutName(name: string): boolean {
  const n = [...name.trim()].length;
  // eslint-disable-next-line no-control-regex
  return n >= 1 && n <= 80 && !/[\u0000-\u001f\u007f]/.test(name);
}

// ── dockview serialized shape (only what we read/repair) ─────────────────────

export interface DockLeaf {
  type: "leaf";
  data: { views: string[]; activeView?: string; id: string; [k: string]: unknown };
  size?: number;
  visible?: boolean;
}
export interface DockBranch {
  type: "branch";
  data: DockNode[];
  size?: number;
  visible?: boolean;
}
export type DockNode = DockLeaf | DockBranch;
export interface DockPanelState {
  id: string;
  contentComponent?: string;
  [k: string]: unknown;
}
export interface SerializedDock {
  grid: { root: DockNode; width: number; height: number; orientation: string; [k: string]: unknown };
  panels: Record<string, DockPanelState>;
  activeGroup?: string;
  floatingGroups?: { data: DockLeaf["data"]; position: unknown; [k: string]: unknown }[];
  [k: string]: unknown;
}

/** dockview JSON is plain data; the generated `JsonValue` type cannot express its index signatures. */
export const asJson = (v: object): JsonValue => v as unknown as JsonValue;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** Result of [`sanitizeDock`]. */
export interface SanitizedDock {
  dock: SerializedDock;
  /** Panel ids still in the layout (grid + floating), in tab order. */
  panels: string[];
  /** Panel ids that were removed (unknown to this build or malformed). */
  dropped: string[];
}

function leafViews(data: unknown): string[] {
  if (!isObj(data) || !Array.isArray(data["views"])) return [];
  return data["views"].filter((v): v is string => typeof v === "string");
}

/**
 * Repairs a serialized dockview layout so it only references panels this
 * build knows: unknown panels are dropped from `panels`, groups and floating
 * groups; empty groups and branches are pruned; popout/edge groups (not used
 * by Orkestr — detached panels are Tauri windows) are removed. Returns
 * `null` when nothing usable remains.
 */
export function sanitizeDock(raw: unknown, isKnownPanel: (component: string) => boolean): SanitizedDock | null {
  if (!isObj(raw) || !isObj(raw["grid"]) || !isObj(raw["panels"])) return null;
  const grid = raw["grid"];
  const dropped = new Set<string>();
  const keptPanels: Record<string, DockPanelState> = {};
  for (const [id, state] of Object.entries(raw["panels"])) {
    const component = isObj(state) ? state["contentComponent"] : undefined;
    if (isObj(state) && typeof component === "string" && isKnownPanel(component) && isValidPanelId(id)) {
      keptPanels[id] = { ...state, id } as DockPanelState;
    } else {
      dropped.add(id);
    }
  }
  const order: string[] = [];
  const fixLeafData = (data: unknown): DockLeaf["data"] | null => {
    if (!isObj(data) || typeof data["id"] !== "string") return null;
    const views = leafViews(data).filter((v) => {
      if (keptPanels[v] && !order.includes(v)) return true;
      if (!keptPanels[v]) dropped.add(v);
      return false;
    });
    if (views.length === 0) return null;
    order.push(...views);
    const active = typeof data["activeView"] === "string" && views.includes(data["activeView"]) ? data["activeView"] : views[0];
    return { ...data, id: data["id"], views, activeView: active };
  };
  const fixNode = (node: unknown): DockNode | null => {
    if (!isObj(node)) return null;
    if (node["type"] === "leaf") {
      const data = fixLeafData(node["data"]);
      return data ? ({ ...node, type: "leaf", data } as DockLeaf) : null;
    }
    if (node["type"] === "branch" && Array.isArray(node["data"])) {
      const children = node["data"].map(fixNode).filter((n): n is DockNode => n !== null);
      return children.length > 0 ? ({ ...node, type: "branch", data: children } as DockBranch) : null;
    }
    return null;
  };
  let root = fixNode(grid["root"]);
  const floating = Array.isArray(raw["floatingGroups"])
    ? raw["floatingGroups"]
        .map((g) => {
          if (!isObj(g)) return null;
          const data = fixLeafData(g["data"]);
          return data ? { ...g, data, position: g["position"] } : null;
        })
        .filter((g): g is NonNullable<typeof g> => g !== null)
    : [];
  if (!root && floating.length === 0) return null;
  // dockview needs a grid root even when every panel floats.
  root ??= { type: "branch", data: [], size: typeof grid["height"] === "number" ? grid["height"] : 0 };
  // Panels referenced by no group are unreachable: drop them too.
  for (const id of Object.keys(keptPanels)) {
    if (!order.includes(id)) {
      delete keptPanels[id];
      dropped.add(id);
    }
  }
  const dock: SerializedDock = {
    grid: {
      ...grid,
      root,
      width: typeof grid["width"] === "number" ? grid["width"] : 0,
      height: typeof grid["height"] === "number" ? grid["height"] : 0,
      orientation: typeof grid["orientation"] === "string" ? grid["orientation"] : "HORIZONTAL",
    },
    panels: keptPanels,
  };
  const groupIds = new Set<string>();
  const collect = (n: DockNode) => (n.type === "leaf" ? groupIds.add(n.data.id) : n.data.forEach(collect));
  collect(root);
  floating.forEach((g) => groupIds.add(g.data.id));
  if (typeof raw["activeGroup"] === "string" && groupIds.has(raw["activeGroup"])) dock.activeGroup = raw["activeGroup"];
  if (floating.length > 0) dock.floatingGroups = floating;
  return { dock, panels: order, dropped: [...dropped].sort() };
}

// ── documents ────────────────────────────────────────────────────────────────

export interface LayoutIssue {
  path: string;
  /** i18n key suffix under `layout:issues.*`. */
  code: "schemaVersion" | "id" | "name" | "basedOn" | "dock" | "dockSize" | "panels" | "panelId" | "popouts" | "duplicate" | "bounds" | "empty";
}

function rectOk(r: LayoutRect | undefined): boolean {
  if (!r) return true;
  const lim = 100_000;
  return r.width > 0 && r.height > 0 && r.width <= lim && r.height <= lim && Math.abs(r.x) <= lim && Math.abs(r.y) <= lim;
}

/** TS mirror of `LayoutDocument::validate` (Rust). */
export function validateLayoutDocument(doc: LayoutDocument): LayoutIssue[] {
  const issues: LayoutIssue[] = [];
  if (doc.schemaVersion !== LAYOUT_SCHEMA_VERSION) issues.push({ path: "schemaVersion", code: "schemaVersion" });
  if (!isValidLayoutId(doc.id)) issues.push({ path: "id", code: "id" });
  if (!isValidLayoutName(doc.name)) issues.push({ path: "name", code: "name" });
  if (doc.basedOn !== undefined && !isBuiltinLayoutId(doc.basedOn)) issues.push({ path: "basedOn", code: "basedOn" });
  if (!isObj(doc.dock)) issues.push({ path: "dock", code: "dock" });
  else if (new TextEncoder().encode(JSON.stringify(doc.dock)).length > MAX_DOCK_BYTES) issues.push({ path: "dock", code: "dockSize" });
  if (doc.panels.length > MAX_PANELS) issues.push({ path: "panels", code: "panels" });
  doc.panels.forEach((p, i) => {
    if (!isValidPanelId(p)) issues.push({ path: `panels.${i}`, code: "panelId" });
  });
  const w = doc.windows;
  if (!rectOk(w.mainBounds)) issues.push({ path: "windows.mainBounds", code: "bounds" });
  if (w.popouts.length > MAX_POPOUTS) issues.push({ path: "windows.popouts", code: "popouts" });
  const seen = new Set<string>();
  w.popouts.forEach((p, i) => {
    if (!isValidPanelId(p.panelId)) issues.push({ path: `windows.popouts.${i}.panelId`, code: "panelId" });
    else if (seen.has(p.panelId)) issues.push({ path: `windows.popouts.${i}.panelId`, code: "duplicate" });
    seen.add(p.panelId);
    if (!rectOk(p.bounds)) issues.push({ path: `windows.popouts.${i}.bounds`, code: "bounds" });
  });
  if (doc.panels.length === 0 && w.popouts.length === 0) issues.push({ path: "panels", code: "empty" });
  return issues;
}

function readRect(v: unknown): LayoutRect | undefined {
  if (!isObj(v)) return undefined;
  const { x, y, width, height } = v;
  if (![x, y, width, height].every((n) => typeof n === "number" && Number.isFinite(n))) return undefined;
  return { x: Math.round(x as number), y: Math.round(y as number), width: Math.round(width as number), height: Math.round(height as number) };
}

function readPopouts(v: unknown): PopoutWindowSpec[] {
  if (!Array.isArray(v)) return [];
  const out: PopoutWindowSpec[] = [];
  for (const p of v) {
    if (!isObj(p) || typeof p["panelId"] !== "string") continue;
    const spec: PopoutWindowSpec = { panelId: p["panelId"], maximized: p["maximized"] === true };
    if (typeof p["projectId"] === "string") spec.projectId = p["projectId"];
    if (typeof p["monitor"] === "string") spec.monitor = p["monitor"];
    const b = readRect(p["bounds"]);
    if (b) spec.bounds = b;
    out.push(spec);
  }
  return out;
}

function readWindows(v: unknown): WindowArrangement {
  const w: WindowArrangement = { mainMaximized: false, popouts: [] };
  if (!isObj(v)) return w;
  if (typeof v["mainMonitor"] === "string") w.mainMonitor = v["mainMonitor"];
  const b = readRect(v["mainBounds"]);
  if (b) w.mainBounds = b;
  w.mainMaximized = v["mainMaximized"] === true;
  w.popouts = readPopouts(v["popouts"]);
  return w;
}

/**
 * Upgrades any known layout shape to the current [`LayoutDocument`]:
 * - v0: a bare dockview `SerializedDockview` (the pre-Phase-3 localStorage cache);
 * - v1: the current document.
 * Unknown/future versions return `null` (never guess).
 */
export function migrateLayoutDocument(raw: unknown, fallback: { id: string; name: string }): LayoutDocument | null {
  if (!isObj(raw)) return null;
  const version = raw["schemaVersion"];
  if (version === undefined && isObj(raw["grid"]) && isObj(raw["panels"])) {
    return {
      schemaVersion: LAYOUT_SCHEMA_VERSION,
      id: fallback.id,
      name: fallback.name,
      dock: asJson(raw),
      panels: Object.keys(raw["panels"]),
      locked: false,
      windows: { mainMaximized: false, popouts: [] },
    };
  }
  if (version !== LAYOUT_SCHEMA_VERSION) return null;
  if (typeof raw["id"] !== "string" || typeof raw["name"] !== "string" || !isObj(raw["dock"])) return null;
  const doc: LayoutDocument = {
    schemaVersion: LAYOUT_SCHEMA_VERSION,
    id: raw["id"],
    name: raw["name"],
    dock: asJson(raw["dock"]),
    panels: Array.isArray(raw["panels"]) ? raw["panels"].filter((p): p is string => typeof p === "string") : [],
    locked: raw["locked"] === true,
    windows: readWindows(raw["windows"]),
  };
  if (typeof raw["basedOn"] === "string") doc.basedOn = raw["basedOn"];
  if (typeof raw["updatedAt"] === "string") doc.updatedAt = raw["updatedAt"];
  return doc;
}

/**
 * Cleans a document for this build: sanitises the dock against the registry
 * and drops detached windows of unknown panels. Returns `null` if nothing usable remains.
 */
export function repairLayoutDocument(
  doc: LayoutDocument,
  isKnownPanel: (id: string) => boolean,
): { doc: LayoutDocument; dropped: string[] } | null {
  const clean = sanitizeDock(doc.dock, isKnownPanel);
  const popouts = doc.windows.popouts.filter((p) => isKnownPanel(p.panelId));
  const droppedPopouts = doc.windows.popouts.filter((p) => !isKnownPanel(p.panelId)).map((p) => p.panelId);
  if (!clean && popouts.length === 0) return null;
  const next: LayoutDocument = {
    ...doc,
    dock: clean ? asJson(clean.dock) : doc.dock,
    panels: clean?.panels ?? [],
    windows: { ...doc.windows, popouts },
  };
  return { doc: next, dropped: [...(clean?.dropped ?? []), ...droppedPopouts] };
}

/**
 * What makes two layouts "the same" for autosave: structure, sizes, panels,
 * lock and windows — not the panel titles dockview stores (they follow the
 * UI language) nor `updatedAt`.
 */
export function layoutSignature(doc: LayoutDocument): string {
  /** Key-sorted copy, so property order never matters. */
  const sorted = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(sorted);
    if (!isObj(v)) return v;
    return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted(v[k])]));
  };
  const dockPanels = isObj(doc.dock) && isObj(doc.dock["panels"]) ? doc.dock["panels"] : {};
  const panels = Object.fromEntries(
    Object.entries(dockPanels).map(([id, p]) => {
      if (!isObj(p)) return [id, p];
      const { title: _title, ...rest } = p;
      return [id, sorted(rest)];
    }),
  );
  const dock = isObj(doc.dock) ? sorted({ ...doc.dock, panels: null }) : doc.dock;
  return JSON.stringify([doc.id, dock, sorted(panels), doc.panels, doc.locked, sorted(doc.windows)]);
}

/** Serialises a layout for a file (`.orkestr-layout.json`). Device-specific fields are kept; import re-validates. */
export function exportLayout(doc: LayoutDocument): string {
  const { updatedAt: _updatedAt, ...rest } = doc;
  return JSON.stringify({ format: EXPORT_FORMAT, version: LAYOUT_SCHEMA_VERSION, layout: rest }, null, 2);
}

export type ImportError = "json" | "format" | "version" | "invalid" | "empty";

/** Parses an exported layout; the imported layout always gets a fresh user id. */
export function importLayout(
  text: string,
  opts: { existingIds: readonly string[]; isKnownPanel: (id: string) => boolean },
): { ok: true; doc: LayoutDocument; dropped: string[] } | { ok: false; error: ImportError } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, error: "json" };
  }
  if (!isObj(parsed) || parsed["format"] !== EXPORT_FORMAT || !isObj(parsed["layout"])) return { ok: false, error: "format" };
  if (parsed["version"] !== LAYOUT_SCHEMA_VERSION) return { ok: false, error: "version" };
  const layout = parsed["layout"];
  const name = typeof layout["name"] === "string" && isValidLayoutName(layout["name"]) ? layout["name"].trim() : "Imported";
  const doc = migrateLayoutDocument(layout, { id: "imported", name });
  if (!doc) return { ok: false, error: "invalid" };
  doc.name = name;
  doc.id = newLayoutId(name, opts.existingIds);
  if (doc.basedOn !== undefined && !isBuiltinLayoutId(doc.basedOn)) delete doc.basedOn;
  const repaired = repairLayoutDocument(doc, opts.isKnownPanel);
  if (!repaired) return { ok: false, error: "empty" };
  if (validateLayoutDocument(repaired.doc).length > 0) return { ok: false, error: "invalid" };
  return { ok: true, doc: repaired.doc, dropped: repaired.dropped };
}

/** A fresh user layout id derived from the name (`u-deep-work`, `u-deep-work-2`, `u-k3x9`). */
export function newLayoutId(name: string, existing: readonly string[], random: () => number = Math.random): string {
  const slug = name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 32);
  const base = slug ? `u-${slug}` : `u-${Math.floor(random() * 36 ** 4).toString(36).padStart(4, "0")}`;
  const taken = new Set(existing);
  if (!taken.has(base) && !isBuiltinLayoutId(base)) return base;
  for (let i = 2; i < 1000; i++) {
    const id = `${base.slice(0, 43)}-${i}`;
    if (!taken.has(id)) return id;
  }
  return `u-${Date.now().toString(36)}`;
}
