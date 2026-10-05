import type { AgentInstance, BudgetStatus, Event, ManagerRun, TaskGraph, TaskGraphNode } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { commands, matchesShortcut } from "../../features/commands/registry";
import { applyLanguagePreference } from "../../i18n";
import {
  ARRANGEMENT_KEY,
  type ArrangementStorage,
  arrangementStorage,
  cycleSize,
  defaultArrangement,
  moveWidget,
  moveWidgetBy,
  normalizeArrangement,
  type PlacementDefaults,
  parseArrangement,
  serializeArrangement,
  setArrangementStorage,
  setHidden,
} from "./arrangement";
import { focusGraph, LINGER_MS, visibleAgents } from "./data";
import { alertFromEvent, applyQualityEvent, mergeAlerts, type QualityRun } from "./feed";
import { MissionControlOverlay } from "./index";
import { useMissionControlStore } from "./store";
import { visualBudget } from "./visual";
import { statusGroup } from "./widgets/graph";
import { secondsUntil } from "./widgets/infra";
import { topBudgets } from "./widgets/money";
import { getWidget, registerWidget, widgets } from "./widgets/registry";

const DEFAULTS: PlacementDefaults[] = [
  { id: "a", size: "m", sizes: ["s", "m", "l"] },
  { id: "b", size: "l", sizes: ["l", "full"] },
  { id: "c", size: "s", sizes: ["s"], hiddenByDefault: true },
];

function ev(kind: string, payload: unknown, patch: Partial<Event> = {}): Event {
  return {
    seq: 1,
    id: `e-${kind}-${Math.random().toString(36).slice(2)}`,
    ts: "2026-10-05T10:00:00Z",
    projectId: null,
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    kind,
    payload,
    ...patch,
  } as Event;
}

describe("arrangement", () => {
  it("defaults follow the registry order and hidden flags", () => {
    const a = defaultArrangement(DEFAULTS);
    expect(a.widgets.map((w) => [w.id, w.size, w.hidden])).toEqual([
      ["a", "m", false],
      ["b", "l", false],
      ["c", "s", true],
    ]);
  });

  it("normalizes: drops unknown/duplicate ids, fixes sizes, appends new widgets", () => {
    const raw = {
      version: 1,
      widgets: [
        { id: "b", size: "full", hidden: true },
        { id: "zzz", size: "m" },
        { id: "b", size: "l" },
        { id: "a", size: "xl" },
        "garbage",
      ],
    };
    const a = normalizeArrangement(raw, DEFAULTS);
    expect(a.widgets).toEqual([
      { id: "b", size: "full", hidden: true },
      { id: "a", size: "m", hidden: false },
      { id: "c", size: "s", hidden: true },
    ]);
  });

  it("falls back to the default for garbage and wrong versions", () => {
    expect(parseArrangement("{not json", DEFAULTS)).toEqual(defaultArrangement(DEFAULTS));
    expect(parseArrangement(null, DEFAULTS)).toEqual(defaultArrangement(DEFAULTS));
    expect(parseArrangement(JSON.stringify({ version: 99, widgets: [] }), DEFAULTS)).toEqual(defaultArrangement(DEFAULTS));
  });

  it("round-trips through serialization", () => {
    const a = setHidden(cycleSize(defaultArrangement(DEFAULTS), "a", ["s", "m", "l"]), "b", true);
    expect(parseArrangement(serializeArrangement(a), DEFAULTS)).toEqual(a);
  });

  it("moves widgets by drop target and by keyboard among visible ones", () => {
    const a = defaultArrangement(DEFAULTS);
    expect(moveWidget(a, "b", "a").widgets.map((w) => w.id)).toEqual(["b", "a", "c"]);
    expect(moveWidget(a, "a", "a")).toBe(a);
    expect(moveWidget(a, "a", "missing")).toBe(a);
    // c is hidden: moving a forward lands on b (last visible).
    expect(moveWidgetBy(a, "a", 5).widgets.map((w) => w.id)).toEqual(["b", "a", "c"]);
    expect(moveWidgetBy(a, "a", -1).widgets.map((w) => w.id)).toEqual(["a", "b", "c"]);
  });

  it("cycles through supported sizes and wraps", () => {
    let a = defaultArrangement(DEFAULTS);
    a = cycleSize(a, "a", ["s", "m", "l"]);
    expect(a.widgets[0]?.size).toBe("l");
    a = cycleSize(a, "a", ["s", "m", "l"]);
    expect(a.widgets[0]?.size).toBe("s");
  });

  it("persists through a bindable storage (layout area hook)", () => {
    const mem = new Map<string, string>();
    const custom: ArrangementStorage = { load: (k) => mem.get(k) ?? null, save: (k, v) => void mem.set(k, v) };
    setArrangementStorage(custom);
    try {
      expect(arrangementStorage()).toBe(custom);
      const store = useMissionControlStore.getState();
      store.reset();
      const first = useMissionControlStore.getState().arrangement.widgets[0]?.id ?? "";
      const second = useMissionControlStore.getState().arrangement.widgets[1]?.id ?? "";
      store.move(second, first);
      const saved = parseArrangement(mem.get(ARRANGEMENT_KEY) ?? null, widgets());
      expect(saved.widgets[0]?.id).toBe(second);
      useMissionControlStore.setState({ arrangement: defaultArrangement(widgets()) });
      useMissionControlStore.getState().reload();
      expect(useMissionControlStore.getState().arrangement.widgets[0]?.id).toBe(second);
    } finally {
      useMissionControlStore.getState().reset();
      setArrangementStorage(null);
    }
  });
});

describe("widget registry", () => {
  it("has unique ids and valid default sizes", () => {
    const ids = widgets().map((w) => w.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const w of widgets()) {
      expect(w.sizes).toContain(w.size);
      expect(w.title.length).toBeGreaterThan(0);
    }
    expect(ids).toEqual(expect.arrayContaining(["overview", "agents", "approvals", "projects", "taskGraph", "nodes", "providers", "budgets", "alerts", "quality", "deployments"]));
  });

  it("rejects duplicate ids and invalid default sizes", () => {
    const base = getWidget("deployments");
    if (!base) throw new Error("missing widget");
    expect(() => registerWidget({ ...base, title: "x" })).toThrow(/twice/);
    expect(() => registerWidget({ ...base, id: "plugin.x", title: "x", size: "full", sizes: ["s"] })).toThrow(/default size/);
  });

  it("titles are localized", () => {
    applyLanguagePreference("ru");
    expect(getWidget("alerts")?.title).toBe("Оповещения");
    applyLanguagePreference("en");
    expect(getWidget("alerts")?.title).toBe("Alerts");
  });
});

describe("visual budget", () => {
  it("scales effects with the performance mode and reduced motion", () => {
    expect(visualBudget("UltraVisual", false)).toMatchObject({ animate: true, rich: true });
    expect(visualBudget("Balanced", true)).toMatchObject({ animate: false, rich: true });
    expect(visualBudget("Performance", false)).toMatchObject({ animate: false, rich: false });
    expect(visualBudget("BatterySaver", false).chartPoints).toBeLessThan(visualBudget("UltraVisual", false).chartPoints);
  });
});

describe("feed", () => {
  it("maps alert-worthy events and ignores the rest", () => {
    expect(alertFromEvent(ev("BuildFailed", { buildId: "b", exitCode: 2, durationMs: 10, summary: "boom" }))).toMatchObject({
      kind: "buildFailed",
      severity: "critical",
      params: { code: 2 },
      detail: "boom",
    });
    expect(alertFromEvent(ev("TestRunFinished", { runId: "r", passed: 3, failed: 0, skipped: 0, durationMs: 1, summary: "" }))).toBeNull();
    expect(alertFromEvent(ev("BudgetThreshold", { scope: "global", spentUsd: 9, limitUsd: 10, fraction: 0.9 }))?.severity).toBe("warning");
    expect(alertFromEvent(ev("BudgetThreshold", { scope: "global", spentUsd: 12, limitUsd: 10, fraction: 1.2 }))?.severity).toBe("critical");
    expect(alertFromEvent(ev("Log", { level: "info", target: "x", message: "m", fields: null }))).toBeNull();
    expect(alertFromEvent(ev("Log", { level: "error", target: "core", message: "m", fields: null }))?.kind).toBe("coreError");
    expect(alertFromEvent(ev("ProviderRateLimited", { providerId: "p", model: "m", retryAfterSecs: 30 }))?.params).toEqual({ model: "m", secs: 30 });
  });

  it("merges alerts newest first without duplicates", () => {
    const a = alertFromEvent(ev("EmergencyStop", { agentsStopped: 2 }, { id: "1", seq: 5 }));
    const b = alertFromEvent(ev("EmergencyStop", { agentsStopped: 3 }, { id: "2", seq: 9 }));
    if (!a || !b) throw new Error("expected alerts");
    const merged = mergeAlerts(mergeAlerts([], [a]), [b, a]);
    expect(merged.map((x) => x.id)).toEqual(["2", "1"]);
    expect(mergeAlerts(merged, [], 1)).toBe(merged);
  });

  it("pairs build start and finish into one run", () => {
    let runs: QualityRun[] = [];
    runs = applyQualityEvent(runs, ev("BuildStarted", { buildId: "b1", command: "cargo build" }, { seq: 1 }));
    expect(runs[0]).toMatchObject({ status: "running", label: "cargo build" });
    runs = applyQualityEvent(runs, ev("BuildSucceeded", { buildId: "b1", exitCode: 0, durationMs: 1500, summary: "ok" }, { seq: 2, ts: "2026-10-05T10:00:02Z" }));
    expect(runs).toHaveLength(1);
    expect(runs[0]).toMatchObject({ status: "passed", label: "cargo build", durationMs: 1500 });
    runs = applyQualityEvent(runs, ev("TestRunFinished", { runId: "t1", passed: 4, failed: 1, skipped: 0, durationMs: 50, summary: "tests" }, { seq: 3 }));
    expect(runs.map((r) => [r.id, r.status])).toEqual([
      ["t1", "failed"],
      ["b1", "passed"],
    ]);
    expect(applyQualityEvent(runs, ev("Log", { level: "error", target: "", message: "", fields: null }))).toBe(runs);
  });
});

describe("data helpers", () => {
  const node = (id: string, status: TaskGraphNode["status"]): TaskGraphNode => ({
    id,
    title: id,
    status,
    priority: "Normal",
    agentId: null,
    parentId: null,
    estimateMinutes: null,
    downstreamLength: 0,
  });
  const graph: TaskGraph = {
    nodes: [node("a", "Completed"), node("b", "Running"), node("c", "Ready"), node("d", "Cancelled")],
    edges: [
      { taskId: "b", dependsOn: "a" },
      { taskId: "c", dependsOn: "b" },
    ],
    ready: ["c"],
    topoOrder: ["a", "b", "c", "d"],
    criticalPath: ["a", "b", "c"],
    criticalPathLength: 3,
  } as TaskGraph;

  it("focuses the graph on the live run, else on open tasks", () => {
    const run = { tasks: [{ taskId: "a" }, { taskId: "b" }] } as unknown as ManagerRun;
    const live = focusGraph(graph, run);
    expect(live.nodes.map((n) => n.id)).toEqual(["a", "b"]);
    expect(live.edges).toEqual([{ taskId: "b", dependsOn: "a" }]);
    expect(live.ready).toEqual([]);
    const open = focusGraph(graph, null);
    expect(open.nodes.map((n) => n.id)).toEqual(["b", "c"]);
    expect(focusGraph(graph, null, 1).nodes.map((n) => n.id)).toEqual(["c"]);
  });

  it("keeps finished agents visible for a short while", () => {
    const now = Date.parse("2026-10-05T10:10:00Z");
    const mk = (id: string, state: AgentInstance["state"], stoppedAt: string | null) =>
      ({ id, state, stoppedAt, lastActivityAt: null }) as AgentInstance;
    const list = [
      mk("run", "Coding", null),
      mk("fresh", "Completed", new Date(now - LINGER_MS / 2).toISOString()),
      mk("old", "Completed", new Date(now - LINGER_MS * 2).toISOString()),
      mk("unknown", "Stopped", null),
    ];
    expect(visibleAgents(list, now).map((a) => a.id)).toEqual(["run", "fresh"]);
  });

  it("picks the tightest limited budgets", () => {
    const b = (id: string, fraction: number | null, patch: Partial<BudgetStatus["budget"]> = {}) =>
      ({ budget: { id, enabled: true, unrestricted: false, limitUsd: 10, ...patch }, fraction }) as BudgetStatus;
    const top = topBudgets([b("a", 0.2), b("b", 0.9), b("off", 1, { enabled: false }), b("free", 2, { unrestricted: true }), b("c", 0.5)], 2);
    expect(top.map((x) => x.budget.id)).toEqual(["b", "c"]);
  });

  it("maps task statuses to visual groups and counts down rate limits", () => {
    expect(statusGroup("Review")).toBe("running");
    expect(statusGroup("Blocked")).toBe("waiting");
    expect(statusGroup("Interrupted")).toBe("failed");
    expect(statusGroup("Ready")).toBe("idle");
    const now = Date.parse("2026-10-05T10:00:00Z");
    expect(secondsUntil("2026-10-05T10:00:30Z", now)).toBe(30);
    expect(secondsUntil("2026-10-05T09:00:00Z", now)).toBeNull();
    expect(secondsUntil(null, now)).toBeNull();
  });
});

describe("full-screen command", () => {
  beforeEach(() => {
    applyLanguagePreference("en");
    useMissionControlStore.setState({ fullscreen: false, editing: false });
  });
  afterEach(() => {
    cleanup();
    useMissionControlStore.setState({ fullscreen: false });
  });

  it("Ctrl+Shift+D is bound to the Mission Control command", () => {
    const cmd = commands.find((c) => c.id === "view.missionControl");
    expect(cmd?.shortcut).toBe("Ctrl+Shift+D");
    const e = { key: "D", code: "KeyD", ctrlKey: true, metaKey: false, shiftKey: true, altKey: false };
    expect(matchesShortcut(e, cmd?.shortcut ?? "")).toBe(true);
    void cmd?.run({ projectId: null });
    expect(useMissionControlStore.getState().fullscreen).toBe(true);
    void cmd?.run({ projectId: null });
    expect(useMissionControlStore.getState().fullscreen).toBe(false);
  });

  it("the overlay opens with the dashboard and closes on Esc", () => {
    render(<MissionControlOverlay />);
    expect(screen.queryByRole("region", { name: "Mission Control, full screen" })).toBeNull();
    act(() => useMissionControlStore.getState().setFullscreen(true));
    expect(screen.getByRole("region", { name: "Mission Control, full screen" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Mission Control" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Pending approvals" })).toBeTruthy();
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(useMissionControlStore.getState().fullscreen).toBe(false);
  });
});
