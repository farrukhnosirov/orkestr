// Claude Code panel: not found / login / chat screens, streamed chat
// rendering, tool cards, permission card answers, composer send / queue /
// interrupt, palette command + shortcut, "Ask Claude about selection".
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
import { shortcutCommand } from "../../features/commands/dispatch";
import { commands } from "../../features/commands/registry";
import { askClaude } from "../../features/files/actions";
import { useClaudeStore } from "../../features/claudeCode/store";
import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import ClaudeCodePanel from "./index";

const P = "11111111-1111-4111-8111-111111111111";
const S = "22222222-2222-4222-8222-222222222222";
const opened: string[] = [];

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

function status(patch: Partial<ClaudeCodeStatus> = {}): ClaudeCodeStatus {
  return {
    binary: { path: "C:\\x\\claude.exe", source: "vscodeExtension", version: "2.1.292" },
    binaryOverride: null,
    overrideError: null,
    auth: { loggedIn: true, authMethod: "claude.ai", subscriptionType: "max", email: "dev@example.com" },
    authError: null,
    sessions: [],
    ...patch,
  };
}

function emit(event: ClaudeCodeEvent) {
  const ev = { seq: 0, id: "e", ts: "", kind: "ClaudeCodeEvent", payload: { sessionId: S, projectId: P, event } } as unknown as Event;
  act(() => useClaudeStore.getState().onEvent(ev));
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  responses.clear();
  opened.length = 0;
  registerWorkspace({ openPanel: (id) => opened.push(id), resetLayout: () => undefined, toggleMaximize: () => undefined });
  useAppStore.setState({
    projects: [{ id: P, name: "App", rootPath: "C:\\work\\app", archived: false, createdAt: "", lastOpenedAt: null } as unknown as Project],
    activeProjectId: P,
  });
  useClaudeStore.setState({ status: null, statusLoading: false, statusError: null, sessions: {}, tabs: {}, active: {}, history: {}, insert: null, error: null, busy: false });
  responses.set("claudeCode.sessions", { live: [], history: [] });
});
afterEach(() => {
  cleanup();
  registerWorkspace(null);
});

async function openChat() {
  responses.set("claudeCode.status", status());
  responses.set("claudeCode.start", { session: info(), history: [] });
  render(<ClaudeCodePanel projectId={P} />);
  fireEvent.click((await screen.findAllByRole("button", { name: "New chat" }))[0] as HTMLElement);
  await screen.findByRole("combobox", { name: "Message to Claude" });
}

describe("Claude Code panel", () => {
  it("asks for a path when Claude Code is not installed", async () => {
    responses.set("claudeCode.status", status({ binary: null }));
    render(<ClaudeCodePanel projectId={P} />);
    expect(await screen.findByText("Claude Code was not found")).toBeTruthy();
    responses.set("claudeCode.setBinaryPath", status());
    fireEvent.change(screen.getByLabelText("Path to the claude executable"), { target: { value: " C:\\t\\claude.exe " } });
    fireEvent.click(screen.getByRole("button", { name: "Use this path" }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeCode.setBinaryPath")?.params).toEqual({ path: "C:\\t\\claude.exe" }));
  });

  it("not signed in: «Sign in» opens a login terminal", async () => {
    responses.set("claudeCode.status", status({ auth: { loggedIn: false, authMethod: null, subscriptionType: null, email: null } }));
    responses.set("claudeCode.loginInTerminal", { terminal: { terminalId: "33333333-3333-4333-8333-333333333333" } });
    render(<ClaudeCodePanel projectId={P} />);
    fireEvent.click(await screen.findByRole("button", { name: "Sign in to Claude" }));
    await waitFor(() => expect(opened).toEqual(["terminal"]));
    expect(calls.find((c) => c.method === "claudeCode.loginInTerminal")?.params).toEqual({ projectId: P });
    expect(await screen.findByText(/claude auth login/)).toBeTruthy();
  });

  it("starts a chat, streams text, renders tools and answers a permission", async () => {
    await openChat();
    expect(calls.find((c) => c.method === "claudeCode.start")?.params).toMatchObject({ projectId: P, permissionMode: "default" });
    emit({ type: "capabilities", models: [{ value: "default", displayName: "Default", description: null }, { value: "haiku", displayName: "Haiku", description: null }], commands: [{ name: "compact", description: "Compact", argumentHint: null }], account: null });
    emit({ type: "userMessage", text: "fix it" });
    emit({ type: "thinking" });
    expect(screen.getByText("Thinking…")).toBeTruthy();
    emit({ type: "textDelta", text: "Looking **now**", parentToolUseId: null });
    expect(screen.getByText("now").tagName).toBe("STRONG");
    emit({ type: "toolUse", id: "t1", name: "Edit", input: { file_path: "C:\\work\\app\\src\\a.ts", old_string: "a", new_string: "b" }, parentToolUseId: null });
    emit({ type: "toolUse", id: "t2", name: "TodoWrite", input: { todos: [{ content: "Step one", status: "in_progress" }] }, parentToolUseId: null });
    expect(screen.getByRole("button", { name: "Open src/a.ts" })).toBeTruthy();
    expect(screen.getByText("Step one")).toBeTruthy();
    emit({
      type: "permissionRequest",
      requestId: "req-1",
      toolName: "Bash",
      toolUseId: "t3",
      input: { command: "git push --force" },
      description: "push",
      blockedPath: null,
      risk: { level: "dangerous", category: "GitDestructive", reason: "force push" },
      canRemember: false,
    });
    expect(screen.getByText(/Dangerous · Destructive git/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Allow for this session" })).toBeNull();
    responses.set("claudeCode.respondPermission", info({ state: "running" }));
    fireEvent.click(screen.getByRole("button", { name: "Deny with a reason…" }));
    fireEvent.change(screen.getByLabelText("Reason for Claude"), { target: { value: "use a PR" } });
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "claudeCode.respondPermission")?.params).toEqual({
        sessionId: S,
        requestId: "req-1",
        decision: "deny",
        message: "use a PR",
      }),
    );
    emit({ type: "permissionResolved", requestId: "req-1", decision: "deny", byPolicy: false, message: "use a PR" });
    expect(screen.getByText(/Denied/)).toBeTruthy();
    emit({ type: "toolResult", toolUseId: "t1", content: "ok", isError: false, parentToolUseId: null });
    emit({ type: "fileChanged", path: "C:\\work\\app\\src\\a.ts" });
    emit({ type: "result", isError: false, subtype: "success", result: "done", durationMs: 1200, numTurns: 2, totalCostUsd: 0, usage: { inputTokens: 10, outputTokens: 20, cacheReadTokens: 5, cacheWriteTokens: 0 } });
    expect(screen.getByText("1 file changed:")).toBeTruthy();
    expect(screen.getByText(/15 in · 20 out tokens/)).toBeTruthy();
    // Clicking a changed file (result footer) opens it in the Code panel.
    fireEvent.click(screen.getByRole("button", { name: "src/a.ts" }));
    expect(opened).toContain("code");
  });

  it("composer: Enter sends, queues while running, Esc interrupts, / completes commands", async () => {
    await openChat();
    emit({ type: "capabilities", models: [], commands: [{ name: "compact", description: "Compact the conversation", argumentHint: null }], account: null });
    const box = screen.getByRole("combobox", { name: "Message to Claude" });
    fireEvent.change(box, { target: { value: "/co", selectionStart: 3 } });
    expect(await screen.findByRole("option", { name: /\/compact/ })).toBeTruthy();
    fireEvent.keyDown(box, { key: "Enter" });
    expect((box as HTMLTextAreaElement).value).toBe("/compact ");
    responses.set("claudeCode.send", info({ state: "running" }));
    fireEvent.change(box, { target: { value: "hello", selectionStart: 5 } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.filter((c) => c.method === "claudeCode.send")).toHaveLength(1));
    emit({ type: "userMessage", text: "hello" });
    // Busy: the next message is queued, not sent.
    fireEvent.change(box, { target: { value: "and then", selectionStart: 8 } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(await screen.findByText("and then")).toBeTruthy();
    expect(calls.filter((c) => c.method === "claudeCode.send")).toHaveLength(1);
    responses.set("claudeCode.interrupt", info());
    fireEvent.keyDown(box, { key: "Escape" });
    await waitFor(() => expect(calls.some((c) => c.method === "claudeCode.interrupt")).toBe(true));
  });

  it("queued message is sent after the turn ends", async () => {
    await openChat();
    responses.set("claudeCode.send", info({ state: "running" }));
    await act(async () => {
      await useClaudeStore.getState().send(S, "first");
    });
    emit({ type: "userMessage", text: "first" });
    await act(async () => {
      await useClaudeStore.getState().send(S, "second");
    });
    expect(useClaudeStore.getState().sessions[S]?.queue).toEqual(["second"]);
    emit({ type: "result", isError: false, subtype: "success", result: null, durationMs: 1, numTurns: 1, totalCostUsd: null, usage: { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 } });
    await waitFor(() => expect(calls.filter((c) => c.method === "claudeCode.send").map((c) => c.params.text)).toEqual(["first", "second"]));
  });

  it("rewinds the chat to before a user message and puts it back into the composer", async () => {
    await openChat();
    const done = { type: "result", isError: false, subtype: "success", result: null, durationMs: 1, numTurns: 1, totalCostUsd: null, usage: { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 } } as const;
    emit({ type: "userMessage", text: "first" });
    emit(done);
    emit({ type: "userMessage", text: "second" });
    emit({ type: "text", text: "second answer", parentToolUseId: null, synthetic: false });
    emit(done);
    responses.set("claudeCode.rewind", { session: info({ claudeSessionId: "06a417de-ea3e-4314-9648-2072200e551d" }), text: "first", filesChanged: ["a.ts"] });
    fireEvent.click(screen.getAllByRole("button", { name: "Rewind to here" })[0] as HTMLElement);
    fireEvent.click(screen.getByRole("button", { name: "Rewind chat and files" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "claudeCode.rewind")?.params).toEqual({ sessionId: S, turnsFromEnd: 1, text: "first", restoreFiles: true }),
    );
    await waitFor(() => expect((screen.getByRole("combobox", { name: "Message to Claude" }) as HTMLTextAreaElement).value).toBe("first"));
    expect(screen.queryByText("second answer")).toBeNull();
    expect(screen.getByText("Restored 1 file")).toBeTruthy();
  });

  it("Ask Claude about selection fills the composer", async () => {
    await openChat();
    act(() => askClaude("@src/a.ts:3\n```\nx\n```\n"));
    expect(opened).toContain("claudeCode");
    await waitFor(() => expect((screen.getByRole("combobox", { name: "Message to Claude" }) as HTMLTextAreaElement).value).toContain("@src/a.ts:3"));
  });

  it("bypass mode needs a danger confirmation", async () => {
    await openChat();
    responses.set("claudeCode.setMode", info({ permissionMode: "bypassPermissions", bypassAllowed: true }));
    fireEvent.click(screen.getByRole("radio", { name: /No confirmations/ }));
    expect(calls.some((c) => c.method === "claudeCode.setMode")).toBe(false);
    fireEvent.click(await screen.findByRole("button", { name: "Turn off confirmations" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "claudeCode.setMode")?.params).toEqual({ sessionId: S, mode: "bypassPermissions", confirmBypass: true }),
    );
  });
});

describe("Claude Code commands", () => {
  it("Ctrl+Alt+C opens the chat (also on the Russian layout); commands are searchable in both languages", () => {
    const ev = { key: "с", code: "KeyC", ctrlKey: true, altKey: true, shiftKey: false, metaKey: false, target: null };
    expect(shortcutCommand(ev, commands, { projectId: P })?.id).toBe("claudeCode.open");
    for (const id of ["claudeCode.open", "claudeCode.newChat", "claudeCode.askSelection", "claudeCode.interrupt"]) {
      const c = commands.find((x) => x.id === id);
      expect(c, id).toBeDefined();
      expect(c?.keywords?.some((k) => /[a-z]/.test(k))).toBe(true);
      expect(c?.keywords?.some((k) => /[а-я]/.test(k))).toBe(true);
    }
  });

  it("Russian titles", async () => {
    await i18n.changeLanguage("ru");
    expect(commands.find((c) => c.id === "claudeCode.open")?.title).toBe("Claude Code: открыть чат");
    expect(commands.find((c) => c.id === "claudeCode.askSelection")?.title).toBe("Claude Code: спросить о выделенном");
    await i18n.changeLanguage("en");
  });
});
