/** Global search covers tasks, memory, the knowledge base and files — not only project names (§54). */
import type { Task } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
const TASK = { id: "t1", title: "Fix login redirect", description: "", tags: [], status: "Running" } as unknown as Task;

const fakeClient = {
  state: "open",
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "tasks.list":
        return { tasks: [TASK, { ...TASK, id: "t2", title: "Write docs" }] };
      case "memory.search":
        return {
          hits: [
            {
              item: { id: "m1", title: "Login uses OAuth", kind: "Note", scope: { scope: "Project", projectId: "p1" } },
              score: 1,
              snippet: "",
            },
          ],
        };
      case "knowledge.search":
        return { hits: [{ sourceId: "s1", sourceName: "Docs", path: "auth.md", headingPath: "Login", ordinal: 0, snippet: "", score: 1 }] };
      case "files.find":
        return { matches: [{ path: "src/auth/login.ts", score: 10, positions: [9, 10] }], filesScanned: 1, truncated: false };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
  openProject: vi.fn(),
}));

const { registerWorkspace } = await import("../../components/workspaceActions");
const { useAppStore } = await import("../../store/app");
const { useEditorStore } = await import("../../store/editor");
const { useTasksUi } = await import("../../panels/Tasks/store");
const { useMemoryFocus } = await import("../../panels/Memory/focus");
const { resetSearchCaches, searchAll, searchProviders } = await import("./providers");

describe("global search providers", () => {
  const openPanel = vi.fn();
  beforeEach(() => {
    calls.length = 0;
    resetSearchCaches();
    openPanel.mockReset();
    registerWorkspace({ openPanel, resetLayout: () => undefined, toggleMaximize: () => undefined });
    useAppStore.setState({ projects: [], activeProjectId: "p1" });
  });
  afterEach(() => registerWorkspace(null));

  it("registers more than the projects provider", () => {
    expect(searchProviders.map((p) => p.id)).toEqual(["projects", "tasks", "memory", "knowledge", "files"]);
  });

  it("finds tasks, memory, knowledge and files for one query", async () => {
    const groups = await searchAll("login", new AbortController().signal);
    const byId = Object.fromEntries(groups.map((g) => [g.provider.id, g.results.map((r) => r.title)]));
    expect(byId["tasks"]).toEqual(["Fix login redirect"]);
    expect(byId["memory"]).toEqual(["Login uses OAuth"]);
    expect(byId["knowledge"]).toEqual(["auth.md — Login"]);
    expect(byId["files"]).toEqual(["src/auth/login.ts"]);
    expect(calls.find((c) => c.method === "memory.search")?.params).toMatchObject({ query: "login", projectId: "p1", includeGlobal: true });
    expect(calls.find((c) => c.method === "files.find")?.params).toMatchObject({ projectId: "p1", query: "login" });
  });

  it("opens what was picked", async () => {
    const groups = await searchAll("login", new AbortController().signal);
    const pick = (id: string) => groups.find((g) => g.provider.id === id)?.results[0];
    await pick("tasks")?.run();
    expect(openPanel).toHaveBeenCalledWith("tasks");
    expect(useTasksUi.getState().selectedTaskId).toBe("t1");
    await pick("memory")?.run();
    expect(openPanel).toHaveBeenCalledWith("memory");
    expect(useMemoryFocus.getState().focus).toEqual({ tab: "project", query: "Login uses OAuth", expand: "m1" });
    await pick("files")?.run();
    expect(useEditorStore.getState().activePath).toBe("src/auth/login.ts");
  });

  it("does not hit the core for a one-character query, and caches the task list", async () => {
    await searchAll("l", new AbortController().signal);
    expect(calls.filter((c) => c.method !== "tasks.list")).toEqual([]);
    expect(calls).toEqual([]);
    await searchAll("log", new AbortController().signal);
    await searchAll("logi", new AbortController().signal);
    expect(calls.filter((c) => c.method === "tasks.list")).toHaveLength(1);
  });
});
