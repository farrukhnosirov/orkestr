import type { Event, HealingAttempt, PipelineStage, TaskPipeline } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { taskReason } from "../../lib/coreText";
import { useAppStore } from "../../store/app";
import { localizePipelineText } from "./text";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const TASK = "0190f0b0-0000-7000-8000-000000000001";
const P = "0190f0b0-0000-7000-8000-0000000000aa";

function stage(kind: PipelineStage["kind"], status: PipelineStage["status"], patch: Partial<PipelineStage> = {}): PipelineStage {
  return { kind, status, runs: status === "Pending" || status === "Skipped" ? 0 : 1, ...patch };
}

function attempt(patch: Partial<HealingAttempt> = {}): HealingAttempt {
  return {
    attempt: 1,
    analysis: {
      stage: "Test",
      summary: "The tests failed: `npm test` — 1 failed. 1 failing test(s).",
      signature: "abc",
      failingTests: [{ name: "greeter greets by name", message: "'Hello, X' !== 'Hello, X!'", file: "src/greeter.mjs", line: 5 }],
      compilerErrors: [{ severity: "Error", message: "type mismatch", file: "src/a.ts", line: 3, code: "TS2322" }],
      findings: [],
      excerpts: [{ file: "src/greeter.mjs", startLine: 1, endLine: 9, text: ">   5 | return `Hello, ${name}`;", reason: "failure line" }],
    },
    model: { providerId: P, model: "mock-smart" },
    escalated: false,
    outcome: "Fixed",
    startedAt: "2026-10-05T10:00:00Z",
    ...patch,
  };
}

function pipeline(patch: Partial<TaskPipeline> = {}): TaskPipeline {
  return {
    id: "p1",
    projectId: PROJECT,
    taskId: TASK,
    status: "Passed",
    stages: [
      stage("Author", "Passed"),
      stage("Review", "Passed", { runs: 2, detail: "Looks good" }),
      stage("Build", "Skipped", { detail: "no build / type check check is configured" }),
      stage("Test", "Passed", { runs: 2, detail: "1 passed" }),
      stage("Security", "Passed"),
      stage("Merge", "Waiting", { detail: "merge proposal ready" }),
    ],
    healing: [attempt()],
    limits: { maxAttempts: 3, maxWallMins: 60, stopOnRepeat: true, escalateModel: true },
    grantedAttempts: 0,
    reviewVerdict: "Approve",
    securityVerdict: "Approve",
    mergeOutcome: "proposed — awaiting approval",
    costAtStartUsd: 0,
    costUsd: 0,
    startedAt: "2026-10-05T10:00:00Z",
    updatedAt: "2026-10-05T10:01:00Z",
    ...patch,
  };
}

const handlers = new Set<(ev: Event) => void>();
const calls: Array<{ method: string; params: unknown }> = [];
let current: TaskPipeline | null = null;

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "pipeline.get":
        return { pipeline: current, history: [], live: current !== null && current.status === "Running" };
      case "pipeline.start":
        return pipeline({ status: "Running", healing: [] });
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.add(h);
    return () => handlers.delete(h);
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { PipelineSection, PipelineStages } = await import("./PipelineSection");

function emit(p: TaskPipeline) {
  const ev = {
    seq: 1,
    id: "e",
    ts: "2026-10-05T10:00:00Z",
    projectId: PROJECT,
    taskId: TASK,
    agentId: null,
    nodeId: null,
    correlationId: null,
    kind: "PipelineUpdated",
    payload: { pipeline: p },
  } as Event;
  act(() => handlers.forEach((h) => h(ev)));
}

describe("PipelineSection", () => {
  beforeEach(async () => {
    await applyLanguagePreference("en");
    calls.length = 0;
    handlers.clear();
    current = pipeline();
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("shows the stepper, verdicts, merge outcome and the healing timeline with its analysis", async () => {
    render(<PipelineSection taskId={TASK} taskStatus="Completed" onToast={vi.fn()} />);
    const stepper = await screen.findByRole("list", { name: "Pipeline stages" });
    const steps = within(stepper).getAllByRole("listitem");
    expect(steps).toHaveLength(6);
    expect(steps[3]?.textContent).toContain("Tests");
    expect(steps[3]?.textContent).toContain("2 runs");
    expect(steps[5]?.textContent).toContain("Waiting");
    expect(screen.getByText(/Review: approved · Security: approved · Merge: proposed/)).toBeTruthy();
    expect(screen.getByText("Attempt 1")).toBeTruthy();
    expect(screen.getByText("Fixed")).toBeTruthy();
    fireEvent.click(screen.getByText("To address"));
    expect(screen.getByText("greeter greets by name")).toBeTruthy();
    expect(screen.getByText("src/a.ts:3")).toBeTruthy();
    expect(document.querySelector(".pl-excerpt pre")?.textContent).toBe(">   5 | return `Hello, ${name}`;");
    expect(screen.queryByText("Run pipeline")).toBeNull();
  });

  it("follows live updates and escalations", async () => {
    current = null;
    render(<PipelineSection taskId={TASK} taskStatus="Testing" onToast={vi.fn()} />);
    await screen.findByText(/The pipeline runs when the task's agent finishes/);
    emit(
      pipeline({
        status: "Escalated",
        statusReason: "self-healing stopped: no progress: the last fix produced the same failure",
        healing: [attempt({ outcome: "StillFailing" })],
        grantedAttempts: 2,
      }),
    );
    await screen.findByText("Escalated to you");
    expect(screen.getByText(/no progress/)).toBeTruthy();
    expect(screen.getByText(/\+2 attempts granted by you/)).toBeTruthy();
    expect(screen.getByText("Still failing")).toBeTruthy();
  });

  it("starts the pipeline for a finished task", async () => {
    current = null;
    const toast = vi.fn();
    render(<PipelineSection taskId={TASK} taskStatus="Review" onToast={toast} />);
    fireEvent.click(await screen.findByText("Run pipeline"));
    await waitFor(() => expect(calls.find((c) => c.method === "pipeline.start")?.params).toEqual({ taskId: TASK }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith("info", "Quality pipeline started"));
  });

  it("speaks Russian", async () => {
    await applyLanguagePreference("ru");
    render(<PipelineSection taskId={TASK} taskStatus="Completed" onToast={vi.fn()} />);
    await screen.findByRole("list", { name: "Этапы конвейера" });
    expect(screen.getByText("Самовосстановление")).toBeTruthy();
    expect(screen.getByText("Попытка 1")).toBeTruthy();
    expect(screen.getByText("Исправлено")).toBeTruthy();
  });
});

describe("PipelineStages (run card)", () => {
  afterEach(() => cleanup());

  it("renders one chip per stage, the healing count and the status", async () => {
    await applyLanguagePreference("en");
    render(
      <PipelineStages
        task="Greeter"
        status="Healing"
        healing={2}
        stages={[
          { kind: "Author", status: "Passed" },
          { kind: "Review", status: "Passed" },
          { kind: "Test", status: "Failed" },
        ]}
      />,
    );
    const list = screen.getByRole("list", { name: "Pipeline of “Greeter”" });
    expect(within(list).getByText("Tests").closest("li")?.className).toContain("pl-chip--failed");
    expect(within(list).getByText("Self-healing")).toBeTruthy();
    expect(within(list).getByText("2")).toBeTruthy();
  });
});

describe("localizePipelineText", () => {
  afterEach(async () => {
    await applyLanguagePreference("en");
  });

  it("keeps English machine texts readable and leaves free text alone", () => {
    expect(localizePipelineText("1 check(s) passed")).toBe("1 check passed");
    expect(localizePipelineText("merged `agent/backend-1` into `main` (proposal 01a1)")).toBe("agent/backend-1 merged into main");
    expect(localizePipelineText("Looks good: 4 file(s) changed. — Mock (offline demo)")).toBe("Looks good: 4 file(s) changed. — Mock (offline demo)");
  });

  it("translates the machine-written forms to Russian", async () => {
    await applyLanguagePreference("ru");
    expect(localizePipelineText("3 check(s) passed")).toBe("пройдено 3 проверки");
    expect(localizePipelineText("5 passed")).toBe("пройдено 5 тестов");
    expect(localizePipelineText("1 finding(s), none blocking")).toBe("1 замечание, блокирующих нет");
    expect(localizePipelineText("proposed — awaiting approval")).toBe("предложено, ждёт подтверждения");
    expect(localizePipelineText("merge proposal ready (ReviewFirst) — waiting for your approval in the Merge Manager")).toBe(
      "предложение слияния готово (Сначала ревью) — подтвердите его в Менеджере слияний",
    );
    expect(
      localizePipelineText("The tests failed: `npm run test` — 2 passed, 1 failed, 0 skipped. 1 failing test(s)."),
    ).toBe("Тесты не прошли: `npm run test` — пройдено 2, не пройдено 1, пропущено 0. Непройденных тестов: 1.");
    expect(
      localizePipelineText("self-healing stopped (no progress: the last fix produced the same failure); the user declined further attempts"),
    ).toBe("самовосстановление остановлено (нет прогресса: последнее исправление дало тот же сбой); пользователь отказался от новых попыток");
  });

  it("translates every escalation and its reasons (status line, task history)", async () => {
    await applyLanguagePreference("ru");
    // The most common Escalated status reason.
    expect(localizePipelineText("self-healing stopped: 3 of 3 fix attempt(s) used")).toBe(
      "самовосстановление остановлено: использовано попыток исправления: 3 из 3",
    );
    expect(localizePipelineText("self-healing stopped: the pipeline's time limit of 60 min is reached")).toBe(
      "самовосстановление остановлено: исчерпан лимит времени конвейера качества (60 мин)",
    );
    expect(localizePipelineText("self-healing stopped: self-healing is disabled for this project")).toBe(
      "самовосстановление остановлено: самовосстановление отключено для этого проекта",
    );
    const budget = localizePipelineText("self-healing stopped (the task's budget is spent ($0.2000 of $0.20)); raise the budget and retry the task to continue");
    expect(budget).toMatch(/^самовосстановление остановлено \(бюджет задачи исчерпан \(0,20\s\$ из 0,20\s\$\)\); увеличьте бюджет/);
    expect(localizePipelineText("more self-healing time granted by the user")).toBe("пользователь выделил самовосстановлению больше времени");
    expect(localizePipelineText("more self-healing budget granted by the user")).toBe("пользователь увеличил бюджет самовосстановления");
    expect(localizePipelineText("waiting for your decision")).toBe("ждёт вашего решения");
    expect(localizePipelineText("superseded by a new attempt of the task")).toBe("заменено новой попыткой задачи");
    expect(localizePipelineText("stopped with the run")).toBe("остановлено вместе с запуском");
    expect(localizePipelineText("self-healing stopped (2 of 2 fix attempt(s) used) and the escalation could not be filed: db locked")).toBe(
      "самовосстановление остановлено (использовано попыток исправления: 2 из 2), и эскалацию создать не удалось: db locked",
    );
    // Task history (coreText.taskReason) uses the same reason localizer.
    expect(taskReason("self-healing stopped (the task's budget is spent ($0.2000 of $0.20)); the user declined further attempts")).toMatch(
      /^Самовосстановление остановлено \(бюджет задачи исчерпан \(0,20\s\$ из 0,20\s\$\)\); вы отказались от новых попыток$/,
    );
    expect(taskReason("self-healing stopped (3 of 3 fix attempt(s) used); raise the budget and retry the task to continue")).toBe(
      "Самовосстановление остановлено (использовано попыток исправления: 3 из 3); увеличьте бюджет и повторите задачу, чтобы продолжить",
    );
  });
});
