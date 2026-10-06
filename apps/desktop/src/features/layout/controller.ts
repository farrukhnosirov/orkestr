/**
 * Layout controller: the only module (with Workspace.tsx) that touches the
 * dockview API (ADR-0013). Applies presets/stored layouts, autosaves the
 * current layout per device through the core (`ui.layouts*`), detaches
 * panels into OS windows, floats them, assigns them to monitors and watches
 * for monitor changes (fallback to the primary monitor).
 */
import type { LayoutDocument, LayoutsState } from "@orkestr/protocol";
import type { DockviewApi, IDockviewPanel } from "dockview-react";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { getPanel } from "../../panels/registry";
import { useAppStore } from "../../store/app";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { applicableSpread, type BuiltinPreset, getBuiltinPreset, resolveSlots } from "./presets";
import {
  asJson,
  exportLayout as exportLayoutText,
  importLayout as parseImportedLayout,
  isBuiltinLayoutId,
  LAYOUT_SCHEMA_VERSION,
  layoutSignature,
  LEGACY_LAYOUT_KEY,
  newLayoutId,
  sanitizeDock,
  type SerializedDock,
} from "./schema";
import { captureArrangement, type LayoutPlan, monitorsChanged, planLayout, planStartup } from "./selection";
import { useLayoutStore } from "./store";
import { canDetach, type RedockPayload, windowsApi } from "./windowsApi";

const SAVE_DEBOUNCE_MS = 800;
const MONITOR_POLL_MS = 4000;

const isKnownPanel = (id: string): boolean => getPanel(id) !== undefined;
const notify = (...args: Parameters<ReturnType<typeof useNotificationsStore.getState>["notify"]>) =>
  useNotificationsStore.getState().notify(...args);
const store = () => useLayoutStore.getState();

let api: DockviewApi | null = null;
let applying = 0;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let windowsTimer: ReturnType<typeof setTimeout> | null = null;
let monitorTimer: ReturnType<typeof setInterval> | null = null;
let disposers: (() => void)[] = [];
let started = false;
/** The active layout changed since it was applied or saved (only then is it stored — visiting a preset never "customises" it). */
let dirty = false;
/** Signature of the active layout as applied / last saved. */
let baseline: string | null = null;

/**
 * Grid group sizes from just before a group was maximized. dockview's
 * "exit maximized" does not give the other groups their sizes back (a
 * restored side column could shrink to a sliver), so they are put back
 * from here.
 */
let preMaximize: { id: string; width: number; height: number }[] | null = null;

// ── core persistence ─────────────────────────────────────────────────────────

async function rpc<T>(fn: (c: NonNullable<ReturnType<typeof maybeClient>>) => Promise<T>): Promise<T | null> {
  const c = maybeClient();
  if (!c) return null;
  try {
    const result = await fn(c);
    return result;
  } catch (e) {
    store().patch({ persistError: describeError(e) });
    console.warn("layout persistence failed", e);
    return null;
  }
}

function adopt(state: LayoutsState | null): void {
  if (state) store().setState(state);
}

// ── dock helpers ─────────────────────────────────────────────────────────────

function addRegistryPanel(dock: DockviewApi, panelId: string, position?: Parameters<DockviewApi["addPanel"]>[0]["position"]): IDockviewPanel | null {
  const def = getPanel(panelId);
  if (!def) return null;
  const existing = dock.getPanel(panelId);
  if (existing) return existing;
  return dock.addPanel({
    id: def.id,
    component: def.id,
    tabComponent: "ok",
    title: def.title,
    params: { panelId: def.id },
    ...(position ? { position } : {}),
  });
}

function syncDockSummary(): void {
  if (!api) return;
  const dockPanels = api.panels.map((p) => p.id);
  const floatingPanels = api.panels.filter((p) => p.group.api.location.type === "floating").map((p) => p.id);
  store().patch({ dockPanels, floatingPanels });
}

function buildPreset(dock: DockviewApi, preset: BuiltinPreset): void {
  dock.clear();
  for (const slot of resolveSlots(preset.slots, isKnownPanel)) {
    addRegistryPanel(dock, slot.panel, slot.position ? { referencePanel: slot.position.referencePanel, direction: slot.position.direction } : undefined);
  }
  if (dock.width > 0 && dock.height > 0) {
    for (const s of preset.sizes) {
      const group = dock.getPanel(s.panel)?.group;
      if (!group) continue;
      if (s.width) group.api.setSize({ width: Math.round(dock.width * s.width) });
      if (s.height) group.api.setSize({ height: Math.round(dock.height * s.height) });
    }
  }
  // Every group shows its first tab; then the preset's focus panel gets focus.
  for (const group of dock.groups) group.panels[0]?.api.setActive();
  const focus = preset.focus.find((id) => dock.getPanel(id)) ?? dock.panels[0]?.id;
  if (focus) dock.getPanel(focus)?.api.setActive();
}

function setDockLocked(locked: boolean): void {
  api?.updateOptions({ locked, disableDnd: locked });
  store().patch({ locked });
}

// ── widget state (per layout, per device) ───────────────────────────────────

/**
 * View state panels keep with the active layout (`LayoutDocument.widgets`),
 * e.g. the Mission Control widget arrangement: stored per device in the core
 * with the layout, restored when the layout is applied.
 */
let widgetState: Record<string, string> = {};
const widgetListeners = new Set<() => void>();

export function getWidgetState(key: string): string | null {
  return widgetState[key] ?? null;
}

/** Updates one panel's view state and autosaves the active layout. */
export function setWidgetState(key: string, value: string | null): void {
  const next = { ...widgetState };
  if (value === null) delete next[key];
  else next[key] = value;
  widgetState = next;
  scheduleSave();
}

/** Called after a layout is applied (its widget state replaced the previous one). */
export function onWidgetStateReplaced(listener: () => void): () => void {
  widgetListeners.add(listener);
  return () => widgetListeners.delete(listener);
}

function replaceWidgetState(doc: LayoutDocument | null): void {
  widgetState = { ...(doc?.widgets ?? {}) };
  widgetListeners.forEach((l) => l());
}

// ── capture / apply ──────────────────────────────────────────────────────────

/** The current layout as a document (id/name of the active layout). */
export async function captureCurrent(): Promise<LayoutDocument | null> {
  if (!api) return null;
  const s = store();
  const clean = sanitizeDock(api.toJSON(), isKnownPanel);
  const [panels, main] = await Promise.all([windowsApi.listPanels(), s.mainPinned ? windowsApi.mainPlacement() : Promise.resolve(null)]);
  const windows = captureArrangement(main, panels);
  if (!clean && windows.popouts.length === 0) return null;
  const id = s.activeId;
  const existing = s.state?.layouts.find((l) => l.id === id);
  const builtin = getBuiltinPreset(id);
  const doc: LayoutDocument = {
    schemaVersion: LAYOUT_SCHEMA_VERSION,
    id,
    name: existing?.name ?? builtin?.storedName ?? id,
    dock: asJson(clean?.dock ?? emptyDock()),
    panels: clean?.panels ?? [],
    locked: s.locked,
    windows,
  };
  const basedOn = isBuiltinLayoutId(id) ? id : existing?.basedOn;
  if (basedOn) doc.basedOn = basedOn;
  if (Object.keys(widgetState).length > 0) doc.widgets = { ...widgetState };
  return doc;
}

function emptyDock(): SerializedDock {
  return { grid: { root: { type: "branch", data: [] }, width: 0, height: 0, orientation: "HORIZONTAL" }, panels: {} };
}

async function saveNow(): Promise<void> {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (!store().loaded || !dirty) return;
  dirty = false;
  const doc = await captureCurrent();
  if (!doc) return;
  const sig = layoutSignature(doc);
  // Title refreshes (language switch) and no-op events do not customise a preset.
  if (sig === baseline) return;
  baseline = sig;
  adopt(await rpc((c) => c.call("ui.layoutsSave", { layout: doc, activate: false })));
}

/** Debounced autosave of the active layout (per device, in the core). */
export function scheduleSave(): void {
  if (applying > 0 || !store().loaded) return;
  dirty = true;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void saveNow();
  }, SAVE_DEBOUNCE_MS);
}

async function restoreWindows(doc: LayoutDocument | null): Promise<void> {
  if (!canDetach()) return;
  const wanted = new Map((doc?.windows.popouts ?? []).map((p) => [p.panelId, p]));
  const open = await windowsApi.listPanels();
  // Windows that this layout does not have go back into the dock... unless the dock has them already.
  for (const w of open) {
    if (w.panelId && !wanted.has(w.panelId)) await windowsApi.closePanel(w.label);
  }
  const fellBack: string[] = [];
  const project = useAppStore.getState().activeProjectId;
  for (const spec of wanted.values()) {
    if (!isKnownPanel(spec.panelId)) continue;
    api?.getPanel(spec.panelId)?.api.close();
    try {
      const info = await windowsApi.openPanel({
        panelId: spec.panelId,
        projectId: spec.projectId ?? project,
        title: getPanel(spec.panelId)?.title ?? spec.panelId,
        monitor: spec.monitor ?? null,
        bounds: spec.bounds ?? null,
        maximized: spec.maximized,
      });
      if (info?.fellBack) fellBack.push(getPanel(spec.panelId)?.title ?? spec.panelId);
    } catch (e) {
      console.warn("could not restore detached panel", spec.panelId, e);
    }
  }
  if (doc?.windows.mainMonitor) {
    await windowsApi.moveToMonitor("main", doc.windows.mainMonitor, doc.windows.mainBounds ?? null, doc.windows.mainMaximized).catch(() => null);
  }
  store().patch({ mainPinned: !!doc?.windows.mainMonitor, mainMonitor: doc?.windows.mainMonitor ?? null });
  if (fellBack.length > 0) {
    notify("Warning", t("layout:toast.monitorMissingTitle"), t("layout:toast.monitorMissingBody", { panels: fellBack.join(", ") }));
  }
  await refreshWindows();
}

async function applyPlan(plan: LayoutPlan, opts: { restoreWindows: boolean }): Promise<void> {
  if (!api) return;
  const dock = api;
  applying++;
  try {
    preMaximize = null; // a new layout replaces the groups
    if (dock.hasMaximizedGroup()) dock.exitMaximizedGroup();
    let doc: LayoutDocument | null = null;
    if (plan.kind === "stored" || plan.kind === "legacy") {
      try {
        dock.fromJSON(plan.doc.dock as unknown as Parameters<DockviewApi["fromJSON"]>[0]);
        doc = plan.doc;
      } catch (e) {
        console.warn("stored layout could not be applied; using the built-in preset", e);
        notify("Warning", t("layout:toast.brokenTitle"), t("layout:toast.brokenBody"));
      }
      if (plan.dropped.length > 0) console.info("layout: dropped unknown panels", plan.dropped);
    }
    if (!doc) {
      const preset = getBuiltinPreset(plan.id) ?? getBuiltinPreset(plan.kind === "stored" ? (plan.doc.basedOn ?? "coding") : "coding");
      if (preset) buildPreset(dock, preset);
    }
    store().patch({ activeId: plan.id });
    dirty = false;
    setDockLocked(doc?.locked ?? false);
    replaceWidgetState(doc);
    if (opts.restoreWindows) await restoreWindows(doc);
    syncDockSummary();
  } finally {
    // dockview emits layout changes synchronously and on the next frame: ignore both.
    setTimeout(() => {
      applying = Math.max(0, applying - 1);
      // What was just applied is the baseline: only real edits are stored.
      void captureCurrent().then((d) => {
        baseline = d ? layoutSignature(d) : null;
      });
    }, 50);
  }
}

// ── lifecycle ────────────────────────────────────────────────────────────────

function readLegacy(): unknown {
  try {
    const raw = window.localStorage.getItem(LEGACY_LAYOUT_KEY);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function clearLegacy(): void {
  try {
    window.localStorage.removeItem(LEGACY_LAYOUT_KEY);
  } catch {
    /* storage unavailable */
  }
}

/** Called by the Workspace when dockview is ready. */
export async function attachDock(dock: DockviewApi): Promise<void> {
  api = dock;
  const sub = dock.onDidLayoutChange(() => {
    syncDockSummary();
    scheduleSave();
  });
  disposers.push(() => sub.dispose());
  const maxSub = dock.onDidMaximizedGroupChange(() => onMaximizedChange(dock));
  disposers.push(() => maxSub.dispose());
  // Something to show immediately while the core answers.
  applying++;
  buildPreset(dock, getBuiltinPreset(store().activeId) ?? getBuiltinPreset("coding")!);
  setTimeout(() => (applying = Math.max(0, applying - 1)), 50);
  syncDockSummary();

  const state = await rpc((c) => c.call("ui.layoutsList", {}));
  if (api !== dock) return;
  adopt(state);
  const legacy = readLegacy();
  const plan = planStartup(state, legacy, isKnownPanel);
  await applyPlan(plan, { restoreWindows: true });
  store().patch({ loaded: state !== null });
  if (plan.kind === "legacy") {
    adopt(await rpc((c) => c.call("ui.layoutsSave", { layout: plan.doc, activate: true })));
  }
  if (legacy && state) clearLegacy();
  startWatchers();
}

/** Called by the Workspace on unmount. */
export function detachDock(): void {
  for (const d of disposers) d();
  disposers = [];
  if (monitorTimer) clearInterval(monitorTimer);
  monitorTimer = null;
  started = false;
  api = null;
}

async function refreshWindows(): Promise<void> {
  const popouts = await windowsApi.listPanels().catch(() => []);
  store().patch({ popouts });
}

async function refreshMonitors(initial = false): Promise<void> {
  const monitors = await windowsApi.listMonitors().catch(() => []);
  const prev = store().monitors;
  if (!initial && !monitorsChanged(prev, monitors)) return;
  store().patch({ monitors });
  if (initial) return;
  // A monitor went away or changed: rescue stranded windows (fallback to primary).
  const moved = await windowsApi.reconcile().catch(() => []);
  if (monitors.length < prev.length || moved.length > 0) {
    notify("Warning", t("layout:toast.monitorRemovedTitle"), t("layout:toast.monitorRemovedBody", { count: moved.length }));
  } else if (monitors.length > prev.length) {
    notify("Info", t("layout:toast.monitorAddedTitle"), t("layout:toast.monitorAddedBody", { count: monitors.length }));
  }
  await refreshWindows();
}

function startWatchers(): void {
  if (started) return;
  started = true;
  void refreshMonitors(true);
  if (!canDetach()) return;
  monitorTimer = setInterval(() => {
    if (typeof document !== "undefined" && document.hidden) return;
    void refreshMonitors();
  }, MONITOR_POLL_MS);
  void windowsApi.onRedock((p) => void onRedock(p)).then((u) => disposers.push(u));
  void windowsApi.onPanelClosed(() => {
    void refreshWindows().then(scheduleSave);
  }).then((u) => disposers.push(u));
  void windowsApi.onWindowsChanged(() => {
    if (windowsTimer) clearTimeout(windowsTimer);
    windowsTimer = setTimeout(() => void refreshWindows().then(scheduleSave), 300);
  }).then((u) => disposers.push(u));
}

async function onRedock(p: RedockPayload): Promise<void> {
  if (!api) return;
  openPanel(p.panelId);
  await refreshWindows();
  scheduleSave();
}

// ── public actions (commands, editor, workspace) ─────────────────────────────

/** Opens (or focuses) a panel: in its detached window if it has one, else in the dock. */
export function openPanel(panelId: string): void {
  const popout = store().popouts.find((w) => w.panelId === panelId);
  if (popout) {
    void windowsApi.openPanel({ panelId, projectId: popout.projectId });
    return;
  }
  if (!api) return;
  const existing = api.getPanel(panelId);
  if (existing) {
    existing.api.setActive();
    return;
  }
  const anchor = api.activePanel;
  addRegistryPanel(api, panelId, anchor ? { referencePanel: anchor.id, direction: "within" } : undefined);
}

/** Hides a panel (closes its tab or its window). */
export async function hidePanel(panelId: string): Promise<void> {
  const popout = store().popouts.find((w) => w.panelId === panelId);
  if (popout) {
    await windowsApi.closePanel(popout.label);
    await refreshWindows();
    scheduleSave();
    return;
  }
  api?.getPanel(panelId)?.api.close();
}

export function captureGroupSizes(dock: DockviewApi): { id: string; width: number; height: number }[] {
  return dock.groups
    .filter((g) => g.api.location.type === "grid")
    .map((g) => ({ id: g.id, width: g.api.width, height: g.api.height }));
}

export function restoreGroupSizes(dock: DockviewApi, sizes: { id: string; width: number; height: number }[]): void {
  // Two passes: resizing one group of a split moves its neighbours, the
  // second pass settles every group on its recorded size.
  for (let pass = 0; pass < 2; pass++) {
    for (const s of sizes) {
      const g = dock.groups.find((x) => x.id === s.id);
      if (!g || g.api.location.type !== "grid" || s.width <= 0 || s.height <= 0) continue;
      if (g.api.width !== s.width || g.api.height !== s.height) g.api.setSize({ width: s.width, height: s.height });
    }
  }
}

/** After the maximized group was restored (by any path): the sizes from before. */
function onMaximizedChange(dock: DockviewApi): void {
  if (dock.hasMaximizedGroup() || !preMaximize) return;
  const sizes = preMaximize;
  preMaximize = null;
  restoreGroupSizes(dock, sizes);
}

export function toggleMaximize(panelId?: string): void {
  if (!api) return;
  if (api.hasMaximizedGroup()) {
    api.exitMaximizedGroup();
    return;
  }
  const panel = panelId ? api.getPanel(panelId) : api.activePanel;
  if (!panel) return;
  preMaximize = captureGroupSizes(api);
  api.maximizeGroup(panel);
}

/** Floats a panel as an in-app window (dockview floating group). */
export function floatPanel(panelId?: string): void {
  if (!api || store().locked) return;
  const panel = panelId ? api.getPanel(panelId) : api.activePanel;
  if (!panel || panel.group.api.location.type === "floating") return;
  const width = Math.min(640, Math.max(360, Math.round(api.width * 0.42)));
  const height = Math.min(480, Math.max(260, Math.round(api.height * 0.5)));
  api.addFloatingGroup(panel, { width, height, position: { top: 56, left: Math.max(24, Math.round((api.width - width) / 2)) } });
}

/** Returns a floating panel to the grid. */
export function dockPanel(panelId: string): void {
  if (!api) return;
  const panel = api.getPanel(panelId);
  if (!panel || panel.group.api.location.type !== "floating") return;
  const target = api.groups.find((g) => g.api.location.type === "grid");
  if (target) panel.api.moveTo({ group: target, position: "center" });
}

/**
 * Detaches a panel into its own frameless window (optionally on a monitor).
 * Outside the desktop shell it floats inside the app instead.
 */
export async function detachPanel(panelId?: string, opts: { monitor?: string | null; maximized?: boolean } = {}): Promise<void> {
  if (store().locked) return;
  const id = panelId ?? api?.activePanel?.id;
  if (!id) return;
  if (!canDetach()) {
    floatPanel(id);
    return;
  }
  const def = getPanel(id);
  if (!def) return;
  const existing = store().popouts.find((w) => w.panelId === id);
  if (existing) {
    if (opts.monitor) await windowsApi.moveToMonitor(existing.label, opts.monitor, null, opts.maximized ?? true);
    await refreshWindows();
    scheduleSave();
    return;
  }
  const hadPanel = !!api?.getPanel(id);
  api?.getPanel(id)?.api.close();
  try {
    const info = await windowsApi.openPanel({
      panelId: id,
      projectId: useAppStore.getState().activeProjectId,
      title: def.title,
      monitor: opts.monitor ?? null,
      maximized: opts.maximized ?? false,
    });
    if (info?.fellBack) {
      notify("Warning", t("layout:toast.monitorMissingTitle"), t("layout:toast.monitorMissingBody", { panels: def.title }));
    }
  } catch (e) {
    if (hadPanel) openPanel(id);
    notify("Error", t("layout:toast.detachFailedTitle", { title: def.title }), describeError(e));
  }
  await refreshWindows();
  scheduleSave();
}

/** Brings a detached panel back into the dock. */
export async function redockPanel(panelId: string): Promise<void> {
  const popout = store().popouts.find((w) => w.panelId === panelId);
  if (popout) await windowsApi.closePanel(popout.label);
  openPanel(panelId);
  await refreshWindows();
  scheduleSave();
}

/** Puts a panel into the main window (`"dock"`) or on a monitor (detached, maximized there). */
export async function assignPanel(panelId: string, target: string): Promise<void> {
  if (target === "dock") await redockPanel(panelId);
  else await detachPanel(panelId, { monitor: target, maximized: true });
}

/** Moves the main window to a monitor and pins it there in this layout. */
export async function moveMainToMonitor(monitor: string | null): Promise<void> {
  if (monitor) {
    try {
      const info = await windowsApi.moveToMonitor("main", monitor, null, true);
      store().patch({ mainPinned: true, mainMonitor: info?.monitor ?? monitor });
    } catch (e) {
      notify("Error", t("layout:toast.moveFailedTitle"), describeError(e));
      return;
    }
  } else {
    store().patch({ mainPinned: false, mainMonitor: null });
  }
  scheduleSave();
}

/** "Use all monitors": detaches the active preset's spread panels onto monitors 2..n. */
export async function spreadAcrossMonitors(): Promise<void> {
  const s = store();
  const monitors = s.monitors.length > 0 ? s.monitors : await windowsApi.listMonitors();
  if (!canDetach() || monitors.length < 2) {
    notify("Info", t("layout:toast.singleMonitorTitle"), t("layout:toast.singleMonitorBody"));
    return;
  }
  const base = s.state?.layouts.find((l) => l.id === s.activeId)?.basedOn ?? s.activeId;
  const preset = getBuiltinPreset(base) ?? getBuiltinPreset("coding")!;
  const hints = applicableSpread(preset.spread, monitors.length, isKnownPanel);
  for (const h of hints) {
    const monitor = monitors.find((m) => m.index === h.monitor);
    if (monitor) await detachPanel(h.panel, { monitor: monitor.key, maximized: true });
  }
}

export function setLocked(locked: boolean): void {
  setDockLocked(locked);
  scheduleSave();
}

/** Switches to a layout (saving the current one first). */
export async function switchLayout(id: string): Promise<void> {
  if (!api) return;
  await saveNow();
  const plan = planLayout(store().state, id, isKnownPanel);
  await applyPlan(plan, { restoreWindows: true });
  adopt(await rpc((c) => c.call("ui.layoutsSetActive", { id: plan.id })));
}

/** Re-applies the built-in recipe of the active layout and forgets its customisation. */
export async function resetActive(): Promise<void> {
  const s = store();
  const id = s.activeId;
  const base = isBuiltinLayoutId(id) ? id : (s.state?.layouts.find((l) => l.id === id)?.basedOn ?? "coding");
  if (isBuiltinLayoutId(id)) adopt(await rpc((c) => c.call("ui.layoutsDelete", { id })));
  await applyPlan({ kind: "builtin", id: isBuiltinLayoutId(id) ? id : base }, { restoreWindows: true });
  if (!isBuiltinLayoutId(id)) {
    store().patch({ activeId: id });
    scheduleSave();
  }
}

/** Saves the current arrangement as a new user layout and activates it. */
export async function saveAs(name: string): Promise<boolean> {
  const doc = await captureCurrent();
  if (!doc) return false;
  const s = store();
  const ids = s.state?.layouts.map((l) => l.id) ?? [];
  const base = isBuiltinLayoutId(s.activeId) ? s.activeId : s.state?.layouts.find((l) => l.id === s.activeId)?.basedOn;
  const next: LayoutDocument = { ...doc, id: newLayoutId(name, ids), name: name.trim() };
  if (base) next.basedOn = base;
  else delete next.basedOn;
  const state = await rpc((c) => c.call("ui.layoutsSave", { layout: next, activate: true }));
  if (!state) {
    notify("Error", t("layout:toast.saveFailedTitle"), store().persistError ?? "");
    return false;
  }
  adopt(state);
  store().patch({ activeId: next.id });
  notify("Success", t("layout:toast.savedTitle"), t("layout:toast.savedBody", { name: next.name }));
  return true;
}

export async function renameLayout(id: string, name: string): Promise<boolean> {
  const state = await rpc((c) => c.call("ui.layoutsRename", { id, name }));
  adopt(state);
  return state !== null;
}

/** Deletes a user layout, or resets a built-in one. */
export async function deleteLayout(id: string): Promise<void> {
  if (id === store().activeId) {
    if (isBuiltinLayoutId(id)) {
      await resetActive();
      return;
    }
    adopt(await rpc((c) => c.call("ui.layoutsDelete", { id })));
    await applyPlan(planLayout(store().state, "coding", isKnownPanel), { restoreWindows: true });
    return;
  }
  adopt(await rpc((c) => c.call("ui.layoutsDelete", { id })));
}

/** Exports the current layout to a `.json` file download. */
export async function exportCurrent(): Promise<void> {
  const doc = await captureCurrent();
  if (!doc) return;
  const blob = new Blob([exportLayoutText(doc)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${doc.id}.orkestr-layout.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Imports a layout file as a new user layout and activates it. */
export async function importFromText(text: string): Promise<{ ok: true } | { ok: false; error: string }> {
  const ids = store().state?.layouts.map((l) => l.id) ?? [];
  const parsed = parseImportedLayout(text, { existingIds: ids, isKnownPanel });
  if (!parsed.ok) return { ok: false, error: parsed.error };
  const state = await rpc((c) => c.call("ui.layoutsSave", { layout: parsed.doc, activate: true }));
  if (!state) return { ok: false, error: "core" };
  adopt(state);
  await applyPlan({ kind: "stored", id: parsed.doc.id, doc: parsed.doc, dropped: parsed.dropped }, { restoreWindows: true });
  return { ok: true };
}

export function openEditor(open = true): void {
  store().patch({ editorOpen: open });
  if (open) void refreshMonitors(true).then(refreshWindows);
}
