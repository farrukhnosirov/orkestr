import type { Event, Task, TaskGraph } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useTasksUi } from "./store";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";

function task(id: string, title: string, patch: Partial<Task> = {}): Task {
  return {
    id,
    projectId: PROJECT,
    parentId: null,
    title,
    description: "",
    acceptanceCriteria: [{ text: "works", met: false }],
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
    createdAt: "2026-10-02T10:00:00Z",
    updatedAt: "2026-10-02T10:00:00Z",
    startedAt: null,
    finishedAt: null,
    version: 0,
    estimateMinutes: null,
    readySince: null,
    owner: null,
    ...patch,
  };
}

const A = task("0190f0b0-0000-7000-8000-000000000001", "Design schema", { status: "Ready", priority: "High" });
const B = task("0190f0b0-0000-7000-8000-000000000002", "Build API", { dependencies: [A.id] });

const GRAPH: TaskGraph = {
  nodes: [],
  edges: [{ dependsOn: A.id, taskId: B.id, kind: "FinishToStart" }],
  ready: [A.id],
  topoOrder: [A.id, B.id],
  criticalPath: [A.id, B.id],
  criticalPathLength: 2,
};

let handler: ((ev: Event) => void) | null = null;
const calls: Array<{ method: string; params: unknown }> = [];

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "tasks.list":
        return { tasks: [A, B] };
      case "tasks.graph":
        return GRAPH;
      case "tasks.history":
        return {
          entries: [
            { seq: 1, taskId: A.id, from: null, to: "Planned", actor: { type: "User" }, reason: "created", forced: false, at: A.createdAt },
          ],
          executions: [],
        };
      case "tasks.transition":
        return { task: { ...A, status: "Running", version: 1 }, changes: [] };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handler = h;
    return () => {
      handler = null;
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { default: TasksPanel } = await import("./index");

describe("TasksPanel", () => {
  beforeEach(() => {
    calls.length = 0;
    useAppStore.getState().setConnection({ state: "open" });
    useTasksUi.setState({ view: "list", selectedTaskId: null, createOpen: false, toasts: [] });
  });
  afterEach(() => cleanup());

  it("shows the empty state without a project", () => {
    render(<TasksPanel projectId={null} />);
    expect(screen.getByText("No project open")).toBeTruthy();
  });

  it("lists tasks, applies live events, opens the drawer and transitions", async () => {
    render(<TasksPanel projectId={PROJECT} />);
    await screen.findByText("Design schema");
    expect(screen.getByText("Build API")).toBeTruthy();
    expect(screen.getByText("2 tasks")).toBeTruthy();

    // Live update from the event stream.
    act(() => {
      handler?.({
        seq: 5,
        id: "e1",
        ts: "",
        projectId: PROJECT,
        taskId: B.id,
        agentId: null,
        nodeId: null,
        correlationId: null,
        kind: "TaskUpdated",
        payload: { task: { ...B, title: "Build public API", version: 1 } },
      } as Event);
    });
    await screen.findByText("Build public API");

    // Open the drawer: only allowed transitions are offered (Ready → Running|Blocked|Cancelled|Planned).
    fireEvent.click(screen.getByText("Design schema"));
    const drawer = await screen.findByRole("complementary", { name: "Task Design schema" });
    expect(drawer).toBeTruthy();
    expect(screen.getByRole("button", { name: "Start" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Complete" })).toBeNull();
    await screen.findByText("Created");

    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() => expect(calls.some((c) => c.method === "tasks.transition")).toBe(true));
    expect(calls.find((c) => c.method === "tasks.transition")?.params).toEqual({ taskId: A.id, to: "Running" });
  });

  it("keeps events that arrive while the initial list loads (stale snapshot)", async () => {
    let resolveList: (v: { tasks: Task[] }) => void = () => undefined;
    const base = fakeClient.call.getMockImplementation();
    if (!base) throw new Error("the fake client has no base implementation");
    fakeClient.call.mockImplementation(async (method: string, params: unknown) => {
      if (method !== "tasks.list") return base(method, params);
      calls.push({ method, params });
      return new Promise<{ tasks: Task[] }>((r) => {
        resolveList = r;
      });
    });
    onTestFinished(() => {
      fakeClient.call.mockImplementation(base);
    });
    render(<TasksPanel projectId={PROJECT} />);
    await waitFor(() => expect(handler).not.toBeNull());
    const ev = (seq: number, kind: string, payload: unknown) =>
      ({ seq, id: `e${seq}`, ts: "", projectId: PROJECT, taskId: null, agentId: null, nodeId: null, correlationId: null, kind, payload }) as Event;
    const C = task("0190f0b0-0000-7000-8000-000000000003", "Write docs");
    // Published after the list snapshot was taken, delivered before the response.
    act(() => {
      handler?.(ev(7, "TaskUpdated", { task: { ...B, title: "Build public API", version: 1 } }));
      handler?.(ev(8, "TaskCreated", { task: C }));
      handler?.(ev(9, "TaskDeleted", { taskId: A.id }));
    });
    // Still loading: the events were buffered, not shown on an empty list.
    expect(calls.filter((c) => c.method === "tasks.list")).toHaveLength(1);
    await act(async () => {
      resolveList({ tasks: [A, B] });
      await Promise.resolve();
    });
    await screen.findByText("Build public API");
    expect(screen.getByText("Write docs")).toBeTruthy();
    expect(screen.queryByText("Build API")).toBeNull();
    expect(screen.queryByText("Design schema")).toBeNull();
  });

  it("renders the graph view with nodes and the critical path", async () => {
    class RO {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal("ResizeObserver", RO);
    GRAPH.nodes = [A, B].map((t) => ({
      id: t.id,
      title: t.title,
      status: t.status,
      priority: t.priority,
      agentId: null,
      parentId: null,
      estimateMinutes: null,
      downstreamLength: 1,
    }));
    useTasksUi.setState({ view: "graph" });
    render(<TasksPanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Task dependency graph" });
    expect(screen.getByText(/Critical path: 2\smin · 2 tasks/)).toBeTruthy();
    await screen.findByText("Design schema");
    vi.unstubAllGlobals();
  });

  it("renders the board grouped by status", async () => {
    useTasksUi.setState({ view: "board" });
    render(<TasksPanel projectId={PROJECT} />);
    await screen.findByText("Design schema");
    expect(screen.getByRole("region", { name: "Task board" })).toBeTruthy();
    expect(screen.getByLabelText("Ready, 1 task")).toBeTruthy();
    expect(screen.getByLabelText("Backlog, 1 task")).toBeTruthy();
  });
});

describe("TasksPanel in Russian", () => {
  beforeEach(async () => {
    calls.length = 0;
    useAppStore.getState().setConnection({ state: "open" });
    useTasksUi.setState({ view: "list", selectedTaskId: null, createOpen: false, toasts: [] });
    await act(async () => {
      await applyLanguagePreference("ru");
    });
  });
  afterEach(async () => {
    cleanup();
    await act(async () => {
      await applyLanguagePreference("en");
    });
  });

  it("renders the list, statuses, plurals and transitions in Russian", async () => {
    render(<TasksPanel projectId={PROJECT} />);
    await screen.findByText("Design schema");
    expect(screen.getByText("2 задачи")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Новая задача" })).toBeTruthy();
    expect(screen.getByText("Готова к работе")).toBeTruthy();
    expect(screen.getByText("Высокий")).toBeTruthy();
    fireEvent.click(screen.getByText("Design schema"));
    await screen.findByRole("complementary", { name: "Задача Design schema" });
    expect(screen.getByRole("button", { name: "Запустить" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Завершить" })).toBeNull();
  });

  it("names board columns with the right case after numerals", async () => {
    useTasksUi.setState({ view: "board" });
    render(<TasksPanel projectId={PROJECT} />);
    await screen.findByText("Design schema");
    expect(screen.getByLabelText("Готовы к работе, 1 задача")).toBeTruthy();
    expect(screen.getByLabelText("Бэклог, 1 задача")).toBeTruthy();
    expect(screen.getByLabelText("В работе, 0 задач")).toBeTruthy();
  });
});
