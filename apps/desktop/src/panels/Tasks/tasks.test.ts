import type { Event, Task, TaskStatus } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { applyTaskEvent } from "./data";
import { assignLayers, countCrossings, layoutDag } from "./layout";
import {
  ALL_STATUSES,
  BOARD_COLUMNS,
  EMPTY_FILTER,
  TRANSITIONS,
  canTransition,
  dropTarget,
  filterTasks,
  formatCost,
  sortTasks,
  upsertTask,
} from "./model";

let seq = 0;
function task(title: string, patch: Partial<Task> = {}): Task {
  seq += 1;
  const n = String(seq).padStart(12, "0");
  return {
    id: `0190f0b0-0000-7000-8000-${n}`,
    projectId: "0190f0b0-0000-7000-8000-ffffffffffff",
    parentId: null,
    title,
    description: "",
    acceptanceCriteria: [],
    dependencies: [],
    priority: "Normal",
    status: "Planned",
    agentId: null,
    model: null,
    worktree: null,
    filesTouched: [],
    executions: [],
    usage: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    budget: null,
    logObjectId: null,
    artifacts: [],
    approvals: [],
    result: null,
    reviewState: "NotRequired",
    attempts: 0,
    tags: [],
    createdAt: `2026-10-02T10:00:${String(seq % 60).padStart(2, "0")}Z`,
    updatedAt: "2026-10-02T10:00:00Z",
    startedAt: null,
    finishedAt: null,
    version: 0,
    estimateMinutes: null,
    readySince: null,
    ...patch,
  };
}

describe("transition table mirror", () => {
  it("has the 41 contract edges and terminal states", () => {
    const edges = ALL_STATUSES.flatMap((s) => TRANSITIONS[s].map((t) => [s, t] as const));
    expect(edges).toHaveLength(41);
    expect(TRANSITIONS.Completed).toEqual([]);
    expect(TRANSITIONS.Cancelled).toEqual([]);
    // Privileged crash-recovery edges are never offered to the user.
    for (const s of ["Waiting", "Review", "Testing"] as TaskStatus[]) expect(canTransition(s, "Interrupted")).toBe(false);
    expect(canTransition("Running", "Interrupted")).toBe(true);
  });

  it("board drops only produce valid transitions", () => {
    for (const from of ALL_STATUSES) {
      for (const col of BOARD_COLUMNS) {
        const to = dropTarget(from, col);
        if (to !== null) expect(canTransition(from, to)).toBe(true);
      }
    }
    const done = BOARD_COLUMNS.find((c) => c.id === "done");
    const ready = BOARD_COLUMNS.find((c) => c.id === "ready");
    expect(done && dropTarget("Planned", done)).toBeNull();
    expect(done && dropTarget("Running", done)).toBe("Completed");
    expect(ready && dropTarget("Failed", ready)).toBe("Ready");
    expect(ready && dropTarget("Ready", ready)).toBeNull();
    // Every status lives in exactly one column.
    for (const s of ALL_STATUSES) expect(BOARD_COLUMNS.filter((c) => c.statuses.includes(s))).toHaveLength(1);
  });
});

describe("sorting, filtering and upserts", () => {
  it("sorts by priority then creation", () => {
    const a = task("a", { priority: "Low" });
    const b = task("b", { priority: "Critical" });
    const c = task("c", { priority: "Critical" });
    expect(sortTasks([a, b, c], "priority", "desc").map((t) => t.title)).toEqual(["b", "c", "a"]);
    expect(sortTasks([c, a, b], "title", "asc").map((t) => t.title)).toEqual(["a", "b", "c"]);
  });

  it("filters by query, status and priority", () => {
    const a = task("Build API", { status: "Ready", tags: ["backend"] });
    const b = task("Write docs", { priority: "Low" });
    expect(filterTasks([a, b], { ...EMPTY_FILTER, query: "BACKEND" })).toEqual([a]);
    expect(filterTasks([a, b], { ...EMPTY_FILTER, statuses: ["Planned"] })).toEqual([b]);
    expect(filterTasks([a, b], { ...EMPTY_FILTER, priorities: ["Low", "High"] })).toEqual([b]);
  });

  it("ignores stale updates and applies events", () => {
    const a = task("a", { version: 3 });
    const stale = { ...a, title: "old", version: 2 };
    expect(upsertTask([a], stale)[0]?.title).toBe("a");
    const fresh = { ...a, title: "new", version: 4 };
    const created = task("b");
    const ev = (kind: string, payload: unknown) => ({ seq: 1, id: "e", ts: "", kind, payload }) as unknown as Event;
    let list = applyTaskEvent([a], ev("TaskUpdated", { task: fresh }));
    expect(list[0]?.title).toBe("new");
    list = applyTaskEvent(list, ev("TaskCreated", { task: created }));
    expect(list).toHaveLength(2);
    list = applyTaskEvent(list, ev("TaskDeleted", { taskId: a.id, projectId: a.projectId }));
    expect(list.map((t) => t.id)).toEqual([created.id]);
    expect(applyTaskEvent(list, ev("Log", {}))).toBe(list);
  });

  it("formats cost", () => {
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(0.004)).toBe("$0.004");
    expect(formatCost(1.234)).toBe("$1.23");
    expect(formatCost(1.234, "ru")).toBe("1,23 $");
  });
});

describe("layered DAG layout", () => {
  it("assigns longest-path layers", () => {
    const layers = assignLayers(["a", "b", "c", "d"], [
      { from: "a", to: "b" },
      { from: "b", to: "d" },
      { from: "a", to: "d" },
      { from: "c", to: "d" },
    ]);
    expect(Object.fromEntries(layers)).toEqual({ a: 0, b: 1, c: 0, d: 2 });
  });

  it("survives cycles and unknown nodes", () => {
    const layers = assignLayers(["a", "b"], [
      { from: "a", to: "b" },
      { from: "b", to: "a" },
      { from: "x", to: "a" },
    ]);
    expect(layers.size).toBe(2);
  });

  it("places prerequisites left of dependents and reduces crossings", () => {
    // Two chains drawn crossed in input order: a1→b2, a2→b1.
    const ids = ["a1", "a2", "b1", "b2"];
    const edges = [
      { from: "a1", to: "b2" },
      { from: "a2", to: "b1" },
    ];
    expect(countCrossings(["a1", "a2"], ["b1", "b2"], edges)).toBe(1);
    const { positions, layers } = layoutDag(ids, edges);
    expect(layers).toHaveLength(2);
    expect(countCrossings(layers[0] ?? [], layers[1] ?? [], edges)).toBe(0);
    for (const e of edges) {
      expect((positions.get(e.from)?.x ?? 0) < (positions.get(e.to)?.x ?? 0)).toBe(true);
    }
    // No two nodes share a position.
    const keys = new Set([...positions.values()].map((p) => `${p.x}:${p.y}`));
    expect(keys.size).toBe(ids.length);
  });

  it("handles an empty graph", () => {
    expect(layoutDag([], []).layers).toEqual([]);
  });
});

describe("tasks translations", () => {
  it("uses Russian plural forms after numerals", () => {
    const ru = (key: "tasks:summary.attention" | "tasks:list.dependencies", count: number) => i18n.t(key, { count, lng: "ru" });
    expect(ru("tasks:summary.attention", 1)).toBe("1 требует внимания");
    expect(ru("tasks:summary.attention", 3)).toBe("3 требуют внимания");
    expect(ru("tasks:summary.attention", 21)).toBe("21 требует внимания");
    expect(ru("tasks:list.dependencies", 1)).toBe("1 зависимость");
    expect(ru("tasks:list.dependencies", 2)).toBe("2 зависимости");
    expect(ru("tasks:list.dependencies", 5)).toBe("5 зависимостей");
    expect(ru("tasks:list.dependencies", 11)).toBe("11 зависимостей");
  });
});
