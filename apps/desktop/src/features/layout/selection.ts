/**
 * Per-device layout selection (§34 "layouts per-device"): which layout to
 * load at startup, how the switcher lists presets, and how the window
 * arrangement is captured / restored across monitors. Pure; tested.
 */
import type { LayoutDocument, LayoutsState, PopoutWindowSpec, WindowArrangement } from "@orkestr/protocol";

import { BUILTIN_PRESETS } from "./presets";
import { BUILTIN_LAYOUT_IDS, isBuiltinLayoutId, migrateLayoutDocument, repairLayoutDocument } from "./schema";

/** What to apply on startup / switch. */
export type LayoutPlan =
  | { kind: "stored"; id: string; doc: LayoutDocument; dropped: string[] }
  | { kind: "builtin"; id: string }
  /** Pre-Phase-3 layout cached by this webview: imported once as the device's Coding layout. */
  | { kind: "legacy"; id: "coding"; doc: LayoutDocument; dropped: string[] };

/**
 * Picks the layout for `id` on this device: its stored (customised)
 * document if usable, otherwise the built-in recipe. A user layout that is
 * gone or unusable falls back to Coding.
 */
export function planLayout(state: LayoutsState | null, id: string, isKnownPanel: (id: string) => boolean): LayoutPlan {
  const stored = state?.layouts.find((l) => l.id === id);
  if (stored) {
    const repaired = repairLayoutDocument(stored, isKnownPanel);
    if (repaired) return { kind: "stored", id, doc: repaired.doc, dropped: repaired.dropped };
  }
  if (isBuiltinLayoutId(id)) return { kind: "builtin", id };
  return planLayout(state, "coding", isKnownPanel);
}

/**
 * Startup plan: the device's active layout; on a device that never saved a
 * layout, the legacy localStorage layout (if any) is migrated as `coding`.
 */
export function planStartup(state: LayoutsState | null, legacy: unknown, isKnownPanel: (id: string) => boolean): LayoutPlan {
  const active = state?.activeId ?? "coding";
  const deviceIsFresh = !state || (state.layouts.length === 0 && active === "coding");
  if (deviceIsFresh && legacy) {
    const doc = migrateLayoutDocument(legacy, { id: "coding", name: "Coding" });
    const repaired = doc ? repairLayoutDocument({ ...doc, basedOn: "coding" }, isKnownPanel) : null;
    if (repaired) return { kind: "legacy", id: "coding", doc: repaired.doc, dropped: repaired.dropped };
  }
  return planLayout(state, active, isKnownPanel);
}

/** One row of the layout switcher. */
export interface LayoutEntry {
  id: string;
  builtin: boolean;
  /** User name (user layouts) — built-ins are shown with their localized name. */
  name: string;
  /** A built-in that has a stored (customised) version on this device. */
  customised: boolean;
  shortcut?: string;
  locked: boolean;
  popouts: number;
}

/** Built-ins first (fixed order), then user layouts by name. */
export function layoutEntries(state: LayoutsState | null): LayoutEntry[] {
  const stored = new Map((state?.layouts ?? []).map((l) => [l.id, l]));
  const entries: LayoutEntry[] = BUILTIN_LAYOUT_IDS.map((id) => {
    const doc = stored.get(id);
    const preset = BUILTIN_PRESETS.find((p) => p.id === id);
    const entry: LayoutEntry = {
      id,
      builtin: true,
      name: id,
      customised: !!doc,
      locked: doc?.locked ?? false,
      popouts: doc?.windows.popouts.length ?? 0,
    };
    if (preset?.shortcut) entry.shortcut = preset.shortcut;
    return entry;
  });
  const users = (state?.layouts ?? [])
    .filter((l) => !isBuiltinLayoutId(l.id))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }))
    .map<LayoutEntry>((l) => ({
      id: l.id,
      builtin: false,
      name: l.name,
      customised: false,
      locked: l.locked,
      popouts: l.windows.popouts.length,
    }));
  return [...entries, ...users];
}

/** Next/previous layout id in switcher order (wraps). */
export function cycleLayout(state: LayoutsState | null, current: string, step: 1 | -1): string {
  const ids = layoutEntries(state).map((e) => e.id);
  const i = ids.indexOf(current);
  const n = ids.length;
  return ids[(((i < 0 ? 0 : i) + step) % n + n) % n] ?? "coding";
}

// ── multi-monitor ────────────────────────────────────────────────────────────

/** A monitor as reported by the shell (`window_list_monitors`). */
export interface MonitorInfo {
  key: string;
  name: string | null;
  bounds: { x: number; y: number; width: number; height: number };
  workArea: { x: number; y: number; width: number; height: number };
  scaleFactor: number;
  primary: boolean;
  /** 1-based, left-to-right. */
  index: number;
}

/** A window as reported by the shell (`window_list_panels` / `window_main_placement`). */
export interface WindowInfo {
  label: string;
  panelId: string | null;
  projectId: string | null;
  monitor: string | null;
  bounds: { x: number; y: number; width: number; height: number } | null;
  maximized: boolean;
  fellBack: boolean;
}

/** Captures the arrangement of the open windows into a layout. */
export function captureArrangement(main: WindowInfo | null, panels: readonly WindowInfo[]): WindowArrangement {
  const arrangement: WindowArrangement = { mainMaximized: main?.maximized ?? false, popouts: [] };
  if (main?.monitor) arrangement.mainMonitor = main.monitor;
  if (main?.bounds) arrangement.mainBounds = main.bounds;
  const seen = new Set<string>();
  for (const w of panels) {
    if (!w.panelId || seen.has(w.panelId)) continue;
    seen.add(w.panelId);
    const spec: PopoutWindowSpec = { panelId: w.panelId, maximized: w.maximized };
    if (w.projectId) spec.projectId = w.projectId;
    if (w.monitor) spec.monitor = w.monitor;
    if (w.bounds) spec.bounds = w.bounds;
    arrangement.popouts.push(spec);
  }
  return arrangement;
}

/** Monitor keys referenced by a layout that are not connected now (fallback to primary). */
export function missingMonitors(arrangement: WindowArrangement, monitors: readonly MonitorInfo[]): string[] {
  const connected = new Set(monitors.map((m) => m.key));
  const wanted = [arrangement.mainMonitor, ...arrangement.popouts.map((p) => p.monitor)].filter(
    (k): k is string => typeof k === "string",
  );
  return [...new Set(wanted.filter((k) => !connected.has(k)))];
}

/** Whether the monitor set changed (plug/unplug, resolution or arrangement change). */
export function monitorsChanged(a: readonly MonitorInfo[], b: readonly MonitorInfo[]): boolean {
  if (a.length !== b.length) return true;
  const sig = (m: MonitorInfo) => `${m.key}|${m.bounds.x},${m.bounds.y},${m.bounds.width}x${m.bounds.height}|${m.primary}`;
  const sa = a.map(sig).sort();
  const sb = b.map(sig).sort();
  return sa.some((s, i) => s !== sb[i]);
}

/** Display label key + number for a monitor key (for "Monitor 2" in the UI). */
export function monitorNumber(monitors: readonly MonitorInfo[], key: string | null | undefined): number | null {
  if (!key) return null;
  return monitors.find((m) => m.key === key)?.index ?? null;
}

/** Scales monitors into a box for the monitor map (keeps aspect ratio). */
export function monitorMapGeometry(
  monitors: readonly MonitorInfo[],
  box: { width: number; height: number },
  padding = 8,
): { key: string; x: number; y: number; width: number; height: number }[] {
  if (monitors.length === 0) return [];
  const minX = Math.min(...monitors.map((m) => m.bounds.x));
  const minY = Math.min(...monitors.map((m) => m.bounds.y));
  const maxX = Math.max(...monitors.map((m) => m.bounds.x + m.bounds.width));
  const maxY = Math.max(...monitors.map((m) => m.bounds.y + m.bounds.height));
  const scale = Math.min((box.width - padding * 2) / (maxX - minX || 1), (box.height - padding * 2) / (maxY - minY || 1));
  const offX = (box.width - (maxX - minX) * scale) / 2;
  const offY = (box.height - (maxY - minY) * scale) / 2;
  return monitors.map((m) => ({
    key: m.key,
    x: offX + (m.bounds.x - minX) * scale,
    y: offY + (m.bounds.y - minY) * scale,
    width: m.bounds.width * scale,
    height: m.bounds.height * scale,
  }));
}
