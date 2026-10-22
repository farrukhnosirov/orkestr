import type { CommandMatch, ComputerAction, HandleResult, PlanProposal } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
let nextHandle: HandleResult | null = null;
let matchScore = 0.95;

const PLAN: PlanProposal = {
  planId: "plan-1",
  request: "write a shopping list",
  corrections: [],
  steps: [
    { type: "Launch", target: "notepad" },
    { type: "Wait", ms: 1500 },
    { type: "TypeText", text: "milk" },
  ],
  explanation: "Mock (offline demo): open Notepad and type “milk”.",
  model: "mock-smart",
  safety: "Safe",
  risks: [],
  createdAt: "2026-10-23T00:00:00Z",
};

const run = (title: string, outcome: "Succeeded" | "Failed" | "Rejected" = "Succeeded"): HandleResult => ({
  status: "Executed",
  run: {
    runId: "r1",
    title,
    outcome,
    steps: [{ index: 0, action: "Launch", ok: outcome !== "Failed", detail: outcome === "Failed" ? "not found" : "ok" }],
    effects: [],
    startedAt: "2026-10-23T00:00:00Z",
    durationMs: 3,
  },
});

const fake = {
  call: vi.fn((method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "computer.handle":
        return Promise.resolve(nextHandle ?? run(String(params["text"])));
      case "computer.match": {
        const m: CommandMatch = { setId: "apps", commandId: "app.notepad.open", title: "Open notepad", phrase: "open notepad", score: matchScore, slots: {}, safety: "Safe" };
        return Promise.resolve({ matches: [m], elapsedUs: 10 });
      }
      case "computer.run":
        return Promise.resolve(run("Open notepad"));
      case "computer.confirm":
        return Promise.resolve(run("Close notepad without saving", params["approve"] ? "Succeeded" : "Rejected"));
      case "computer.approvePlan":
        return Promise.resolve(run(PLAN.request));
      case "computer.plan":
        return Promise.resolve({ plan: { ...PLAN, corrections: [params["correction"]] } });
      case "computer.rememberPlan":
        return Promise.resolve({ setId: "my-commands", command: { id: "user.1", category: "custom", title: { en: "x", ru: "x" }, phrases: params["phrases"], steps: PLAN.steps, safety: "Safe" } });
      case "computer.history":
        return Promise.resolve({ entries: [{ at: "2026-10-23T00:00:00Z", text: "open notepad", title: "Open notepad", outcome: "Succeeded" }] });
      case "computer.status":
        return Promise.resolve({ available: true, backend: "windows", catalogueCommands: 1035, userCommands: 1, enabledSets: 14, running: 0, pending: 0, indexedApps: 120 });
      case "computer.stop":
        return Promise.resolve({ runsStopped: 1, pendingCancelled: 1, timersCancelled: 0 });
      default:
        return Promise.resolve({});
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

import { applyLanguagePreference } from "../../i18n";
import { applyResult, approvePlan, correctPlan, rememberPlan, runAssistant, stepText, stopAssistant } from "./api";
import { AssistantSurface } from "./AssistantSurface";
import { ModeToggle } from "./ModeToggle";
import { ACTION_TYPES, defaultStep, slotNum, STEP_FIELDS } from "./steps";
import { useAssistantStore } from "./store";
import { isWeakIntent, routeUtterance } from "./voiceRoute";

beforeEach(async () => {
  calls.length = 0;
  nextHandle = null;
  matchScore = 0.95;
  useAssistantStore.setState({ mode: "orchestra", pending: null, plan: null, ranPlanId: null, reply: null, shown: null, busy: false });
  await applyLanguagePreference("en");
});
afterEach(() => cleanup());

describe("assistant api", () => {
  it("maps results to replies and store state", () => {
    expect(applyResult(run("Open notepad")).reply).toBe("Done: Open notepad");
    expect(applyResult(run("Open x", "Failed")).reply).toContain("not found");
    const r = applyResult({
      status: "NeedsConfirmation",
      pending: { pendingId: "p1", title: "Close notepad", reasons: [{ step: 0, code: "closeDiscard", detail: "notepad", level: "Confirm" }], steps: [] },
    });
    expect(r.outcome).toBe("needsConfirmation");
    expect(r.reply).toContain("notepad closes without saving");
    expect(useAssistantStore.getState().pending?.pendingId).toBe("p1");
    const base = run("m") as Extract<HandleResult, { status: "Executed" }>;
    applyResult({ status: "Executed", run: { ...base.run, effects: [{ type: "SetMode", mode: "Assistant" }] } });
    expect(useAssistantStore.getState().mode).toBe("assistant");
  });

  it("plan flow: propose → correct → approve → remember", async () => {
    nextHandle = { status: "Proposed", plan: PLAN };
    const p = await runAssistant("write a shopping list", "text");
    expect(p.outcome).toBe("proposed");
    expect(useAssistantStore.getState().plan?.planId).toBe("plan-1");
    await correctPlan("add eggs");
    expect(calls.find((c) => c.method === "computer.plan")?.params).toMatchObject({ planId: "plan-1", correction: "add eggs" });
    const ok = await approvePlan();
    expect(ok.outcome).toBe("ok");
    expect(useAssistantStore.getState().ranPlanId).toBe("plan-1");
    const saved = await rememberPlan("shopping list");
    expect(saved.reply).toContain("shopping list");
    expect(useAssistantStore.getState().plan).toBeNull();
  });

  it("describes every step type in both languages", async () => {
    for (const lang of ["en", "ru"] as const) {
      await applyLanguagePreference(lang);
      for (const type of ACTION_TYPES) {
        const text = stepText(defaultStep(type));
        expect(text.length, `${lang} ${type}`).toBeGreaterThan(2);
        expect(text).not.toContain("{{");
      }
    }
    expect(stepText({ type: "CloseApp", app: "notepad", discard: true } as ComputerAction)).toContain("notepad");
  });

  it("step field specs cover every action and parse slot numbers", () => {
    for (const type of ACTION_TYPES) expect(STEP_FIELDS[type]).toBeDefined();
    expect(ACTION_TYPES.length).toBe(33);
    expect(slotNum("5")).toBe(5);
    expect(slotNum("{n}")).toBe("{n}");
  });

  it("global stop clears the pending confirmation", async () => {
    useAssistantStore.setState({ pending: { pendingId: "p", title: "x", reasons: [], steps: [] } });
    await stopAssistant();
    expect(useAssistantStore.getState().pending).toBeNull();
    expect(useAssistantStore.getState().reply?.text).toContain("2");
  });
});

describe("voice routing", () => {
  it("weak Orkestr intents fall through to the computer", () => {
    expect(isWeakIntent({ kind: "askManager", text: "x" })).toBe(true);
    expect(isWeakIntent({ kind: "switchProject", project: null, query: "блокнот" })).toBe(true);
    expect(isWeakIntent({ kind: "stopAll" })).toBe(false);
    expect(isWeakIntent({ kind: "openPanel", panel: "terminal" })).toBe(false);
  });

  it("orchestra mode: Orkestr intents win, matched computer commands run, unknown text reaches the Manager", async () => {
    expect(await routeUtterance("останови всё", { kind: "stopAll" })).toBeNull();
    const r = await routeUtterance("открой блокнот", { kind: "askManager", text: "открой блокнот" });
    expect(r?.outcome).toBe("ok");
    expect(calls.find((c) => c.method === "computer.handle")?.params).toMatchObject({ planUnknown: false, source: "voice" });
    matchScore = 0.3;
    expect(await routeUtterance("как дела у проекта", { kind: "askManager", text: "как дела у проекта" })).toBeNull();
  });

  it("assistant mode: plans unknown commands; yes / no answer the pending prompts", async () => {
    useAssistantStore.setState({ mode: "assistant" });
    matchScore = 0.3;
    nextHandle = { status: "Proposed", plan: PLAN };
    const r = await routeUtterance("составь список покупок", { kind: "askManager", text: "x" });
    expect(r?.outcome).toBe("proposed");
    expect(calls.find((c) => c.method === "computer.handle")?.params).toMatchObject({ planUnknown: true });
    const ok = await routeUtterance("да", { kind: "askManager", text: "да" });
    expect(ok?.outcome).toBe("ok");
    expect(calls.some((c) => c.method === "computer.approvePlan")).toBe(true);

    useAssistantStore.setState({ plan: null, pending: { pendingId: "p9", title: "x", reasons: [], steps: [] } });
    const no = await routeUtterance("нет", { kind: "askManager", text: "нет" });
    expect(no?.outcome).toBe("cancelled");
    expect(calls.find((c) => c.method === "computer.confirm")?.params).toEqual({ pendingId: "p9", approve: false });
  });
});

describe("assistant surface", () => {
  it.each(["en", "ru"] as const)("renders the assistant panel (%s) and runs a typed command", async (lang) => {
    await applyLanguagePreference(lang);
    useAssistantStore.setState({ mode: "assistant" });
    render(
      <>
        <ModeToggle />
        <AssistantSurface />
      </>,
    );
    expect(screen.getByTestId("assistant-surface")).toBeTruthy();
    await waitFor(() => expect(screen.getByText(lang === "ru" ? "Недавние" : "Recent")).toBeTruthy());
    const input = screen.getByRole("textbox", { name: lang === "ru" ? "Введите команду или найдите её…" : "Type a command or search commands…" });
    fireEvent.change(input, { target: { value: "open notepad" } });
    await waitFor(() => expect(screen.getAllByRole("option").length).toBe(1));
    await act(async () => {
      fireEvent.submit(input.closest("form") as HTMLFormElement);
    });
    await waitFor(() => expect(calls.some((c) => c.method === "computer.handle" || c.method === "computer.run")).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: lang === "ru" ? "Режим оркестра" : "Orchestra mode" }));
    expect(useAssistantStore.getState().mode).toBe("orchestra");
  });
});
