import type { CursorSession, CursorSettings, CursorStatus, Event as OkEvent } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];

const SETTINGS: CursorSettings = {
  source: "subscriptions",
  subscriptionOrder: ["claudeCode", "codex"],
  apiOrder: ["anthropic", "openai", "gemini"],
  models: {},
  maxSteps: 30,
  maxSeconds: 300,
  abortOnMouse: true,
  minimizeOrkestr: true,
  maxImageSide: 1280,
  consents: {},
};

const SESSION: CursorSession = {
  sessionId: "cursor-1",
  task: "open bluetooth settings and turn it on",
  source: "ui",
  state: "AwaitingConfirmation",
  backend: "claudeCode",
  model: "Claude Code · sonnet",
  plan: ["open settings", "turn bluetooth on"],
  current: "Turning Bluetooth on",
  steps: [
    {
      index: 1,
      at: "2026-10-26T00:00:00Z",
      thought: "open settings",
      action: { type: "launch", app: "ms-settings:bluetooth" },
      outcome: "ok",
      detail: "",
      backend: "claudeCode",
      thinkMs: 1200,
      shot: { id: "shot-1", width: 1280, height: 720 },
    },
  ],
  lastShot: { id: "shot-1", width: 1280, height: 720 },
  confirmation: {
    code: "system",
    detail: "Bluetooth",
    action: { type: "click", button: "Left", double: false, element: 4 },
    shot: { id: "shot-1", width: 1280, height: 720 },
    targetRect: [640, 100, 80, 30],
  },
  maxSteps: 30,
  maxSeconds: 300,
  startedAt: "2026-10-26T00:00:00Z",
  lang: "en",
};

let status: CursorStatus = {
  backends: [
    { id: "claudeCode", label: "Claude Code", source: "subscriptions", available: true, detail: "subscription:max", model: "sonnet", consented: false },
    { id: "codex", label: "Codex (ChatGPT)", source: "subscriptions", available: true, detail: "subscription:ChatGPT", model: "", consented: true },
    { id: "anthropic", label: "Anthropic API", source: "api", available: false, detail: "noKey", model: "claude-sonnet-4-5", consented: false },
  ],
  activeBackend: "claudeCode",
  settings: SETTINGS,
  supported: true,
};
let startResult: unknown = { status: "NeedsConsent", backend: status.backends[0] };

const fake = {
  call: vi.fn((method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "computer.cursorStatus":
      case "computer.cursorConfirm":
      case "computer.cursorStop":
        return Promise.resolve(status);
      case "computer.cursorSetSettings":
        status = { ...status, settings: params.settings as CursorSettings };
        return Promise.resolve(status);
      case "computer.cursorConsent":
        startResult = { status: "Started", session: { ...SESSION, state: "Thinking", confirmation: undefined } };
        return Promise.resolve(status);
      case "computer.cursorStart":
        return Promise.resolve(startResult);
      case "computer.cursorScreenshot":
        return Promise.resolve({ mediaType: "image/png", dataBase64: "iVBORw0KGgo=", width: 1280, height: 720 });
      default:
        return Promise.resolve({});
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

import { applyLanguagePreference } from "../../i18n";
import { routeCursor } from "../assistant/voiceRoute";
import { CursorPanel } from "./CursorPanel";
import { CursorSettingsSection, reorder } from "./CursorSettingsSection";
import { actionText, onCursorEvent, onCursorPhrase, startCursor, useCursorStore } from "./store";
import { parseCursorTrigger } from "./trigger";

beforeEach(async () => {
  calls.length = 0;
  startResult = { status: "NeedsConsent", backend: status.backends[0] };
  status = { ...status, session: undefined, settings: SETTINGS };
  useCursorStore.setState({ status: null, active: false, shots: {}, consent: null, awaitingTask: false, error: null, busy: false, open: false, dismissed: null });
  await applyLanguagePreference("en");
});
afterEach(() => cleanup());

describe("cursor triggers", () => {
  it("recognizes Russian and English triggers and extracts the task", () => {
    expect(parseCursorTrigger("Джарвис, возьми управление и открой настройки блютуз")).toBe("открой настройки блютуз");
    expect(parseCursorTrigger("захвати курсор")).toBe("");
    expect(parseCursorTrigger("возьми управление курсором: закрой все окна кроме VS Code")).toBe("закрой все окна кроме VS Code");
    expect(parseCursorTrigger("Jarvis, take control and find tickets to Tashkent")).toBe("find tickets to Tashkent");
    expect(parseCursorTrigger("take over the mouse")).toBe("");
    expect(parseCursorTrigger("открой блокнот")).toBeNull();
    expect(parseCursorTrigger("возьми яблоко")).toBeNull();
  });
});

describe("cursor store", () => {
  it("needs consent first, then starts after it is granted", async () => {
    const r = await startCursor("open notepad", "ui");
    expect(r?.status).toBe("NeedsConsent");
    expect(useCursorStore.getState().consent?.backend.id).toBe("claudeCode");
    render(<CursorPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "Allow and start" }));
    await waitFor(() => expect(calls.some((c) => c.method === "computer.cursorConsent" && c.params["backend"] === "claudeCode")).toBe(true));
    await waitFor(() => expect(calls.filter((c) => c.method === "computer.cursorStart")).toHaveLength(2));
    expect(useCursorStore.getState().consent).toBeNull();
  });

  it("speaks event phrases and tells listeners", () => {
    const heard: string[] = [];
    const off = onCursorPhrase((t) => heard.push(t));
    const ev = {
      seq: 1,
      id: "e",
      ts: "2026-10-26T00:00:00Z",
      projectId: null,
      taskId: null,
      agentId: null,
      nodeId: null,
      correlationId: null,
      kind: "ComputerCursor",
      payload: { sessionId: "cursor-1", event: "state", state: "Starting", say: "Taking control, sir.", steps: 0 },
    } as unknown as OkEvent;
    onCursorEvent(ev);
    off();
    expect(heard).toEqual(["Taking control, sir."]);
  });

  it("describes actions", () => {
    expect(actionText({ type: "click", button: "Left", double: false, element: 3 })).toBe("Click #3");
    expect(actionText({ type: "type", text: "привет" })).toBe("Type text (6 chars)");
    expect(actionText({ type: "keys", keys: ["ctrl", "l"] })).toBe("Press ctrl+l");
  });
});

describe("cursor panel", () => {
  it("shows the live session with the highlighted target and confirms", async () => {
    status = { ...status, session: SESSION };
    useCursorStore.setState({ status, active: true });
    render(<CursorPanel />);
    expect(await screen.findByText("Waiting for your confirmation")).toBeTruthy();
    expect(screen.getByText("Changing system settings")).toBeTruthy();
    const target = screen.getAllByTestId("cursor-target")[0] as HTMLElement;
    expect(target.style.left).toBe("50%");
    expect(screen.getByText("turn bluetooth on")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(calls.some((c) => c.method === "computer.cursorConfirm" && c.params["approve"] === true)).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(calls.some((c) => c.method === "computer.cursorStop")).toBe(true));
    await waitFor(() => expect(calls.some((c) => c.method === "computer.cursorScreenshot")).toBe(true));
  });

  it("starts from the launcher with the active model shown", async () => {
    render(<CursorPanel />);
    fireEvent.click(await screen.findByTestId("cursor-launch"));
    expect(await screen.findByText("Model: Claude Code · sonnet")).toBeTruthy();
    fireEvent.change(screen.getByLabelText(/What should I do/), { target: { value: "open notepad" } });
    fireEvent.click(screen.getByRole("button", { name: "Take control" }));
    await waitFor(() => expect(calls.some((c) => c.method === "computer.cursorStart" && c.params["task"] === "open notepad")).toBe(true));
  });
});

describe("settings + voice", () => {
  it("reorders backends inside a source", () => {
    const s = reorder(SETTINGS, "subscriptions", ["claudeCode", "codex"], "codex", -1);
    expect(s.subscriptionOrder).toEqual(["codex", "claudeCode"]);
    expect(reorder(SETTINGS, "api", ["anthropic", "openai"], "anthropic", -1)).toBe(SETTINGS);
  });

  it("a model typed right before another change is not lost", async () => {
    render(<CursorSettingsSection />);
    const row = await screen.findByTestId("cursor-backend-claudeCode");
    const input = within(row).getByPlaceholderText("sonnet");
    fireEvent.change(input, { target: { value: "opus" } });
    // Leaving the field saves the model; the click lands before that save returns.
    fireEvent.blur(input);
    fireEvent.click(within(row).getByRole("button", { name: "Move down" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "computer.cursorSetSettings")).toHaveLength(2));
    await waitFor(() => expect(useCursorStore.getState().status?.settings.subscriptionOrder).toEqual(["codex", "claudeCode"]));
    const last = calls.filter((c) => c.method === "computer.cursorSetSettings").at(-1)!.params.settings as CursorSettings;
    expect(last.models).toEqual({ claudeCode: "opus" });
    expect(last.subscriptionOrder).toEqual(["codex", "claudeCode"]);
    expect(useCursorStore.getState().status?.settings.models).toEqual({ claudeCode: "opus" });
  });

  it("voice: trigger without a task asks for it, the next utterance starts", async () => {
    const r = await routeCursor("Джарвис, возьми управление");
    expect(r?.reply).toBe("What shall I do?");
    expect(useCursorStore.getState().awaitingTask).toBe(true);
    startResult = { status: "Started", session: { ...SESSION, state: "Starting", confirmation: undefined } };
    const r2 = await routeCursor("открой настройки блютуз");
    expect(r2?.outcome).toBe("ok");
    expect(calls.find((c) => c.method === "computer.cursorStart")?.params).toMatchObject({ task: "открой настройки блютуз", source: "voice" });
    expect(await routeCursor("открой блокнот")).toBeNull();
  });
});
