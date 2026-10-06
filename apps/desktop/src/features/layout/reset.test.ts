/**
 * Resetting a user layout must be stored in the core: the debounced autosave
 * is suppressed while the reset is applied, so the controller saves it
 * explicitly (regression: the customised layout came back after a restart).
 */
import type { LayoutDocument, LayoutsState } from "@orkestr/protocol";
import type { DockviewApi } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ list: [] as { method: string; params: unknown }[], state: null as unknown }));

vi.mock("../../core/client", () => ({
  maybeClient: () => ({
    call: async (method: string, params: { layout?: LayoutDocument }) => {
      calls.list.push({ method, params });
      const state = calls.state as LayoutsState;
      if (method === "ui.layoutsSave" && params.layout) {
        const layout = params.layout;
        calls.state = { ...state, layouts: [...state.layouts.filter((l) => l.id !== layout.id), layout] };
      }
      return calls.state;
    },
  }),
}));

import { attachDock, detachDock, resetActive } from "./controller";
import { asJson, LAYOUT_SCHEMA_VERSION } from "./schema";
import { useLayoutStore } from "./store";

/** Minimal dockview stand-in: one group holding the panels in insertion order. */
function fakeDock(): DockviewApi {
  let panels: string[] = [];
  const panel = (id: string) => ({
    id,
    group: { api: { location: { type: "grid" }, setSize: () => undefined }, panels: [] },
    api: { setActive: () => undefined, close: () => undefined },
  });
  const dock = {
    width: 0,
    height: 0,
    get panels() {
      return panels.map(panel);
    },
    get groups() {
      return [];
    },
    onDidLayoutChange: () => ({ dispose: () => undefined }),
    onDidMaximizedGroupChange: () => ({ dispose: () => undefined }),
    onDidAddGroup: () => ({ dispose: () => undefined }),
    clear: () => {
      panels = [];
    },
    addPanel: ({ id }: { id: string }) => {
      if (!panels.includes(id)) panels.push(id);
      return panel(id);
    },
    getPanel: (id: string) => (panels.includes(id) ? panel(id) : undefined),
    hasMaximizedGroup: () => false,
    exitMaximizedGroup: () => undefined,
    updateOptions: () => undefined,
    toJSON: () => ({
      grid: { root: { type: "branch", data: [{ type: "leaf", data: { id: "g1", views: [...panels], activeView: panels[0] } }] }, width: 0, height: 0, orientation: "HORIZONTAL" },
      panels: Object.fromEntries(panels.map((p) => [p, { id: p, contentComponent: p, title: p }])),
    }),
    fromJSON: (json: { panels: Record<string, unknown> }) => {
      panels = Object.keys(json.panels);
    },
  };
  return dock as unknown as DockviewApi;
}

function customised(): LayoutDocument {
  return {
    schemaVersion: LAYOUT_SCHEMA_VERSION,
    id: "my-layout",
    name: "My layout",
    basedOn: "coding",
    dock: asJson({
      grid: { root: { type: "branch", data: [{ type: "leaf", data: { id: "g1", views: ["logs"], activeView: "logs" } }] }, width: 0, height: 0, orientation: "HORIZONTAL" },
      panels: { logs: { id: "logs", contentComponent: "logs", title: "logs" } },
    }),
    panels: [],
    locked: false,
    windows: { popouts: [], mainMaximized: false },
  } as LayoutDocument;
}

afterEach(() => {
  detachDock();
  calls.list = [];
});

describe("reset of a user layout", () => {
  it("stores the reset arrangement in the core", async () => {
    calls.state = { deviceId: "d1", activeId: "my-layout", layouts: [customised()] } satisfies LayoutsState;
    await attachDock(fakeDock());
    expect(useLayoutStore.getState().activeId).toBe("my-layout");
    calls.list = [];

    await resetActive();

    const saves = calls.list.filter((c) => c.method === "ui.layoutsSave");
    expect(saves).toHaveLength(1);
    const saved = (saves[0]?.params as { layout: LayoutDocument }).layout;
    expect(saved.id).toBe("my-layout");
    expect(saved.basedOn).toBe("coding");
    // The base preset's panels, not the customised single "logs" panel.
    const stored = (calls.state as LayoutsState).layouts.find((l) => l.id === "my-layout");
    expect(Object.keys((stored?.dock as { panels: Record<string, unknown> }).panels).length).toBeGreaterThan(1);
    expect(useLayoutStore.getState().activeId).toBe("my-layout");
  });
});
