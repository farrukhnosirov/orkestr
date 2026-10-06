import type { Event, ManagerChatMessage, ManagerConfig, ManagerPlan, ManagerRun, PlanTask } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import {
  EMPTY_CHAT,
  EMPTY_EXPERT,
  applyManagerEvent,
  attachmentOf,
  buildOptions,
  chatItems,
  composerKeyAction,
  draftToEdits,
  latestProposedPlan,
  pinnedRun,
  planLevels,
  planTree,
  runProgress,
  upsertMessage,
  validateDraft,
} from "./model";
import { RunReport } from "./RunReport";
import { useManagerUi } from "./store";
import { visibleRange } from "./VirtualList";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const CONV = "0190f0b0-0000-7000-8000-0000000000c0";
const PLAN = "0190f0b0-0000-7000-8000-0000000000p1";
const RUN = "0190f0b0-0000-7000-8000-0000000000r1";
const PROVIDER = "0190f0b0-0000-7000-8000-0000000000aa";

let clock = 0;

function msg(id: string, role: ManagerChatMessage["role"], text: string, patch: Partial<ManagerChatMessage> = {}): ManagerChatMessage {
  return {
    id,
    projectId: PROJECT,
    conversationId: CONV,
    role,
    text,
    toolEvents: [],
    planId: null,
    runId: null,
    model: null,
    usage: null,
    createdAt: new Date(Date.UTC(2026, 9, 2, 10, 0, clock++)).toISOString(),
    ...patch,
  };
}

function ptask(key: string, title: string, deps: string[], patch: Partial<PlanTask> = {}): PlanTask {
  return {
    key,
    title,
    description: "",
    acceptanceCriteria: [`${title} works`],
    role: "Backend",
    dependsOn: deps,
    priority: "Normal",
    epic: "Hello module",
    feature: null,
    files: [],
    estimatedTokens: null,
    estimatedCostUsd: null,
    taskId: null,
    ...patch,
  };
}

function plan(patch: Partial<ManagerPlan> = {}): ManagerPlan {
  return {
    id: PLAN,
    projectId: PROJECT,
    conversationId: CONV,
    goal: "Create a hello module with tests",
    summary: "Design, implement, document, test.",
    assumptions: [],
    risks: [],
    tasks: [
      ptask("t1", "Design the hello module API", [], { role: "Architect", feature: "Design" }),
      ptask("t2", "Implement the hello module", ["t1"], { feature: "Implementation" }),
      ptask("t3", "Document the hello module", ["t1"], { role: "ProductManager", feature: "Documentation" }),
      ptask("t4", "Test the hello module", ["t2"], { role: "Qa", feature: "Quality" }),
    ],
    estimatedCostUsd: 0.12,
    maxParallelism: 2,
    parallelism: 2,
    status: "Proposed",
    source: "Structured",
    model: null,
    warnings: [],
    options: {},
    runId: null,
    rejectionReason: null,
    createdAt: "2026-10-02T10:00:01Z",
    updatedAt: "2026-10-02T10:00:01Z",
    ...patch,
  };
}

function run(patch: Partial<ManagerRun> = {}): ManagerRun {
  const base = { branch: null, worktreePath: null, costUsd: 0, filesChanged: [], lastError: null, awaitingReview: false, summary: null, attempts: 1 };
  return {
    id: RUN,
    projectId: PROJECT,
    planId: PLAN,
    goal: "Create a hello module with tests",
    status: "Running",
    statusReason: null,
    limits: { maxParallelAgents: 2, maxAgents: 20, maxCostUsd: 1, timeLimitMins: null, maxConsecutiveFailures: 3, maxAttemptsPerTask: 2 },
    approvalPolicy: "SemiAuto",
    tasks: [
      { ...base, taskId: "a", key: "t1", title: "Design", role: "Architect", status: "Completed", agentId: "ag1" },
      { ...base, taskId: "b", key: "t2", title: "Implement", role: "Backend", status: "Running", agentId: "ag2", branch: "agent/backend-1" },
      { ...base, taskId: "c", key: "t3", title: "Document", role: "ProductManager", status: "Review", agentId: "ag3", awaitingReview: true },
      { ...base, taskId: "d", key: "t4", title: "Test", role: "Qa", status: "Blocked", agentId: null },
    ],
    agentsSpawned: 3,
    peakParallel: 2,
    consecutiveFailures: 0,
    costUsd: 0.04,
    activeSecs: 65,
    startedAt: "2026-10-02T10:01:00Z",
    updatedAt: "2026-10-02T10:02:00Z",
    finishedAt: null,
    report: null,
    ...patch,
  };
}

function ev(kind: string, payload: unknown): Event {
  return { seq: 1, id: "e", ts: "", projectId: PROJECT, taskId: null, agentId: null, nodeId: null, correlationId: null, kind, payload } as Event;
}

const CONFIG: ManagerConfig = {
  model: { providerId: PROVIDER, model: "mock-smart" },
  reasoning: "Medium",
  systemInstructions: "",
  maxParallelAgents: 2,
  budgetUsd: null,
  timeLimitMins: null,
  contextPolicy: "Balanced",
  fallbackChain: [],
  approvalPolicy: "SemiAuto",
};

// ── pure logic ───────────────────────────────────────────

describe("manager model", () => {
  it("streams deltas and replaces them with the stored reply", () => {
    let s = applyManagerEvent(EMPTY_CHAT, ev("ManagerMessageAdded", { message: msg("u1", "User", "hi"), busy: true }));
    s = applyManagerEvent(s, ev("ManagerMessage", { agentId: null, messageId: "r1", role: "Assistant", delta: "Hel", done: false }));
    s = applyManagerEvent(s, ev("ManagerMessage", { agentId: null, messageId: "r1", role: "Assistant", delta: "lo", done: false }));
    expect(s.streaming).toEqual([{ id: "r1", text: "Hello" }]);
    expect(s.busy).toBe(true);
    expect(chatItems(s).map((i) => i.kind)).toEqual(["message", "streaming"]);
    s = applyManagerEvent(s, ev("ManagerMessageAdded", { message: msg("r1", "Manager", "Hello"), busy: false }));
    expect(s.streaming).toEqual([]);
    expect(s.messages.map((m) => m.id)).toEqual(["u1", "r1"]);
    expect(s.busy).toBe(false);
    // Late delta for a stored reply and agent deltas are ignored.
    expect(applyManagerEvent(s, ev("ManagerMessage", { agentId: null, messageId: "r1", role: "Assistant", delta: "x", done: false }))).toBe(s);
    expect(applyManagerEvent(s, ev("ManagerMessage", { agentId: "a", messageId: "z", role: "Assistant", delta: "x", done: false }))).toBe(s);
    // Idempotent upsert.
    expect(upsertMessage(s.messages, msg("r1", "Manager", "Hello!"))[1]?.text).toBe("Hello!");
  });

  it("tracks plans and runs, keeps the newest run state and pins the live run", () => {
    let s = applyManagerEvent(EMPTY_CHAT, ev("ManagerPlanUpdated", { plan: plan() }));
    expect(latestProposedPlan(s)?.id).toBe(PLAN);
    s = applyManagerEvent(s, ev("ManagerRunUpdated", { run: run() }));
    expect(pinnedRun(s)?.id).toBe(RUN);
    const stale = applyManagerEvent(s, ev("ManagerRunUpdated", { run: run({ status: "Paused", updatedAt: "2026-10-02T10:00:00Z" }) }));
    expect(stale.runs[RUN]?.status).toBe("Running");
    s = applyManagerEvent(s, ev("ManagerRunUpdated", { run: run({ status: "Completed", updatedAt: "2026-10-02T10:09:00Z" }) }));
    expect(pinnedRun(s)).toBeNull();
    // The first message of a run carries the run card.
    s = { ...s, messages: [msg("e1", "SystemEvent", "Run started", { runId: RUN }), msg("e22", "SystemEvent", "Run finished", { runId: RUN })] };
    expect(chatItems(s).map((i) => (i.kind === "message" ? i.showRun : null))).toEqual([true, false]);
  });

  it("builds a plan tree by dependency depth and validates edits", () => {
    const p = plan();
    expect([...planLevels(p.tasks).entries()]).toEqual([
      ["t1", 0],
      ["t2", 1],
      ["t3", 1],
      ["t4", 2],
    ]);
    const cyclic = [ptask("a", "A", ["b"]), ptask("b", "B", ["a"])];
    expect(planLevels(cyclic).size).toBe(2);
    expect(planTree(p.tasks).map((g) => g.feature)).toEqual(["Design", "Implementation", "Documentation", "Quality"]);
    expect(draftToEdits(p, { titles: {}, removed: [] })).toBeUndefined();
    expect(draftToEdits(p, { titles: { t2: "Implement hello()", t3: "Document the hello module" }, removed: ["t4"] })).toEqual({
      removeTasks: ["t4"],
      titles: [{ key: "t2", title: "Implement hello()" }],
    });
    expect(validateDraft(p, { titles: { t1: "  " }, removed: [] })).toMatch(/empty/);
    expect(validateDraft(p, { titles: {}, removed: ["t1", "t2", "t3", "t4"] })).toMatch(/at least one/);
  });

  it("computes progress, expert options, attachments and composer keys", () => {
    expect(runProgress(run())).toEqual({ total: 4, completed: 1, running: 1, failed: 0, review: 1, percent: 25 });
    expect(buildOptions(EMPTY_EXPERT, null)).toEqual({});
    expect(buildOptions({ budgetUsd: "2.5", maxParallelAgents: "3", timeLimitMins: "" }, { providerId: PROVIDER, model: " m " })).toEqual({
      options: { model: { providerId: PROVIDER, model: "m" }, budgetUsd: 2.5, maxParallelAgents: 3 },
    });
    expect(buildOptions({ ...EMPTY_EXPERT, maxParallelAgents: "0" }, null).error).toMatch(/1 to 64/);
    expect(buildOptions({ ...EMPTY_EXPERT, budgetUsd: "-1" }, null).error).toMatch(/non-negative/);
    expect(attachmentOf({ text: "fn a() {}", projectId: PROJECT, path: "src/a.rs", startLine: 3, endLine: 5, language: "rust" })).toEqual({
      path: "src/a.rs",
      text: "fn a() {}",
      startLine: 3,
      endLine: 5,
      language: "rust",
    });
    expect(attachmentOf({ text: "just text", projectId: null })).toBeNull();
    const k = { shiftKey: false, ctrlKey: false, metaKey: false };
    expect(composerKeyAction({ ...k, key: "Enter" })).toBe("send");
    expect(composerKeyAction({ ...k, key: "Enter", shiftKey: true })).toBe("newline");
    expect(composerKeyAction({ ...k, key: "Enter", ctrlKey: true })).toBe("send");
    expect(composerKeyAction({ ...k, key: "Enter", isComposing: true })).toBeNull();
    expect(composerKeyAction({ ...k, key: "a" })).toBeNull();
  });

  it("virtual list range covers the viewport plus overscan", () => {
    const heights = Array.from({ length: 100 }, () => 50);
    const offsets = heights.map((_, i) => i * 50);
    expect(visibleRange(offsets, heights, 0, 200, 0)).toEqual([0, 3]);
    expect(visibleRange(offsets, heights, 1000, 200, 100)).toEqual([18, 25]);
    expect(visibleRange(offsets, heights, 4900, 200, 0)).toEqual([98, 99]);
    expect(visibleRange([], [], 0, 100, 0)).toEqual([0, -1]);
  });
});

// ── panel ────────────────────────────────────────────────

let handlers: Array<(ev: Event) => void> = [];
const calls: { method: string; params: unknown }[] = [];
let history = {
  conversationId: CONV,
  messages: [msg("u1", "User", "Create a hello module with tests"), msg("m12", "Manager", "I drafted a **plan**.", { planId: PLAN })],
  summaries: [],
  busy: false,
  plans: [plan()],
  activeRun: null as ManagerRun | null,
  config: CONFIG,
};

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "manager.history":
        return history;
      case "manager.status":
        return { runs: history.activeRun ? [history.activeRun] : [], liveRunIds: [] };
      case "manager.resumeSummary":
        return {
          projectId: PROJECT,
          headline: "Last session: 1 of 4 tasks completed (25%).",
          lines: ["Run of “Create a hello module with tests” is Interrupted."],
          tasksTotal: 4,
          tasksCompleted: 1,
          tasksInProgress: 1,
          tasksFailed: 0,
          tasksBlocked: 2,
          tasksCancelled: 0,
          percentComplete: 25,
          goals: [],
          issues: [],
          pendingApprovals: 1,
          lastSnapshot: null,
          activeRun: run({ status: "Interrupted" }),
          lastActivityAt: null,
          isEmpty: false,
        };
      case "manager.chat":
        return { userMessage: msg("u2", "User", "x"), replyMessageId: "r2" };
      case "manager.approvePlan":
        return { plan: plan({ status: "Running" }), run: run() };
      case "manager.rejectPlan":
        return { plan: plan({ status: "Rejected" }) };
      case "manager.pause":
      case "manager.resume":
      case "manager.cancel":
        return { run: run() };
      case "providers.list":
        return { providers: [] };
      case "providers.kinds":
        return { kinds: [], pricingTableDate: "" };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.push(h);
    return () => {
      handlers = handlers.filter((x) => x !== h);
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { default: ManagerPanel } = await import("./index");
const { managerCommands } = await import("./commands");
const { askManager } = await import("../../features/files/actions");

const emit = (e: Event) => act(() => handlers.forEach((h) => h(e)));

describe("ManagerPanel", () => {
  beforeEach(() => {
    calls.length = 0;
    history = { ...history, busy: false, activeRun: null, plans: [plan()] };
    useAppStore.getState().setConnection({ state: "open" });
    useManagerUi.setState({ mode: "natural", prefill: null, focusNonce: 0, proposedPlan: null, dismissedResume: [], toasts: [] });
  });
  afterEach(() => cleanup());

  it("shows the empty state without a project", () => {
    render(<ManagerPanel projectId={null} />);
    expect(screen.getByText("No project open")).toBeTruthy();
  });

  it("renders history, the plan card and streams a reply", async () => {
    render(<ManagerPanel projectId={PROJECT} />);
    const card = await screen.findByRole("region", { name: "Plan: Create a hello module with tests" });
    expect(within(card).getByText("Implement the hello module")).toBeTruthy();
    expect(within(card).getAllByText(/after Design the hello module API/).length).toBe(2);
    expect(screen.getByText("plan")).toBeTruthy(); // markdown bold
    expect(screen.getByRole("button", { name: /Manager model: mock-smart/ })).toBeTruthy();
    await waitFor(() => expect(useManagerUi.getState().proposedPlan).toEqual({ projectId: PROJECT, planId: PLAN }));
    // Resume banner.
    expect(screen.getByRole("region", { name: "Last session" })).toBeTruthy();

    // Send with Enter; the reply streams in.
    const box = screen.getByLabelText("Message to the AI Manager");
    fireEvent.change(box, { target: { value: "What's the status?" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.some((c) => c.method === "manager.chat")).toBe(true));
    expect(calls.find((c) => c.method === "manager.chat")?.params).toEqual({
      projectId: PROJECT,
      text: "What's the status?",
      options: { language: "en" },
    });
    await screen.findByText(/The Manager is working/);
    emit(ev("ManagerMessage", { agentId: null, messageId: "r2", role: "Assistant", delta: "All good so far", done: false }));
    await screen.findByText("All good so far");
    emit(ev("ManagerMessageAdded", { message: msg("r2", "Manager", "All good so far."), busy: false }));
    await screen.findByText("All good so far.");
    expect(screen.queryByText(/The Manager is working/)).toBeNull();
  });

  it("edits and approves a plan, then shows the live run", async () => {
    render(<ManagerPanel projectId={PROJECT} />);
    const card = await screen.findByRole("region", { name: "Plan: Create a hello module with tests" });
    fireEvent.click(within(card).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(card).getByLabelText("Title of t2"), { target: { value: "Implement hello()" } });
    fireEvent.click(within(card).getByRole("button", { name: "Remove t3" }));
    fireEvent.click(within(card).getByRole("button", { name: /Approve edited plan/ }));
    await waitFor(() => expect(calls.some((c) => c.method === "manager.approvePlan")).toBe(true));
    expect(calls.find((c) => c.method === "manager.approvePlan")?.params).toEqual({
      planId: PLAN,
      edits: { removeTasks: ["t3"], titles: [{ key: "t2", title: "Implement hello()" }] },
    });
    emit(ev("ManagerPlanUpdated", { plan: plan({ status: "Running", runId: RUN }) }));
    emit(ev("ManagerRunUpdated", { run: run() }));
    const live = await screen.findByRole("region", { name: "Run: Create a hello module with tests" });
    expect(within(live).getByRole("progressbar").getAttribute("aria-valuenow")).toBe("25");
    fireEvent.click(within(live).getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(calls.some((c) => c.method === "manager.pause")).toBe(true));
    expect(calls.find((c) => c.method === "manager.pause")?.params).toEqual({ runId: RUN });
  });

  it("rejects a plan with a reason", async () => {
    render(<ManagerPanel projectId={PROJECT} />);
    const card = await screen.findByRole("region", { name: "Plan: Create a hello module with tests" });
    fireEvent.click(within(card).getByRole("button", { name: "Reject" }));
    fireEvent.change(within(card).getByLabelText("Rejection reason"), { target: { value: "Use TypeScript" } });
    fireEvent.click(within(card).getByRole("button", { name: "Reject plan" }));
    await waitFor(() => expect(calls.some((c) => c.method === "manager.rejectPlan")).toBe(true));
    expect(calls.find((c) => c.method === "manager.rejectPlan")?.params).toEqual({ planId: PLAN, reason: "Use TypeScript" });
  });

  it("prefills the composer from the Code panel and sends options in Expert mode", async () => {
    useManagerUi.setState({ mode: "expert" });
    askManager({ text: "fn a() {}", projectId: PROJECT, path: "src/a.rs", startLine: 3, endLine: 5, language: "rust" });
    render(<ManagerPanel projectId={PROJECT} />);
    await screen.findByText("src/a.rs:3-5");
    const box = screen.getByLabelText("Message to the AI Manager") as HTMLTextAreaElement;
    expect(box.value).toBe("Explain this code from src/a.rs");
    fireEvent.change(screen.getByPlaceholderText("Manager default"), { target: { value: "1.5" } });
    fireEvent.change(screen.getByPlaceholderText("default"), { target: { value: "99" } });
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await screen.findByText(/1 to 64/);
    expect(calls.some((c) => c.method === "manager.chat")).toBe(false);
    fireEvent.change(screen.getByPlaceholderText("default"), { target: { value: "3" } });
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(calls.some((c) => c.method === "manager.chat")).toBe(true));
    expect(calls.find((c) => c.method === "manager.chat")?.params).toEqual({
      projectId: PROJECT,
      text: "Explain this code from src/a.rs",
      attachment: { path: "src/a.rs", text: "fn a() {}", startLine: 3, endLine: 5, language: "rust" },
      options: { budgetUsd: 1.5, maxParallelAgents: 3, language: "en" },
    });
  });

  it("Approve Plan and Ask Manager commands", async () => {
    const approve = managerCommands.find((c) => c.id === "manager.approvePlan");
    const ask = managerCommands.find((c) => c.id === "manager.ask");
    expect(ask?.shortcut).toBe("Ctrl+Shift+M");
    expect(approve?.enabled?.({ projectId: PROJECT })).toBe(false);
    render(<ManagerPanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Plan: Create a hello module with tests" });
    expect(approve?.enabled?.({ projectId: PROJECT })).toBe(true);
    expect(approve?.enabled?.({ projectId: "other" })).toBe(false);
    await act(async () => {
      await approve?.run({ projectId: PROJECT });
    });
    expect(calls.find((c) => c.method === "manager.approvePlan")?.params).toEqual({ planId: PLAN });
    const before = useManagerUi.getState().focusNonce;
    void ask?.run({ projectId: PROJECT });
    expect(useManagerUi.getState().focusNonce).toBe(before + 1);
  });
});

describe("ManagerPanel in Russian", () => {
  beforeEach(async () => {
    calls.length = 0;
    history = { ...history, busy: false, activeRun: null, plans: [plan()] };
    useAppStore.getState().setConnection({ state: "open" });
    useManagerUi.setState({ mode: "natural", prefill: null, focusNonce: 0, proposedPlan: null, dismissedResume: [], toasts: [] });
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

  it("renders the plan card, resume banner and composer in Russian and asks for Russian replies", async () => {
    render(<ManagerPanel projectId={PROJECT} />);
    const card = await screen.findByRole("region", { name: "План: Create a hello module with tests" });
    expect(within(card).getByText("Предложен")).toBeTruthy();
    expect(within(card).getByText(/^4 задачи · до 2 параллельно/)).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Подтвердить" })).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Отклонить" })).toBeTruthy();
    expect(within(card).getAllByText("Бэкенд-разработчик").length).toBeGreaterThan(0);
    const banner = screen.getByRole("region", { name: "Прошлый сеанс" });
    expect(within(banner).getByText("1 запрос на подтверждение")).toBeTruthy();
    expect(within(banner).getByRole("button", { name: "Возобновить запуск" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Какой статус у проекта?" })).toBeTruthy();

    const box = screen.getByLabelText("Сообщение AI-менеджеру");
    fireEvent.change(box, { target: { value: "Какой статус?" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.some((c) => c.method === "manager.chat")).toBe(true));
    expect(calls.find((c) => c.method === "manager.chat")?.params).toEqual({
      projectId: PROJECT,
      text: "Какой статус?",
      options: { language: "ru" },
    });
    await screen.findByText(/Менеджер работает/);
  });

  it("validates plan edits and expert options in Russian", () => {
    expect(validateDraft(plan(), { titles: {}, removed: ["t1", "t2", "t3", "t4"] })).toBe("Оставьте хотя бы одну задачу.");
    expect(buildOptions({ ...EMPTY_EXPERT, maxParallelAgents: "0" }, null).error).toMatch(/от 1 до 64/);
    expect(runProgress(run()).percent).toBe(25);
  });
});

describe("RunReport", () => {
  const report = {
    summary: "**Run completed** — English Markdown written by the core",
    completed: 1,
    failed: 0,
    notDone: 0,
    filesChanged: ["src/payroll.mjs"],
    tests: ["Test — `npm test`: passed"],
    openIssues: [],
    costUsd: 0,
    durationSecs: 18,
    mergeProposals: [
      { taskId: "b", title: "Implement", branch: "agent/backend-1", filesChanged: ["src/payroll.mjs"], additions: 3, deletions: 1, note: "merged `agent/backend-1` into `main`" },
    ],
    pipelines: [
      {
        taskId: "b",
        title: "Implement",
        status: "Passed",
        stages: [],
        reviewVerdict: "Approve",
        securityVerdict: "Approve",
        gates: "Passed",
        failedGates: [],
        healingAttempts: 1,
        mergeOutcome: "proposed — awaiting approval",
      },
    ],
  } as unknown as NonNullable<ManagerRun["report"]>;
  const finished = () => run({ status: "Completed", tasks: run().tasks.slice(0, 1), report });

  afterEach(async () => {
    cleanup();
    await act(async () => {
      await applyLanguagePreference("en");
    });
  });

  it("is rendered from the structured fields, not the stored Markdown", () => {
    render(<RunReport run={finished()} report={report} />);
    expect(screen.queryByText(/English Markdown/)).toBeNull();
    expect(screen.getByText("What was done")).toBeTruthy();
    expect(screen.getByText(/self-healing: 1 attempt · merge: proposed, awaiting approval/)).toBeTruthy();
    expect(screen.getByText(/agent\/backend-1 merged into main/)).toBeTruthy();
  });

  it("follows the UI language", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    render(<RunReport run={finished()} report={report} />);
    expect(screen.getByText("Что сделано")).toBeTruthy();
    expect(screen.getByText("Конвейер качества")).toBeTruthy();
    expect(screen.getByText(/самовосстановление: 1 попытка · слияние: предложено, ждёт подтверждения/)).toBeTruthy();
    expect(screen.getByText(/1 файл, \+3 −1 · agent\/backend-1 слита в main/)).toBeTruthy();
  });
});
