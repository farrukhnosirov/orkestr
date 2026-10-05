import type { Event, PmDraft, PmEpic, TeamView } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { useTasksUi } from "../store";
import { allTasks, applyPmEvent, counts, edit, hasCycle, newFeature, nextKey, progressPercent, validateEpic } from "./pmModel";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const TEAM = "0190f0b0-0000-7000-8000-00000000aaaa";

function epic(): PmEpic {
  return {
    title: "Payroll: first release",
    description: "Deliver payroll.",
    taskId: null,
    features: [
      {
        key: "f1",
        title: "Payroll core",
        description: "",
        taskId: null,
        stories: [
          {
            key: "s1",
            title: "As a user I can store payroll records",
            description: "",
            acceptanceCriteria: ["A record can be created"],
            taskId: null,
            tasks: [
              { key: "t1", title: "Design the payroll data model", description: "", role: "Architect", priority: "High", dependsOn: [], estimateMinutes: 30, taskId: null },
              { key: "t2", title: "Implement the payroll service", description: "", role: "Backend", priority: "High", dependsOn: ["t1"], estimateMinutes: null, taskId: null },
            ],
          },
        ],
      },
    ],
  };
}

function draft(patch: Partial<PmDraft> = {}): PmDraft {
  return {
    id: "d1",
    projectId: PROJECT,
    idea: "a payroll module",
    status: "Draft",
    epic: epic(),
    warnings: [],
    model: null,
    planId: null,
    runId: null,
    teamId: null,
    progress: null,
    createdAt: "2026-10-05T10:00:00Z",
    updatedAt: "2026-10-05T10:00:00Z",
    ...patch,
  };
}

describe("product model", () => {
  it("edits the breakdown immutably with fresh keys", () => {
    const e = epic();
    expect(nextKey(e, "t")).toBe("t3");
    const added = edit.addFeature(e);
    expect(added.features).toHaveLength(2);
    expect(e.features).toHaveLength(1);
    const f2 = added.features[1];
    expect(f2?.key).toBe("f2");
    expect(f2?.stories[0]?.key).toBe("s2");
    expect(f2?.stories[0]?.tasks[0]?.key).toBe("t3");
    expect(newFeature(added).stories[0]?.tasks[0]?.key).toBe("t4");
    expect(counts(added)).toEqual({ features: 2, stories: 2, tasks: 3 });
    // Removing a task drops dependencies on it.
    const removed = edit.removeTask(e, 0, 0, 0);
    expect(allTasks(removed).map((t) => [t.key, t.dependsOn])).toEqual([["t2", []]]);
    const toggled = edit.toggleDependency(e, "t1", "t2");
    expect(allTasks(toggled)[0]?.dependsOn).toEqual(["t2"]);
    expect(edit.toggleDependency(e, "t1", "t1")).toEqual(e);
  });

  it("validates like the core", () => {
    expect(validateEpic(epic())).toBeNull();
    expect(validateEpic(edit.epic(epic(), { title: " " }))).toMatch(/title/);
    expect(validateEpic(edit.toggleDependency(epic(), "t1", "t2"))).toMatch(/cycle/);
    expect(validateEpic(edit.criterion(epic(), 0, 0, 0, ""))).toMatch(/criteria/);
    expect(validateEpic(edit.task(epic(), 0, 0, 1, { dependsOn: ["zz"] }))).toMatch(/unknown/);
    expect(validateEpic(edit.addFeature(epic()))).toMatch(/title/);
    expect(hasCycle(new Map([["a", ["b"]], ["b", []]]))).toBe(false);
  });

  it("tracks progress and applies events", () => {
    expect(progressPercent(draft({ status: "Approved", progress: { total: 4, completed: 1, failed: 0 } }))).toBe(25);
    const list = [draft({ progress: { total: 4, completed: 2, failed: 0 } })];
    const ev = { kind: "PmDraftUpdated", payload: { draft: draft({ status: "Approved" }) } } as Event;
    const next = applyPmEvent(list, ev);
    expect(next[0]?.status).toBe("Approved");
    expect(next[0]?.progress?.completed).toBe(2);
    expect(applyPmEvent(list, { kind: "TaskCreated", payload: {} } as unknown as Event)).toBe(list);
  });
});

const TEAM_VIEW = {
  team: { id: TEAM, name: "Web Team", kind: "Web", description: "", members: [], projectId: null, createdAt: "", policies: {}, builtin: true, updatedAt: null },
  assignments: [],
  leadRuns: [],
} as unknown as TeamView;

const handlers = new Set<(ev: Event) => void>();
const emit = (e: Event) => handlers.forEach((h) => h(e));
const calls: Array<{ method: string; params: unknown }> = [];
const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "pm.list":
        return { drafts: [draft()] };
      case "teams.list":
        return { teams: [TEAM_VIEW] };
      case "pm.update":
        return draft({ epic: (params as { epic: PmEpic }).epic, updatedAt: "2026-10-05T10:01:00Z" });
      case "pm.approve":
        return { draft: draft({ status: "Approved" }), plan: {}, run: { id: "r1" } };
      case "pm.draft":
        return draft({ id: "d2", idea: (params as { idea: string }).idea });
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.add(h);
    return () => {
      handlers.delete(h);
    };
  }),
};

vi.mock("../../../core/client", () => ({ maybeClient: () => fakeClient, getClient: () => fakeClient }));

const { ProductView } = await import("./ProductView");

describe("ProductView", () => {
  beforeEach(() => {
    calls.length = 0;
    useAppStore.getState().setConnection({ state: "open" });
    useTasksUi.setState({ toasts: [] });
  });
  afterEach(() => cleanup());

  it("drafts an idea, edits the tree, saves and approves with a team", async () => {
    render(<ProductView projectId={PROJECT} tasks={[]} />);
    const title = await screen.findByDisplayValue("Payroll: first release");
    expect(screen.getByText("1 feature · 1 story · 2 tasks")).toBeTruthy();
    expect(screen.getByRole("tree", { name: "Epic, features, stories and tasks" })).toBeTruthy();

    // Edit → Save sends the edited epic.
    fireEvent.change(title, { target: { value: "Payroll v1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "pm.update")).toBe(true));
    expect((calls.find((c) => c.method === "pm.update")?.params as { epic: PmEpic }).epic.title).toBe("Payroll v1");

    // A cycle disables approval with a visible reason.
    const deps = await screen.findAllByRole("button", { name: "t2" });
    fireEvent.click(deps[0] as HTMLElement);
    expect((await screen.findByRole("alert")).textContent).toMatch(/cycle/);
    fireEvent.click(screen.getByRole("button", { name: "t2" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());

    // Approve with the Web Team and a run.
    fireEvent.change(screen.getByLabelText("Agent team"), { target: { value: TEAM } });
    fireEvent.click(screen.getByRole("button", { name: "Approve and create 2 tasks" }));
    await waitFor(() => expect(calls.some((c) => c.method === "pm.approve")).toBe(true));
    expect(calls.find((c) => c.method === "pm.approve")?.params).toEqual({ draftId: "d1", startRun: true, teamId: TEAM });

    // A new idea.
    fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "a todo app" } });
    fireEvent.click(screen.getByRole("button", { name: "Break down" }));
    await waitFor(() => expect(calls.some((c) => c.method === "pm.draft")).toBe(true));
    expect((calls.find((c) => c.method === "pm.draft")?.params as { idea: string }).idea).toBe("a todo app");
    act(() => emit({ kind: "PmDraftUpdated", payload: { draft: draft({ id: "d3", epic: { ...epic(), title: "Todo app" } }) } } as Event));
    await screen.findByText("Todo app");
  });
});

describe("ProductView in Russian", () => {
  beforeEach(async () => {
    useAppStore.getState().setConnection({ state: "open" });
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

  it("renders the tree with Russian labels and plurals", async () => {
    render(<ProductView projectId={PROJECT} tasks={[]} />);
    await screen.findByDisplayValue("Payroll: first release");
    expect(screen.getByText("1 функция · 1 история · 2 задачи")).toBeTruthy();
    expect(screen.getByText("Эпик")).toBeTruthy();
    expect(screen.getByText("Критерии приёмки")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Подтвердить и создать 2 задачи" })).toBeTruthy();
  });
});
