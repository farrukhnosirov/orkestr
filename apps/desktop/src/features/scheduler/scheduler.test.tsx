import type { Event, SchedEntry, SchedulerExplanation, SchedulerSettings, SchedulerStatus } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { fromForm, reasonText, toForm, topReason } from "./model";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const OTHER = "0190f0b0-0000-7000-8000-eeeeeeeeeeee";
const PROVIDER = "0190f0b0-0000-7000-8000-aaaaaaaaaaaa";

const SETTINGS: SchedulerSettings = {
  maxParallelGlobal: 8,
  maxParallelPerProject: 4,
  projectCaps: [],
  teamCaps: [],
  providerCaps: [],
  modelCaps: [],
  maxCpuPercent: 85,
  minFreeRamMb: 2048,
  minFreeDiskMb: 1024,
  maxGpuPercent: null,
  agingMinutes: 20,
  fairShare: true,
  rateLimitBackoffSecs: 30,
  autoResume: "followApproval",
  maxAutoResumeAttempts: 3,
  mergeRiskChecks: true,
};

function entry(id: string, title: string, patch: Partial<SchedEntry> = {}): SchedEntry {
  return {
    taskId: id,
    projectId: PROJECT,
    runId: "0190f0b0-0000-7000-8000-000000000099",
    title,
    state: "waiting",
    status: "Ready",
    priority: "Normal",
    effectivePriority: "Normal",
    since: "2026-10-05T10:00:00Z",
    providerId: PROVIDER,
    model: `${PROVIDER}/mock-smart`,
    team: null,
    agentId: null,
    reason: { kind: "providerConcurrency", providerId: PROVIDER, running: 2, limit: 2 },
    reasonText: "waiting: provider concurrency 2/2",
    position: 1,
    ...patch,
  };
}

const RUNNING = entry("0190f0b0-0000-7000-8000-000000000001", "Build API", {
  state: "running",
  status: "Running",
  reason: null,
  reasonText: null,
  position: null,
});
const WAITING = entry("0190f0b0-0000-7000-8000-000000000002", "Write docs", { priority: "Low", effectivePriority: "Normal" });
const ELSEWHERE = entry("0190f0b0-0000-7000-8000-000000000003", "Other project task", {
  projectId: OTHER,
  state: "queued",
  reason: null,
  reasonText: null,
  position: 2,
});

function status(patch: Partial<SchedulerStatus> = {}): SchedulerStatus {
  return {
    settings: SETTINGS,
    counts: { running: 3, queued: 1, waiting: 1 },
    running: [RUNNING],
    queue: [WAITING],
    providers: [
      {
        providerId: PROVIDER,
        name: "Mock",
        runningAgents: 2,
        agentLimit: 2,
        inFlight: 1,
        queuedRequests: 0,
        rateLimitedUntil: null,
        rateLimitHits: 0,
      },
    ],
    projects: [{ projectId: PROJECT, running: 1, queued: 1, limit: 4 }],
    resources: {
      sampled: true,
      cpuPercent: 40,
      logicalCores: 8,
      ramAvailableMb: 4096,
      ramTotalMb: 16384,
      diskFreeMb: 50_000,
      gpuPercent: null,
    },
    mergeRisks: [],
    updatedAt: "2026-10-05T10:05:00Z",
    ...patch,
  };
}

const EXPLANATION: SchedulerExplanation = {
  taskId: WAITING.taskId,
  status: "Ready",
  entry: WAITING,
  scheduled: true,
  checks: [
    { check: "global", ok: true, reason: null, reasonText: null },
    {
      check: "provider",
      ok: false,
      reason: { kind: "providerConcurrency", providerId: PROVIDER, running: 2, limit: 2 },
      reasonText: "waiting: provider concurrency 2/2",
    },
  ],
  summary: "waiting: provider concurrency 2/2",
};

let current: SchedulerStatus = status();
const handlers: Array<(ev: Event) => void> = [];
const calls: Array<{ method: string; params: unknown }> = [];

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "scheduler.status":
        return current;
      case "scheduler.setLimits":
        return { settings: (params as { settings: SchedulerSettings }).settings };
      case "scheduler.explain":
        return EXPLANATION;
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.push(h);
    return () => {
      const i = handlers.indexOf(h);
      if (i >= 0) handlers.splice(i, 1);
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { SchedulerBar } = await import("./SchedulerBar");
const { QueueView } = await import("./QueueView");

describe("scheduler model", () => {
  it("localizes wait reasons in both languages", async () => {
    const s = status();
    expect(reasonText({ kind: "providerConcurrency", providerId: PROVIDER, running: 2, limit: 2 }, s)).toBe(
      "provider concurrency 2/2 (Mock)",
    );
    expect(reasonText({ kind: "projectLimit", running: 4, limit: 4 })).toBe("project limit 4/4");
    expect(reasonText({ kind: "fileConflict", withTask: RUNNING.taskId, path: "src/a.ts" })).toBe(
      "file src/a.ts is being edited by another agent",
    );
    await applyLanguagePreference("ru");
    try {
      expect(reasonText({ kind: "providerConcurrency", providerId: PROVIDER, running: 2, limit: 2 }, s)).toBe(
        "параллельные запросы провайдера 2/2 (Mock)",
      );
      expect(reasonText({ kind: "paused" })).toBe("запуск приостановлен");
    } finally {
      await applyLanguagePreference("en");
    }
  });

  it("picks the most common reason for the bar", () => {
    const a = entry("1", "a", { reason: { kind: "globalLimit", running: 8, limit: 8 } });
    const b = entry("2", "b", { reason: { kind: "globalLimit", running: 8, limit: 8 } });
    const c = entry("3", "c");
    expect(topReason([a, c, b])?.kind).toBe("globalLimit");
    expect(topReason([])).toBeNull();
  });

  it("round-trips the limits form and rejects out-of-range values", () => {
    const form = toForm(SETTINGS, PROJECT);
    expect(fromForm(SETTINGS, form, PROJECT)).toEqual(SETTINGS);
    const edited = fromForm(
      SETTINGS,
      { ...form, maxParallelGlobal: "12", projectCap: "2", maxGpuPercent: "70", providerCaps: { [PROVIDER]: "3" } },
      PROJECT,
    );
    expect(edited?.maxParallelGlobal).toBe(12);
    expect(edited?.projectCaps).toEqual([{ projectId: PROJECT, max: 2 }]);
    expect(edited?.maxGpuPercent).toBe(70);
    expect(edited?.providerCaps).toEqual([{ providerId: PROVIDER, max: 3 }]);
    expect(fromForm(SETTINGS, { ...form, maxParallelGlobal: "0" }, PROJECT)).toBeNull();
    expect(fromForm(SETTINGS, { ...form, maxCpuPercent: "101" }, PROJECT)).toBeNull();
    expect(fromForm(SETTINGS, { ...form, rateLimitBackoffSecs: "" }, PROJECT)).toBeNull();
  });
});

describe("SchedulerBar", () => {
  beforeEach(() => {
    calls.length = 0;
    current = status();
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("shows counts, the main wait reason and capacity, and refreshes on SchedulerUpdated", async () => {
    render(<SchedulerBar projectId={PROJECT} />);
    await screen.findByText("1 running");
    expect(screen.getByText("1 waiting")).toBeTruthy();
    expect(screen.getByText("Waiting: provider concurrency 2/2 (Mock)")).toBeTruthy();
    expect(screen.getByText("Global 3/8")).toBeTruthy();
    expect(screen.getByText("Project 1/4")).toBeTruthy();
    expect(calls[0]).toEqual({ method: "scheduler.status", params: { projectId: PROJECT } });

    current = status({
      queue: [],
      running: [RUNNING],
      providers: [{ ...status().providers[0]!, rateLimitedUntil: "2999-01-01T00:00:00Z", rateLimitHits: 1 }],
      mergeRisks: [
        {
          projectId: PROJECT,
          taskA: RUNNING.taskId,
          taskB: WAITING.taskId,
          branchA: "agent/a",
          branchB: "agent/b",
          files: ["src/api.ts"],
          conflicts: [],
          detectedAt: "2026-10-05T10:05:00Z",
        },
      ],
    });
    act(() => {
      for (const h of handlers) {
        h({
          seq: 9,
          id: "e",
          ts: "",
          projectId: null,
          taskId: null,
          agentId: null,
          nodeId: null,
          correlationId: null,
          kind: "SchedulerUpdated",
          payload: { counts: { running: 1, queued: 0, waiting: 0 }, settingsChanged: false },
        } as Event);
      }
    });
    await screen.findByText("1 merge risk");
    expect(screen.queryByText("1 waiting")).toBeNull();
    expect(screen.getByText(/^Mock: rate limited until/)).toBeTruthy();
  });

  it("edits limits through scheduler.setLimits", async () => {
    render(<SchedulerBar projectId={PROJECT} />);
    await screen.findByText("1 running");
    fireEvent.click(screen.getByRole("button", { name: "Limits" }));
    const dialog = await screen.findByRole("dialog", { name: "Parallel execution limits" });
    expect(dialog).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Max parallel agents (all projects)"), { target: { value: "5" } });
    fireEvent.change(screen.getByLabelText(/^Cap for this project/), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save limits" }));
    await waitFor(() => expect(calls.some((c) => c.method === "scheduler.setLimits")).toBe(true));
    const sent = calls.find((c) => c.method === "scheduler.setLimits")?.params as { settings: SchedulerSettings };
    expect(sent.settings.maxParallelGlobal).toBe(5);
    expect(sent.settings.projectCaps).toEqual([{ projectId: PROJECT, max: 2 }]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("rejects invalid limits without calling the core", async () => {
    render(<SchedulerBar projectId={PROJECT} />);
    await screen.findByText("1 running");
    fireEvent.click(screen.getByRole("button", { name: "Limits" }));
    await screen.findByRole("dialog");
    fireEvent.change(screen.getByLabelText("Max parallel agents (all projects)"), { target: { value: "0" } });
    fireEvent.click(screen.getByRole("button", { name: "Save limits" }));
    await screen.findByRole("alert");
    expect(calls.some((c) => c.method === "scheduler.setLimits")).toBe(false);
  });
});

describe("QueueView", () => {
  beforeEach(() => {
    calls.length = 0;
    current = status({ queue: [WAITING, ELSEWHERE] });
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("lists the queue across projects and explains a waiting task", async () => {
    render(<QueueView projectId={PROJECT} />);
    await screen.findByText("Write docs");
    expect(calls[0]).toEqual({ method: "scheduler.status", params: {} });
    expect(screen.getByText("Other project task")).toBeTruthy();
    expect(screen.getByText("another project")).toBeTruthy();
    expect(screen.getByText("#1")).toBeTruthy();
    expect(screen.getAllByText("Waiting: provider concurrency 2/2 (Mock)").length).toBeGreaterThan(0);
    const why = screen.getAllByRole("button", { name: "Why?" });
    fireEvent.click(why[0]!);
    await screen.findByText("Provider concurrency");
    expect(screen.getByText("Global limit")).toBeTruthy();
    expect(calls.some((c) => c.method === "scheduler.explain")).toBe(true);
  });

  it("shows the empty state", async () => {
    current = status({ running: [], queue: [] });
    render(<QueueView projectId={PROJECT} />);
    await screen.findByText("Nothing queued");
  });
});

describe("scheduler UI in Russian", () => {
  beforeEach(async () => {
    calls.length = 0;
    current = status();
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

  it("renders counts with Russian plurals and reasons", async () => {
    render(<SchedulerBar projectId={PROJECT} />);
    await screen.findByText("1 выполняется");
    expect(screen.getByText("1 ожидает")).toBeTruthy();
    expect(screen.getByText("Ожидает: параллельные запросы провайдера 2/2 (Mock)")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Лимиты" })).toBeTruthy();
  });
});
