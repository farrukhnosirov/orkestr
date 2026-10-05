import type { LayoutDocument, LayoutsState } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { applicableSpread, BUILTIN_PRESETS, getBuiltinPreset, MISSION_CONTROL_PANEL, resolveSlots } from "./presets";
import { parseDetachedRoute } from "./route";
import {
  asJson,
  BUILTIN_LAYOUT_IDS,
  exportLayout,
  importLayout,
  isValidLayoutName,
  LAYOUT_SCHEMA_VERSION,
  MAX_DOCK_BYTES,
  migrateLayoutDocument,
  newLayoutId,
  repairLayoutDocument,
  sanitizeDock,
  validateLayoutDocument,
} from "./schema";
import {
  captureArrangement,
  cycleLayout,
  layoutEntries,
  missingMonitors,
  monitorMapGeometry,
  monitorNumber,
  monitorsChanged,
  type MonitorInfo,
  planLayout,
  planStartup,
  type WindowInfo,
} from "./selection";

const KNOWN = new Set(["code", "manager", "files", "agents", "tasks", "logs", "terminal", "git", "costs", "monitoring"]);
const isKnown = (id: string) => KNOWN.has(id);

/** A dockview layout: [files | (code+manager / logs)] plus one floating group. */
function dock(extraPanels: Record<string, string> = {}) {
  const panel = (id: string, component = id) => ({ id, contentComponent: component, title: id });
  return {
    grid: {
      root: {
        type: "branch",
        data: [
          { type: "leaf", data: { id: "g1", views: ["files"], activeView: "files" }, size: 200 },
          {
            type: "branch",
            data: [
              { type: "leaf", data: { id: "g2", views: ["code", "manager", "ghost"], activeView: "ghost" }, size: 500 },
              { type: "leaf", data: { id: "g3", views: ["logs"], activeView: "logs" }, size: 200 },
            ],
            size: 800,
          },
        ],
        size: 700,
      },
      width: 1000,
      height: 700,
      orientation: "HORIZONTAL",
    },
    panels: {
      files: panel("files"),
      code: panel("code"),
      manager: panel("manager"),
      logs: panel("logs"),
      ghost: panel("ghost", "missionControl"),
      ...Object.fromEntries(Object.entries(extraPanels).map(([id, c]) => [id, panel(id, c)])),
    },
    activeGroup: "g2",
    floatingGroups: [{ data: { id: "f1", views: ["tasks"], activeView: "tasks" }, position: { top: 10, left: 10, width: 300, height: 200 } }],
  };
}

function doc(over: Partial<LayoutDocument> = {}): LayoutDocument {
  return {
    schemaVersion: LAYOUT_SCHEMA_VERSION,
    id: "u-deep-work",
    name: "Deep work",
    basedOn: "coding",
    dock: asJson(dock({ tasks: "tasks" })),
    panels: ["files", "code", "manager", "logs", "tasks"],
    locked: false,
    windows: { mainMaximized: false, popouts: [] },
    ...over,
  };
}

function state(layouts: LayoutDocument[], activeId = "coding"): LayoutsState {
  return { deviceId: "0190a000-0000-7000-8000-000000000001", activeId, layouts };
}

function mon(key: string, x: number, index: number, primary = false, width = 1920): MonitorInfo {
  const bounds = { x, y: 0, width, height: 1080 };
  return { key, name: key, bounds, workArea: { ...bounds, height: 1040 }, scaleFactor: 1, primary, index };
}

describe("sanitizeDock", () => {
  it("drops unknown panels, fixes the active view and keeps tab order", () => {
    const r = sanitizeDock(dock({ tasks: "tasks" }), isKnown);
    expect(r).not.toBeNull();
    expect(r?.panels).toEqual(["files", "code", "manager", "logs", "tasks"]);
    expect(r?.dropped).toEqual(["ghost"]);
    const g2 = (r?.dock.grid.root as { data: { data: { data: { activeView: string } }[] }[] }).data[1]?.data[0]?.data;
    expect(g2?.activeView).toBe("code");
    expect(r?.dock.activeGroup).toBe("g2");
    expect(r?.dock.floatingGroups).toHaveLength(1);
  });

  it("prunes groups and branches that become empty", () => {
    const raw = dock({ tasks: "tasks" });
    const r = sanitizeDock(raw, (id) => id === "files" || id === "tasks");
    expect(r?.panels).toEqual(["files", "tasks"]);
    const root = r?.dock.grid.root as { type: string; data: unknown[] };
    expect(root.data).toHaveLength(1);
    expect(r?.dock.activeGroup).toBeUndefined();
  });

  it("drops panels that no group references and rejects garbage", () => {
    const raw = dock({ orphan: "agents" });
    const r = sanitizeDock(raw, isKnown);
    expect(r?.dock.panels["orphan"]).toBeUndefined();
    expect(r?.dropped).toContain("orphan");
    expect(sanitizeDock(null, isKnown)).toBeNull();
    expect(sanitizeDock({ grid: 1, panels: {} }, isKnown)).toBeNull();
    expect(sanitizeDock(dock(), () => false)).toBeNull();
  });

  it("keeps a layout where everything floats", () => {
    const raw = dock({ tasks: "tasks" });
    const r = sanitizeDock(raw, (id) => id === "tasks");
    expect(r?.panels).toEqual(["tasks"]);
    expect(r?.dock.grid.root).toEqual({ type: "branch", data: [], size: 700 });
  });

  it("does not let a panel appear in two groups", () => {
    const raw = dock();
    raw.grid.root.data[0] = { type: "leaf", data: { id: "g1", views: ["files", "code"], activeView: "code" }, size: 200 };
    const r = sanitizeDock(raw, isKnown);
    expect(r?.panels.filter((p) => p === "code")).toHaveLength(1);
  });
});

describe("validateLayoutDocument", () => {
  it("accepts a valid document", () => {
    expect(validateLayoutDocument(doc())).toEqual([]);
  });

  it("reports every problem with a code", () => {
    const bad = doc({
      schemaVersion: 9,
      id: "Bad Id",
      name: " ",
      basedOn: "nope",
      panels: ["ok", "../x"],
      windows: {
        mainMaximized: false,
        mainBounds: { x: 0, y: 0, width: 0, height: 10 },
        popouts: [
          { panelId: "code", maximized: false },
          { panelId: "code", maximized: false, bounds: { x: 0, y: 0, width: 10, height: 999_999 } },
        ],
      },
    });
    const codes = validateLayoutDocument(bad).map((i) => i.code);
    expect(codes).toEqual(expect.arrayContaining(["schemaVersion", "id", "name", "basedOn", "panelId", "bounds", "duplicate"]));
  });

  it("rejects empty and oversized layouts", () => {
    expect(validateLayoutDocument(doc({ panels: [] })).map((i) => i.code)).toContain("empty");
    const huge = doc({ dock: asJson({ grid: {}, panels: {}, pad: "x".repeat(MAX_DOCK_BYTES) }) });
    expect(validateLayoutDocument(huge).map((i) => i.code)).toContain("dockSize");
  });

  it("validates names", () => {
    expect(isValidLayoutName("Глубокая работа")).toBe(true);
    expect(isValidLayoutName("")).toBe(false);
    expect(isValidLayoutName("a\u0007b")).toBe(false);
    expect(isValidLayoutName("x".repeat(81))).toBe(false);
  });
});

describe("migrateLayoutDocument", () => {
  it("upgrades a bare dockview layout (v0, pre-Phase-3 cache)", () => {
    const d = migrateLayoutDocument(dock(), { id: "coding", name: "Coding" });
    expect(d?.schemaVersion).toBe(LAYOUT_SCHEMA_VERSION);
    expect(d?.id).toBe("coding");
    expect(d?.panels).toContain("ghost");
    expect(d?.windows).toEqual({ mainMaximized: false, popouts: [] });
  });

  it("reads v1 and normalises its window arrangement", () => {
    const raw = {
      ...doc(),
      windows: {
        mainMonitor: "DISPLAY1",
        mainBounds: { x: 1.4, y: 2, width: 800, height: 600 },
        mainMaximized: true,
        popouts: [{ panelId: "logs", monitor: "DISPLAY2", bounds: { x: 1, y: 2, width: "w" } }, { nope: true }],
      },
    };
    const d = migrateLayoutDocument(raw, { id: "x", name: "x" });
    expect(d?.windows.mainBounds).toEqual({ x: 1, y: 2, width: 800, height: 600 });
    expect(d?.windows.mainMaximized).toBe(true);
    expect(d?.windows.popouts).toEqual([{ panelId: "logs", monitor: "DISPLAY2", maximized: false }]);
  });

  it("never guesses future versions", () => {
    expect(migrateLayoutDocument({ ...doc(), schemaVersion: 2 }, { id: "x", name: "x" })).toBeNull();
    expect(migrateLayoutDocument("nope", { id: "x", name: "x" })).toBeNull();
  });
});

describe("repairLayoutDocument", () => {
  it("drops unknown panels and their windows", () => {
    const d = doc({ windows: { mainMaximized: false, popouts: [{ panelId: "missionControl", maximized: true }, { panelId: "git", maximized: false }] } });
    const r = repairLayoutDocument(d, isKnown);
    expect(r?.doc.windows.popouts.map((p) => p.panelId)).toEqual(["git"]);
    expect(r?.dropped).toEqual(expect.arrayContaining(["ghost", "missionControl"]));
    expect(r?.doc.panels).not.toContain("ghost");
  });

  it("returns null when nothing usable remains", () => {
    expect(repairLayoutDocument(doc(), () => false)).toBeNull();
  });
});

describe("export / import", () => {
  it("round-trips and gets a fresh id", () => {
    const text = exportLayout({ ...doc(), updatedAt: "2026-10-05T00:00:00Z" });
    expect(text).not.toContain("updatedAt");
    const r = importLayout(text, { existingIds: ["u-deep-work"], isKnownPanel: isKnown });
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.doc.id).toBe("u-deep-work-2");
    expect(r.doc.name).toBe("Deep work");
    expect(r.dropped).toEqual(["ghost"]);
    expect(validateLayoutDocument(r.doc)).toEqual([]);
  });

  it("explains why a file cannot be imported", () => {
    const opts = { existingIds: [], isKnownPanel: isKnown };
    expect(importLayout("{", opts)).toEqual({ ok: false, error: "json" });
    expect(importLayout("{}", opts)).toEqual({ ok: false, error: "format" });
    expect(importLayout(JSON.stringify({ format: "orkestr.layout", version: 99, layout: {} }), opts)).toEqual({ ok: false, error: "version" });
    expect(importLayout(JSON.stringify({ format: "orkestr.layout", version: 1, layout: { schemaVersion: 1 } }), opts)).toEqual({
      ok: false,
      error: "invalid",
    });
    expect(importLayout(exportLayout(doc()), { existingIds: [], isKnownPanel: () => false })).toEqual({ ok: false, error: "empty" });
  });

  it("drops a basedOn that is not a built-in", () => {
    const text = JSON.stringify({ format: "orkestr.layout", version: 1, layout: { ...doc(), basedOn: "hacker" } });
    const r = importLayout(text, { existingIds: [], isKnownPanel: isKnown });
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.doc.basedOn).toBeUndefined();
  });
});

describe("newLayoutId", () => {
  it("derives a unique id from the name", () => {
    expect(newLayoutId("Deep Work!", [])).toBe("u-deep-work");
    expect(newLayoutId("Deep Work", ["u-deep-work", "u-deep-work-2"])).toBe("u-deep-work-3");
    expect(newLayoutId("Глубокая работа", [], () => 0)).toBe("u-0000");
    for (const id of [newLayoutId("x".repeat(200), []), newLayoutId("Глубокая", [], () => 0.999)]) {
      expect(id).toMatch(/^[a-z][a-z0-9-]{0,47}$/);
    }
  });
});

describe("presets", () => {
  it("has the six built-ins with Ctrl+1..5", () => {
    expect(BUILTIN_PRESETS.map((p) => p.id)).toEqual([...BUILTIN_LAYOUT_IDS]);
    expect(BUILTIN_PRESETS.map((p) => p.shortcut).filter(Boolean)).toEqual(["Ctrl+1", "Ctrl+2", "Ctrl+3", "Ctrl+4", "Ctrl+5"]);
  });

  it("re-anchors slots whose anchor panel is missing", () => {
    const mc = getBuiltinPreset("mission-control");
    expect(mc).toBeDefined();
    if (!mc) return;
    // Without Mission Control, the manager becomes the first panel and the rest anchor on it.
    const slots = resolveSlots(mc.slots, (id) => id !== MISSION_CONTROL_PANEL);
    expect(slots[0]).toEqual({ panel: "manager" });
    expect(slots.find((s) => s.panel === "agents")?.position).toEqual({ referencePanel: "manager", direction: "right" });
    expect(slots.find((s) => s.panel === "monitoring")?.position).toEqual({ referencePanel: "manager", direction: "below" });
    // Every anchor refers to a panel placed earlier.
    slots.forEach((s, i) => {
      if (s.position) expect(slots.slice(0, i).map((x) => x.panel)).toContain(s.position.referencePanel);
    });
  });

  it("every preset resolves to valid slots with the shipped panels", () => {
    for (const p of BUILTIN_PRESETS) {
      const slots = resolveSlots(p.slots, isKnown);
      expect(slots.length).toBeGreaterThan(0);
      expect(new Set(slots.map((s) => s.panel)).size).toBe(slots.length);
      slots.forEach((s, i) => {
        if (i === 0) expect(s.position).toBeUndefined();
        else expect(slots.slice(0, i).map((x) => x.panel)).toContain(s.position?.referencePanel);
      });
    }
  });

  it("spreads only onto existing monitors, one panel each", () => {
    const hints = [
      { panel: "monitoring", monitor: 2 },
      { panel: "logs", monitor: 2 },
      { panel: "browser", monitor: 3 },
      { panel: "costs", monitor: 1 },
    ];
    expect(applicableSpread(hints, 1, isKnown)).toEqual([]);
    expect(applicableSpread(hints, 3, isKnown)).toEqual([{ panel: "monitoring", monitor: 2 }]);
    expect(applicableSpread(hints, 3, () => true)).toEqual([
      { panel: "monitoring", monitor: 2 },
      { panel: "browser", monitor: 3 },
    ]);
  });
});

describe("per-device selection", () => {
  it("uses the stored layout of the device, else the built-in recipe", () => {
    const custom = doc({ id: "coding", name: "Coding", basedOn: "coding" });
    expect(planLayout(state([custom]), "coding", isKnown).kind).toBe("stored");
    expect(planLayout(state([]), "focus", isKnown)).toEqual({ kind: "builtin", id: "focus" });
    expect(planLayout(null, "review", isKnown)).toEqual({ kind: "builtin", id: "review" });
  });

  it("falls back to Coding for a missing or unusable user layout", () => {
    expect(planLayout(state([]), "u-gone", isKnown)).toEqual({ kind: "builtin", id: "coding" });
    const broken = doc({ id: "u-broken" });
    expect(planLayout(state([broken]), "u-broken", () => false)).toEqual({ kind: "builtin", id: "coding" });
  });

  it("migrates the legacy cache only on a fresh device", () => {
    const legacy = dock();
    const fresh = planStartup(state([]), legacy, isKnown);
    expect(fresh.kind).toBe("legacy");
    if (fresh.kind === "legacy") {
      expect(fresh.doc.basedOn).toBe("coding");
      expect(fresh.dropped).toContain("ghost");
    }
    expect(planStartup(state([doc()], "u-deep-work"), legacy, isKnown)).toMatchObject({ kind: "stored", id: "u-deep-work" });
    expect(planStartup(state([], "focus"), legacy, isKnown)).toEqual({ kind: "builtin", id: "focus" });
    expect(planStartup(state([]), "garbage", isKnown)).toEqual({ kind: "builtin", id: "coding" });
  });

  it("lists built-ins first, then user layouts by name", () => {
    const s = state([
      doc({ id: "u-zeta", name: "zeta" }),
      doc({ id: "u-alpha", name: "Alpha", windows: { mainMaximized: false, popouts: [{ panelId: "logs", maximized: false }] } }),
      doc({ id: "focus", name: "Focus", locked: true }),
    ]);
    const entries = layoutEntries(s);
    expect(entries.map((e) => e.id)).toEqual([...BUILTIN_LAYOUT_IDS, "u-alpha", "u-zeta"]);
    expect(entries.find((e) => e.id === "focus")).toMatchObject({ customised: true, locked: true, shortcut: "Ctrl+4" });
    expect(entries.find((e) => e.id === "u-alpha")).toMatchObject({ builtin: false, popouts: 1 });
  });

  it("cycles through layouts and wraps", () => {
    const s = state([doc({ id: "u-a", name: "a" })]);
    expect(cycleLayout(s, "coding", 1)).toBe("mission-control");
    expect(cycleLayout(s, "u-a", 1)).toBe("coding");
    expect(cycleLayout(s, "coding", -1)).toBe("u-a");
    expect(cycleLayout(null, "unknown", 1)).toBe("mission-control");
  });
});

describe("multi-monitor", () => {
  const monitors = [mon("DISPLAY1", 0, 1, true), mon("DISPLAY2", 1920, 2), mon("DISPLAY3", 3840, 3)];

  it("captures the window arrangement, one window per panel", () => {
    const w = (panelId: string | null, monitor: string | null, maximized = false): WindowInfo => ({
      label: panelId ? `panel-${panelId}` : "main",
      panelId,
      projectId: panelId ? "p1" : null,
      monitor,
      bounds: { x: 10, y: 20, width: 800, height: 600 },
      maximized,
      fellBack: false,
    });
    const a = captureArrangement(w(null, "DISPLAY1", true), [w("logs", "DISPLAY2"), w("logs", "DISPLAY3"), w(null, null)]);
    expect(a).toEqual({
      mainMonitor: "DISPLAY1",
      mainBounds: { x: 10, y: 20, width: 800, height: 600 },
      mainMaximized: true,
      popouts: [{ panelId: "logs", projectId: "p1", monitor: "DISPLAY2", bounds: { x: 10, y: 20, width: 800, height: 600 }, maximized: false }],
    });
    expect(captureArrangement(null, [])).toEqual({ mainMaximized: false, popouts: [] });
  });

  it("finds monitors a layout needs that are disconnected", () => {
    const arrangement = {
      mainMonitor: "DISPLAY1",
      mainMaximized: false,
      popouts: [
        { panelId: "logs", monitor: "DISPLAY4", maximized: false },
        { panelId: "git", monitor: "DISPLAY4", maximized: false },
        { panelId: "code", maximized: false },
      ],
    };
    expect(missingMonitors(arrangement, monitors)).toEqual(["DISPLAY4"]);
    expect(missingMonitors(arrangement, [])).toEqual(["DISPLAY1", "DISPLAY4"]);
  });

  it("detects plug, unplug and rearrangement but not reordering", () => {
    expect(monitorsChanged(monitors, [...monitors].reverse())).toBe(false);
    expect(monitorsChanged(monitors, monitors.slice(0, 2))).toBe(true);
    expect(monitorsChanged(monitors, [mon("DISPLAY1", 0, 1, true), mon("DISPLAY2", -1920, 2), mon("DISPLAY3", 3840, 3)])).toBe(true);
    expect(monitorsChanged(monitors, [mon("DISPLAY1", 0, 1), mon("DISPLAY2", 1920, 2, true), mon("DISPLAY3", 3840, 3)])).toBe(true);
  });

  it("numbers monitors and scales the map into its box", () => {
    expect(monitorNumber(monitors, "DISPLAY3")).toBe(3);
    expect(monitorNumber(monitors, "nope")).toBeNull();
    expect(monitorNumber(monitors, null)).toBeNull();
    const g = monitorMapGeometry(monitors, { width: 312, height: 150 }, 8);
    expect(g).toHaveLength(3);
    for (const r of g) {
      expect(r.x).toBeGreaterThanOrEqual(0);
      expect(r.x + r.width).toBeLessThanOrEqual(312 + 1e-6);
      expect(r.y + r.height).toBeLessThanOrEqual(150 + 1e-6);
    }
    // Left-to-right order and equal widths are preserved.
    expect(g[0]!.x).toBeLessThan(g[1]!.x);
    expect(g[0]!.width).toBeCloseTo(g[1]!.width);
    expect(monitorMapGeometry([], { width: 10, height: 10 })).toEqual([]);
  });
});

describe("detached route", () => {
  it("parses panel windows and rejects anything else", () => {
    expect(parseDetachedRoute("#/panel/missionControl?projectId=0190a000-0000-7000-8000-000000000001")).toEqual({
      panelId: "missionControl",
      projectId: "0190a000-0000-7000-8000-000000000001",
    });
    expect(parseDetachedRoute("#/panel/logs")).toEqual({ panelId: "logs", projectId: null });
    expect(parseDetachedRoute("#/panel/logs?projectId=../../etc")).toEqual({ panelId: "logs", projectId: null });
    expect(parseDetachedRoute("")).toBeNull();
    expect(parseDetachedRoute("#/panel/../x")).toBeNull();
    expect(parseDetachedRoute("#/settings")).toBeNull();
  });
});
