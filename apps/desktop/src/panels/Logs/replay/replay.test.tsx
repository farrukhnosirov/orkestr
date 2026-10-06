import type { Event, ReplayRunSummary, ReplayStep, ReplayStepResult, ReplayTimeline } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference, i18n } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { useEditorStore } from "../../../store/editor";
import { coreStepDetail, moveCursor, segments, statusTone, stepDelay, stepTitle } from "./model";

const PROJECT = "0192a000-0000-7000-8000-0000000000aa";
const RUN = "0192a000-0000-7000-8000-0000000000r1";
const TASK = "0192a000-0000-7000-8000-0000000000t1";
const AGENT = "0192a000-0000-7000-8000-0000000000a1";

function step(index: number, phase: ReplayStep["phase"], kind: string, title: string, extra: Partial<ReplayStep> = {}): ReplayStep {
  return {
    index,
    seq: index === 0 ? null : index,
    ts: new Date(Date.UTC(2026, 0, 1, 10, 0, index)).toISOString(),
    phase,
    kind,
    status: "Info",
    title,
    detail: null,
    agentId: null,
    taskId: null,
    correlationId: null,
    files: [],
    durationMs: null,
    taskStatus: null,
    agentState: null,
    toolStatus: null,
    approvalId: null,
    approvalStatus: null,
    verdict: null,
    testsPassed: null,
    testsFailed: null,
    runStatus: null,
    ...extra,
  };
}

const STEPS: ReplayStep[] = [
  step(0, "Request", "UserRequest", "Add login"),
  step(1, "Plan", "PlanApproved", "Add login", { status: "Success" }),
  step(2, "Agents", "AgentSpawned", "backend-1", { agentId: AGENT, taskId: TASK, agentState: "Idle" }),
  step(3, "Tools", "ToolCall", "fs.write", { agentId: AGENT, taskId: TASK, toolStatus: "Succeeded", status: "Success", durationMs: 40 }),
  step(4, "Files", "FileChanged", "src/login.rs", { agentId: AGENT, files: ["src/login.rs"], detail: "Created" }),
  step(5, "Run", "RunUpdated", "Add login", { runStatus: "Completed", status: "Success" }),
];

function timeline(steps: ReplayStep[] = STEPS): ReplayTimeline {
  return {
    projectId: PROJECT,
    subject: "Run",
    runId: RUN,
    planId: null,
    taskId: null,
    title: "Add login",
    runStatus: "Completed",
    taskStatus: null,
    startedAt: steps[0]?.ts ?? null,
    endedAt: steps[steps.length - 1]?.ts ?? null,
    steps,
    agents: [{ agentId: AGENT, name: "backend-1", role: null, taskId: TASK, steps: 3 }],
    tasks: [{ taskId: TASK, title: "Backend login", status: "Completed" }],
    phases: [],
    totalSteps: steps.length,
    eventsScanned: 20,
    truncated: false,
  };
}

const RUNS: ReplayRunSummary[] = [
  {
    runId: RUN,
    planId: "plan",
    goal: "Add login",
    status: "Completed",
    startedAt: "2026-01-01T10:00:00Z",
    finishedAt: "2026-01-01T10:05:00Z",
    tasks: [{ taskId: TASK, title: "Backend login", status: "Completed" }],
    agentsSpawned: 1,
    costUsd: 0,
  },
];

describe("replay model", () => {
  it("groups consecutive phases and moves the cursor within bounds", () => {
    const segs = segments([...STEPS, step(6, "Run", "RunUpdated", "x")]);
    expect(segs.map((s) => s.phase)).toEqual(["Request", "Plan", "Agents", "Tools", "Files", "Run"]);
    expect(segs[5]?.steps).toHaveLength(2);
    expect(moveCursor(0, 6, "prev")).toBe(0);
    expect(moveCursor(5, 6, "next")).toBe(5);
    expect(moveCursor(2, 6, "next")).toBe(3);
    expect(moveCursor(9, 6, "prev")).toBe(4);
    expect(moveCursor(3, 6, "last")).toBe(5);
    expect(moveCursor(3, 0, "last")).toBe(0);
    expect(stepDelay(2)).toBe(600);
    expect(stepDelay(0)).toBe(1200);
    expect(statusTone("Error")).toBe("danger");
  });

  it("localizes core-written titles and details", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    try {
      expect(stepTitle(step(0, "Agents", "AgentSpawned", "QA Engineer #1"))).toBe("Тестировщик #1");
      expect(stepTitle(step(0, "Agents", "AgentStopped", "QA Engineer #1"))).toBe("Тестировщик #1");
      expect(stepTitle(step(0, "Files", "SnapshotCreated", "Before QA Engineer #1 changes"))).toBe("Перед изменениями: Тестировщик #1");
      expect(stepTitle(step(0, "Files", "FileChanged", "QA Engineer #1"))).toBe("QA Engineer #1");
      expect(coreStepDetail(step(0, "Agents", "AgentStateChanged", "x", { detail: "Finished" }))).toBe("Работа завершена");
      expect(coreStepDetail(step(0, "Agents", "AgentStopped", "x", { detail: "stopped by the user" }))).toBe("остановлен пользователем");
      expect(coreStepDetail(step(0, "Agents", "AgentStopped", "x", { detail: "boom" }))).toBe("boom");
      expect(coreStepDetail(step(0, "Run", "TaskStatusChanged", "x", { detail: "agent_result_done" }))).toBe("Агент сообщил, что задача выполнена");
      expect(coreStepDetail(step(0, "Run", "TaskStatusChanged", "x", { detail: "agent_error: oops" }))).toBe("Ошибка агента: oops");
      expect(coreStepDetail(step(0, "Files", "FileChanged", "x", { detail: "Created" }))).toBeNull();
    } finally {
      await act(async () => {
        await applyLanguagePreference("en");
      });
    }
  });
});

// ── View ────────────────────────────────────────────────

const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
let runs: ReplayRunSummary[] = RUNS;

function stepResult(index: number): ReplayStepResult {
  return {
    step: STEPS[index] ?? null,
    index,
    total: STEPS.length,
    hasPrev: index > 0,
    hasNext: index < STEPS.length - 1,
    state: {
      tasks: [{ taskId: TASK, title: "Backend login", status: index >= 5 ? "Completed" : null }],
      agents: index >= 2 ? [{ agentId: AGENT, name: "backend-1", state: "Idle", active: true }] : [],
      filesChanged: index >= 4 ? ["src/login.rs"] : [],
      toolCalls: index >= 3 ? 1 : 0,
      toolFailures: 0,
      approvalsPending: 0,
      approvalsResolved: 0,
      testsPassed: 0,
      testsFailed: 0,
      reviews: 0,
      runStatus: index >= 5 ? "Completed" : null,
    },
  };
}

const fakeClient = {
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "events.replay":
        return { events: [], hasMore: false };
      case "replay.runs":
        return { runs };
      case "replay.timeline": {
        const agent = params.agentId as string | undefined;
        return { timeline: timeline(agent ? STEPS.filter((s) => s.agentId === agent) : STEPS) };
      }
      case "replay.step":
        return stepResult(Number(params.index ?? 0));
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], _h: (ev: Event) => void) => () => {}),
};

vi.mock("../../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { default: LogsPanel } = await import("../index");

describe("Replay tab", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    calls.length = 0;
    runs = RUNS;
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(async () => {
    vi.useRealTimers();
    cleanup();
    await act(async () => {
      await applyLanguagePreference("en");
    });
  });

  const openReplay = async () => {
    render(<LogsPanel projectId={PROJECT} />);
    fireEvent.click(screen.getByRole("tab", { name: "Replay" }));
    return screen.findByRole("list", { name: "Execution timeline" });
  };

  it("shows an empty state when there are no runs", async () => {
    runs = [];
    render(<LogsPanel projectId={PROJECT} />);
    fireEvent.click(screen.getByRole("tab", { name: "Replay" }));
    await screen.findByText("No runs to replay yet");
  });

  it("loads the newest run, steps through it and shows the state", async () => {
    const list = await openReplay();
    await within(list).findByText("fs.write");
    expect(calls.find((c) => c.method === "replay.timeline")?.params).toEqual({ projectId: PROJECT, runId: RUN });
    expect(within(list).getByText("User request")).toBeTruthy();
    expect(screen.getByText("Step 1 of 6")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Next step" }));
    expect(screen.getByText("Step 2 of 6")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Last step" }));
    expect(screen.getByText("Step 6 of 6")).toBeTruthy();
    await waitFor(() => expect(calls.some((c) => c.method === "replay.step" && c.params.index === 5)).toBe(true));
    const state = await screen.findByRole("region", { name: "State at this step" });
    expect(within(state).getByText("1 tool call")).toBeTruthy();

    // Clicking a step selects it; its file opens in Code.
    fireEvent.click(within(list).getByText("src/login.rs"));
    const detail = screen.getByRole("region", { name: "Step details" });
    expect(within(detail).getByText("created")).toBeTruthy();
    fireEvent.click(within(detail).getByRole("button", { name: "Open src/login.rs in Code" }));
    expect(useEditorStore.getState().activePath).toBe("src/login.rs");
  });

  it("filters by agent and phase", async () => {
    const list = await openReplay();
    await within(list).findByText("fs.write");
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: AGENT } });
    await waitFor(() => expect(calls.some((c) => c.method === "replay.timeline" && c.params.agentId === AGENT)).toBe(true));
    await waitFor(() => expect(within(list).queryByText("User request")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: /^Tools/ }));
    await waitFor(() =>
      expect(
        calls.some(
          (c) => c.method === "replay.timeline" && Array.isArray(c.params.phases) && !(c.params.phases as string[]).includes("Tools"),
        ),
      ).toBe(true),
    );
  });

  it("plays to the end", async () => {
    const list = await openReplay();
    await within(list).findByText("fs.write");
    fireEvent.change(screen.getByLabelText("Speed"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    await waitFor(() => expect(screen.getByText("Step 6 of 6")).toBeTruthy(), { timeout: 4000 });
    await waitFor(() => expect(screen.getByRole("button", { name: "Play" })).toBeTruthy());
  });

  it("renders in Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    render(<LogsPanel projectId={PROJECT} />);
    fireEvent.click(screen.getByRole("tab", { name: "Воспроизведение" }));
    const list = await screen.findByRole("list", { name: "Хронология выполнения" });
    await within(list).findByText("Вызов инструмента");
    expect(screen.getByText("Шаг 1 из 6")).toBeTruthy();
  });
});
