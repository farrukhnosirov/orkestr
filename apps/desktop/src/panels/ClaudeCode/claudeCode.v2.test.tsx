// Claude Code panel v2: «Чаты» sidebar (grouping, pin, rename, resume),
// plan usage SESSION | WEEKLY, the commands drawer dispatching commands and
// the «Возможности» buttons.
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ClaudeCodeEvent, ClaudeCodeStatus, ClaudeSessionInfo, Event, Project } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
const responses = new Map<string, unknown>();
const fakeClient = {
  call: (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (!responses.has(method)) return Promise.reject(new Error(`unexpected ${method}`));
    return Promise.resolve(responses.get(method));
  },
  subscribe: () => () => undefined,
};
vi.mock("../../core/client", () => ({
  getClient: () => fakeClient,
  maybeClient: () => fakeClient,
  openProject: () => Promise.resolve(),
}));

import { registerWorkspace } from "../../components/workspaceActions";
import { useClaudeStore } from "../../features/claudeCode/store";
import { useVsCodeStore } from "../../features/vscode/store";
import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import ClaudeCodePanel from "./index";

const P = "11111111-1111-4111-8111-111111111111";
const S = "22222222-2222-4222-8222-222222222222";
const H = "06a417de-ea3e-4314-9648-2072200e551d";

function info(patch: Partial<ClaudeSessionInfo> = {}): ClaudeSessionInfo {
  return {
    sessionId: S,
    projectId: P,
    cwd: "C:\\work\\app",
    claudeSessionId: null,
    model: null,
    permissionMode: "default",
    bypassAllowed: false,
    state: "idle",
    title: null,
    pid: 1,
    startedAt: "2026-10-08T00:00:00Z",
    error: null,
    ...patch,
  };
}

function status(): ClaudeCodeStatus {
  return {
    binary: { path: "C:\\x\\claude.exe", source: "vscodeExtension", version: "2.1.292" },
    binaryOverride: null,
    overrideError: null,
    auth: { loggedIn: true, authMethod: "claude.ai", subscriptionType: "max", email: "dev@example.com" },
    authError: null,
    sessions: [],
  };
}

const usage = {
  available: true,
  subscriptionType: "max",
  session: { percent: 14, resetsAt: new Date(Date.now() + 90 * 60_000).toISOString(), label: null, severity: "normal" },
  weekly: { percent: 23, resetsAt: "2026-10-13T17:00:00Z", label: null, severity: "normal" },
  weeklyModels: [],
  extraUsageEnabled: false,
  fetchedAt: new Date().toISOString(),
  error: null,
};

function emit(event: ClaudeCodeEvent) {
  const ev = { seq: 0, id: "e", ts: "", kind: "ClaudeCodeEvent", payload: { sessionId: S, projectId: P, event } } as unknown as Event;
  act(() => useClaudeStore.getState().onEvent(ev));
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  responses.clear();
  registerWorkspace({ openPanel: () => undefined, resetLayout: () => undefined, toggleMaximize: () => undefined });
  useAppStore.setState({
    projects: [{ id: P, name: "App", rootPath: "C:\\work\\app", archived: false, createdAt: "", lastOpenedAt: null } as unknown as Project],
    activeProjectId: P,
  });
  useClaudeStore.setState({
    status: null,
    statusLoading: false,
    statusError: null,
    sessions: {},
    tabs: {},
    active: {},
    history: {},
    meta: {},
    usage: null,
    usageLoading: false,
    insert: null,
    error: null,
    busy: false,
  });
  responses.set("claudeCode.status", status());
  responses.set("claudeCode.usage", usage);
  responses.set("claudeCode.sessions", { live: [], history: [], meta: [] });
});
afterEach(() => {
  cleanup();
  registerWorkspace(null);
});

describe("Claude Code panel v2", () => {
  it("sidebar lists past chats by day; pin and rename go through claudeCode.setChatMeta; click resumes", async () => {
    responses.set("claudeCode.sessions", {
      live: [],
      history: [{ claudeSessionId: H, title: "Fix the login bug", firstPrompt: "fix", createdAt: null, updatedAt: new Date().toISOString(), messages: 6 }],
      meta: [],
    });
    render(<ClaudeCodePanel projectId={P} />);
    expect(await screen.findByText("Fix the login bug")).toBeTruthy();
    expect(screen.getByRole("region", { name: "Today" })).toBeTruthy();
    expect(screen.getByText("6 messages")).toBeTruthy();
    // Usage: SESSION | WEEKLY.
    await waitFor(() => expect(screen.getAllByText("14%").length).toBeGreaterThan(0));
    expect(screen.getAllByText("23%").length).toBeGreaterThan(0);

    responses.set("claudeCode.setChatMeta", { claudeSessionId: H, title: null, pinned: true, hidden: false });
    fireEvent.click(screen.getByRole("button", { name: "Chat actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Pin" }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.setChatMeta")?.params).toEqual({ claudeSessionId: H, pinned: true }));
    expect(await screen.findByRole("region", { name: "Pinned" })).toBeTruthy();

    responses.set("claudeCode.setChatMeta", { claudeSessionId: H, title: "Login", pinned: true, hidden: false });
    fireEvent.click(screen.getByRole("button", { name: "Chat actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    fireEvent.change(screen.getByLabelText("Chat title"), { target: { value: "Login" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(calls.filter((c) => c.method === "claudeCode.setChatMeta").at(-1)?.params).toEqual({ claudeSessionId: H, title: "Login" }),
    );
    expect(await screen.findByText("Login")).toBeTruthy();

    responses.set("claudeCode.start", { session: info({ claudeSessionId: H }), history: [] });
    fireEvent.click(screen.getByText("Login"));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.start")?.params).toMatchObject({ projectId: P, resume: H }));
  });

  it("commands drawer: cards with descriptions send the command; features are buttons", async () => {
    responses.set("claudeCode.start", { session: info(), history: [] });
    render(<ClaudeCodePanel projectId={P} />);
    fireEvent.click((await screen.findAllByRole("button", { name: "New chat" }))[0] as HTMLElement);
    await screen.findByRole("combobox", { name: "Message to Claude" });
    emit({
      type: "capabilities",
      models: [{ value: "default", displayName: "Default", description: null, effortLevels: ["low", "high"] }],
      commands: [
        { name: "compact", description: "Free up context", argumentHint: "<instructions>", builtin: true },
        { name: "security-review", description: "Security", argumentHint: null, builtin: true },
        { name: "my-deploy", description: "Deploy to staging", argumentHint: null, builtin: false },
      ],
      account: null,
    });
    fireEvent.click(screen.getByRole("button", { name: "Commands" }));
    const drawer = await screen.findByRole("dialog", { name: "Claude Code commands" });
    expect(drawer.textContent).toContain("Compress the conversation into a summary to free up context");
    expect(drawer.textContent).toContain("Deploy to staging");
    expect(screen.getByRole("region", { name: "Custom commands and skills" })).toBeTruthy();
    responses.set("claudeCode.send", info({ state: "running" }));
    fireEvent.change(screen.getByLabelText("Argument for /compact"), { target: { value: "keep API notes" } });
    fireEvent.click(screen.getByRole("button", { name: "Run /compact" }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.send")?.params).toEqual({ sessionId: S, text: "/compact keep API notes" }));
    expect(screen.queryByRole("dialog", { name: "Claude Code commands" })).toBeNull();

    responses.set("claudeCode.setMode", info({ permissionMode: "plan" }));
    fireEvent.click(screen.getByRole("button", { name: "Plan mode" }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.setMode")?.params).toEqual({ sessionId: S, mode: "plan" }));
    responses.set("claudeCode.setThinking", info());
    fireEvent.click(screen.getByRole("button", { name: "Extended thinking" }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.setThinking")?.params).toEqual({ sessionId: S, enabled: true }));
    responses.set("claudeCode.contextUsage", {
      totalTokens: 25000,
      maxTokens: 200000,
      percentage: 12.5,
      categories: [{ name: "Messages", tokens: 900, kind: "used" }],
    });
    fireEvent.click(screen.getByRole("button", { name: "Context" }));
    expect(await screen.findByText("Messages")).toBeTruthy();
  });

  it("«In VS Code» opens the same chat in the Claude Code extension through the bridge", async () => {
    useVsCodeStore.setState({ status: { installed: true, bridgeConnected: true } as never });
    Object.assign(navigator, { clipboard: { writeText: () => Promise.resolve() } });
    responses.set("claudeCode.start", { session: info({ claudeSessionId: H }), history: [] });
    responses.set("vscode.execute", { ok: true, result: null });
    render(<ClaudeCodePanel projectId={P} />);
    fireEvent.click((await screen.findAllByRole("button", { name: "New chat" }))[0] as HTMLElement);
    await screen.findByRole("combobox", { name: "Message to Claude" });
    fireEvent.click(screen.getByRole("button", { name: "In VS Code" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "vscode.execute")?.params).toMatchObject({ commandId: "claude-vscode.editor.open", args: [H] }),
    );
  });

  it("usage shows «No data» when the CLI reports no plan limits", async () => {
    responses.set("claudeCode.usage", { ...usage, available: false, session: null, weekly: null, error: "none" });
    render(<ClaudeCodePanel projectId={P} />);
    await waitFor(() => expect(screen.getAllByText("No data").length).toBeGreaterThan(0));
  });
});
