// Codex panel: install / sign-in / chat screens, streamed chat with tool and
// approval cards, slash commands, pickers, limits; the engines settings card,
// the title-bar indicator and the agent engine badge.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodexEvent, CodexLimits, CodexSessionInfo, CodexStatus, EnginesStatus, Event, Project } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
const responses = new Map<string, unknown>();
const fakeClient = {
  call: (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (!responses.has(method)) return Promise.reject(new Error(`unexpected ${method}`));
    const r = responses.get(method);
    return typeof r === "function" ? Promise.resolve((r as (p: unknown) => unknown)(params)) : Promise.resolve(r);
  },
  subscribe: () => () => undefined,
};
vi.mock("../../core/client", () => ({
  getClient: () => fakeClient,
  maybeClient: () => fakeClient,
  openProject: () => Promise.resolve(),
}));

import { registerWorkspace } from "../../components/workspaceActions";
import { commands } from "../../features/commands/registry";
import { useCodexStore, DEFAULT_CODEX_PREFS } from "../../features/codex/store";
import { AgentEngineBadge } from "../../features/engines/AgentEngineBadge";
import { EnginesCard } from "../../features/engines/EnginesCard";
import { EnginesIndicator } from "../../features/engines/EnginesIndicator";
import { useEnginesStore } from "../../features/engines/store";
import { useProvidersStore } from "../../features/providers/store";
import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import CodexPanel from "./index";

const P = "11111111-1111-4111-8111-111111111111";
const S = "33333333-3333-4333-8333-333333333333";
const opened: string[] = [];

function info(patch: Partial<CodexSessionInfo> = {}): CodexSessionInfo {
  return {
    sessionId: S,
    projectId: P,
    cwd: "C:\\work\\app",
    threadId: "th-1",
    model: null,
    effort: null,
    mode: "ask",
    state: "idle",
    title: null,
    startedAt: "2026-10-23T09:00:00Z",
    error: null,
    ...patch,
  };
}

function status(patch: Partial<CodexStatus> = {}): CodexStatus {
  return {
    binary: { path: "C:\\tools\\codex.cmd", source: "orkestr", version: "0.163.0" },
    latestVersion: "0.163.0",
    updateAvailable: false,
    npmAvailable: true,
    loggedIn: true,
    account: { kind: "chatgpt", email: "dev@example.com", plan: "plus" },
    login: null,
    providerId: null,
    sessions: [],
    error: null,
    ...patch,
  };
}

const limits: CodexLimits = {
  available: true,
  plan: "plus",
  primary: { percent: 42, windowMinutes: 300, resetsAt: null },
  secondary: { percent: 12, windowMinutes: 10080, resetsAt: null },
  credits: null,
  limitReached: null,
  usageAllowed: true,
  fetchedAt: "2026-10-23T09:00:00Z",
  error: null,
};

function emit(event: CodexEvent) {
  const ev = { seq: 0, id: "e", ts: "", kind: "CodexEvent", payload: { sessionId: S, projectId: P, event } } as unknown as Event;
  act(() => useCodexStore.getState().onEvent(ev));
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
  useCodexStore.setState({
    status: null,
    statusLoading: false,
    statusError: null,
    installing: false,
    install: null,
    login: null,
    loginBusy: false,
    limits: null,
    limitsLoading: false,
    models: [],
    threads: {},
    threadsLoading: false,
    threadsError: null,
    sessions: {},
    order: {},
    active: {},
    insert: null,
    busy: false,
    error: null,
    prefs: DEFAULT_CODEX_PREFS,
  });
  responses.set("codex.threads", { threads: [{ id: "th-old", title: "Fix the login bug", cwd: "C:\\work\\app", model: "gpt-5-codex", createdAt: null, updatedAt: "2026-10-01T10:00:00Z", forkedFrom: null }], live: [], error: null });
  responses.set("codex.limits", limits);
  responses.set("codex.models", {
    models: [
      { id: "gpt-5-codex", displayName: "GPT-5 Codex", description: "", efforts: ["low", "medium", "high"], defaultEffort: "medium", isDefault: true },
      { id: "gpt-5", displayName: "GPT-5", description: "", efforts: ["minimal", "low", "medium", "high"], defaultEffort: "medium", isDefault: false },
    ],
    fallback: false,
    error: null,
  });
  responses.set("codex.transcript", { events: [] });
});
afterEach(() => {
  cleanup();
  registerWorkspace(null);
});

describe("setup screens", () => {
  it("installs the CLI and shows the npm log tail on failure", async () => {
    responses.set("codex.status", status({ binary: null, loggedIn: false, account: null }));
    responses.set("codex.install", { ok: false, log: "npm ERR! network\nnpm ERR! code ETIMEDOUT", status: status({ binary: null, loggedIn: false, account: null }) });
    render(<CodexPanel projectId={P} />);
    const btn = await screen.findByTestId("codex-install-button");
    expect(btn.textContent).toBe("Install Codex CLI");
    fireEvent.click(btn);
    await screen.findByText("Installation failed. Last lines of the npm log:");
    expect(screen.getByText(/ETIMEDOUT/)).toBeTruthy();
    expect(calls.some((c) => c.method === "codex.install")).toBe(true);
  });

  it("asks for npm when it is missing", async () => {
    responses.set("codex.status", status({ binary: null, npmAvailable: false, loggedIn: false, account: null }));
    render(<CodexPanel projectId={P} />);
    expect(((await screen.findByTestId("codex-install-button")) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/npm was not found/)).toBeTruthy();
  });

  it("signs in with ChatGPT in the browser (link + waiting) and with a device code (copyable code), in Russian", async () => {
    await i18n.changeLanguage("ru");
    responses.set("codex.status", status({ loggedIn: false, account: null, updateAvailable: true, latestVersion: "0.170.0" }));
    responses.set("codex.login", (p: unknown) =>
      (p as { mode: string }).mode === "browser"
        ? { loginId: "l1", mode: "browser", phase: "pending", authUrl: "https://auth.openai.com/x", verificationUrl: null, userCode: null, error: null, startedAt: "" }
        : { loginId: "l2", mode: "deviceCode", phase: "pending", authUrl: null, verificationUrl: "https://auth.openai.com/device", userCode: "ABCD-1234", error: null, startedAt: "" },
    );
    responses.set("codex.loginCancel", status({ loggedIn: false, account: null }));
    render(<CodexPanel projectId={P} />);
    const signIn = await screen.findByTestId("codex-signin-button");
    expect(signIn.textContent).toBe("Войти через ChatGPT");
    expect(screen.getByTestId("codex-update-button").textContent).toBe("Обновить до 0.170.0");
    fireEvent.click(signIn);
    await screen.findByText("Ожидание входа в браузере…");
    expect(screen.getByText("Открыть страницу входа").closest("a")?.getAttribute("href")).toBe("https://auth.openai.com/x");
    fireEvent.click(screen.getByTestId("codex-login-cancel"));
    await waitFor(() => expect(calls.some((c) => c.method === "codex.loginCancel")).toBe(true));
    act(() => useCodexStore.setState({ login: null }));
    fireEvent.click(await screen.findByTestId("codex-signin-device"));
    expect((await screen.findByTestId("codex-user-code")).textContent).toBe("ABCD-1234");
    expect(calls.filter((c) => c.method === "codex.login").map((c) => c.params.mode)).toEqual(["browser", "deviceCode"]);
    // The account event finishes the sign-in.
    act(() =>
      useCodexStore.getState().onEvent({
        seq: 0,
        kind: "CodexAccount",
        payload: { loggedIn: true, account: { kind: "chatgpt", email: "dev@example.com", plan: "pro" }, login: null },
      } as unknown as Event),
    );
    expect(await screen.findByText("dev@example.com")).toBeTruthy();
    await i18n.changeLanguage("en");
  });
});

async function openChat() {
  responses.set("codex.status", status());
  responses.set("codex.start", info());
  responses.set("codex.send", info({ state: "running" }));
  render(<CodexPanel projectId={P} />);
  fireEvent.click(await screen.findByTestId("codex-start"));
  await screen.findByTestId("codex-transcript");
}

describe("chat", () => {
  it("lists saved threads, starts a chat with the preferences and streams the answer", async () => {
    await openChat();
    expect(screen.getByText("Fix the login bug")).toBeTruthy();
    expect(calls.find((c) => c.method === "codex.start")?.params).toMatchObject({ projectId: P, mode: "ask" });
    emit({ type: "userMessage", text: "What does main.ts do?" });
    emit({ type: "textDelta", itemId: "m1", text: "It boots" });
    emit({ type: "text", itemId: "m1", text: "It boots the app." });
    expect(screen.getByText("What does main.ts do?")).toBeTruthy();
    expect(screen.getByText("It boots the app.")).toBeTruthy();
    emit({ type: "toolUse", id: "c1", name: "Bash", input: { command: "npm test", cwd: "C:\\work\\app" } });
    emit({ type: "toolResult", toolUseId: "c1", content: "ok", isError: false });
    expect(screen.getByText("npm test")).toBeTruthy();
    emit({ type: "turnCompleted", turnId: "t1", status: "completed", error: null, durationMs: 1200 });
    expect(screen.getByTestId("codex-turn").textContent).toContain("Done");
    // Model picker shows display names from codex.models.
    const model = screen.getByTestId("codex-model") as HTMLSelectElement;
    expect([...model.options].map((o) => o.textContent)).toEqual(["Default (GPT-5 Codex)", "GPT-5 Codex", "GPT-5"]);
    responses.set("codex.setOptions", info({ model: "gpt-5" }));
    fireEvent.change(model, { target: { value: "gpt-5" } });
    await waitFor(() => expect(calls.find((c) => c.method === "codex.setOptions")?.params).toMatchObject({ sessionId: S, model: "gpt-5" }));
  });

  it("answers approvals: allow / allow for session / deny / stop turn; dangerous hides «for session»; forbidden is auto-denied", async () => {
    await openChat();
    responses.set("codex.respondApproval", info({ state: "running" }));
    emit({ type: "approvalRequest", requestId: "r1", kind: "command", toolName: "Bash", input: { command: "npm i", cwd: "C:\\work\\app" }, reason: "install deps", risk: null, canRemember: true });
    let card = screen.getByTestId("codex-approval");
    expect(within(card).getByText("Codex wants to run a command")).toBeTruthy();
    fireEvent.click(within(card).getByTestId("codex-approval-session"));
    await waitFor(() => expect(calls.find((c) => c.method === "codex.respondApproval")?.params).toEqual({ sessionId: S, requestId: "r1", decision: "allowSession" }));
    emit({ type: "approvalResolved", requestId: "r1", decision: "allowSession", byPolicy: false, message: null });
    expect(within(card).getByText("Allowed for this session")).toBeTruthy();

    emit({
      type: "approvalRequest",
      requestId: "r2",
      kind: "command",
      toolName: "Bash",
      input: { command: "git push --force" },
      reason: null,
      risk: { level: "dangerous", category: "GitForcePush", reason: "rewrites history" } as never,
      canRemember: true,
    });
    card = screen.getAllByTestId("codex-approval")[1] as HTMLElement;
    expect(within(card).queryByTestId("codex-approval-session")).toBeNull();
    fireEvent.click(within(card).getByTestId("codex-approval-abort"));
    await waitFor(() => expect(calls.some((c) => c.method === "codex.respondApproval" && c.params.decision === "abort")).toBe(true));

    emit({
      type: "approvalRequest",
      requestId: "r3",
      kind: "command",
      toolName: "Bash",
      input: { command: "rm -rf /" },
      reason: null,
      risk: { level: "forbidden", category: "DeleteFiles", reason: "wipes the disk" } as never,
      canRemember: false,
    });
    card = screen.getAllByTestId("codex-approval")[2] as HTMLElement;
    expect(within(card).getByText(/denied automatically/)).toBeTruthy();
    expect(within(card).queryByTestId("codex-approval-allow")).toBeNull();
  });

  it("renders Codex patches with their diff", async () => {
    await openChat();
    emit({
      type: "toolUse",
      id: "p1",
      name: "Edit",
      input: { file_path: "C:\\work\\app\\src\\a.ts", changes: [{ path: "C:\\work\\app\\src\\a.ts", kind: "update", diff: "@@ -1 +1 @@\n-old line\n+new line\n" }] },
    });
    const patch = screen.getByTestId("codex-patch");
    expect(within(patch).getByText("src/a.ts")).toBeTruthy();
    expect(within(patch).getByText("new line")).toBeTruthy();
    expect(within(patch).getByText("Applying…")).toBeTruthy();
  });

  it("handles slash commands natively and shows the limits", async () => {
    await openChat();
    responses.set("codex.compact", info());
    const box = screen.getByRole("combobox", { name: "Message to Codex" });
    // The first Enter picks the suggestion from the popup, the second runs it.
    fireEvent.change(box, { target: { value: "/compact" } });
    fireEvent.keyDown(box, { key: "Enter" });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.some((c) => c.method === "codex.compact")).toBe(true));
    expect(calls.some((c) => c.method === "codex.send")).toBe(false);
    fireEvent.change(box, { target: { value: "/status " } });
    fireEvent.keyDown(box, { key: "Enter" });
    await screen.findByText(/Short window: 42/);
    expect(screen.getAllByTestId("codex-limit-primary")[0]?.textContent).toContain("5 hours");
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(calls.find((c) => c.method === "codex.send")?.params).toEqual({ sessionId: S, text: "hello" }));
  });

  it("shows unknown limits and the usage-limit error", async () => {
    responses.set("codex.limits", { ...limits, available: false, primary: null, secondary: null, plan: null });
    await openChat();
    await waitFor(() => expect(screen.getByTestId("codex-limits-meter").textContent).toContain("Unknown"));
    emit({ type: "error", message: "usage_limit_reached", authRequired: false, usageLimit: true, willRetry: false });
    expect(screen.getByTestId("codex-error").textContent).toContain("The ChatGPT plan limit is reached");
    expect(screen.getByTestId("codex-usage-banner")).toBeTruthy();
  });

  it("is in the palette as «Codex: open chat»", () => {
    const cmd = commands.find((c) => c.id === "codex.open");
    expect(cmd?.title).toBe("Codex: open chat");
    void cmd?.run({ projectId: P });
    expect(opened).toEqual(["codex"]);
  });
});

const engines: EnginesStatus = {
  claude: { engine: "claudeCode", installed: true, signedIn: true, version: "2.1.0", plan: "max", providerId: null, limitPercent: 85, limitResetsAt: null, runningAgents: 2, reason: null },
  codex: { engine: "codex", installed: false, signedIn: false, version: null, plan: null, providerId: null, limitPercent: null, limitResetsAt: null, runningAgents: 0, reason: "codex not found" },
  config: { mode: "off", manager: "claudeCode", primary: "claudeCode", codexRoles: ["Backend", "QA"], claudeModel: "sonnet", codexModel: "default", crossFallback: true },
};

describe("parallel engines", () => {
  beforeEach(() => {
    useAppStore.setState({ connection: { ...useAppStore.getState().connection, state: "open" } });
    useEnginesStore.setState({ status: null, loading: false, saving: false, error: null });
    responses.set("engines.status", engines);
  });

  it("shows both subscriptions and saves the configuration", async () => {
    responses.set("engines.setConfig", (cfg: unknown) => ({ ...engines, config: cfg }));
    render(<EnginesCard />);
    await screen.findByTestId("engines-status-claudeCode");
    expect(screen.getByTestId("engines-status-claudeCode").textContent).toContain("Limit almost used up");
    expect(screen.getByTestId("engines-status-codex").textContent).toContain("Not installed");
    const save = screen.getByTestId("engines-save") as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    fireEvent.click(within(screen.getByTestId("engines-mode-split")).getByRole("radio"));
    fireEvent.click(screen.getByLabelText("Security"));
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    await waitFor(() =>
      expect(calls.find((c) => c.method === "engines.setConfig")?.params).toMatchObject({ mode: "split", codexRoles: ["Backend", "QA", "Security"] }),
    );
    await screen.findByText("Saved");
  });

  it("title-bar indicator: amber Claude (limit > 80 %), grey Codex (not installed), click opens Codex", async () => {
    render(<EnginesIndicator />);
    const claude = await screen.findByTestId("engines-indicator-claude");
    await waitFor(() => expect(claude.querySelector(".eng-dot--warn")).toBeTruthy());
    expect(claude.getAttribute("title")).toContain("Plan: max");
    const codex = screen.getByTestId("engines-indicator-codex");
    expect(codex.querySelector(".eng-dot--off")).toBeTruthy();
    fireEvent.click(codex);
    expect(opened).toEqual(["codex"]);
  });

  it("agent engine badge follows the provider kind", () => {
    useProvidersStore.setState({
      providers: [
        { provider: { id: "pc", kind: "Codex" } },
        { provider: { id: "pa", kind: "ClaudeCode" } },
        { provider: { id: "po", kind: "OpenAi" } },
      ] as never,
    });
    render(
      <>
        <AgentEngineBadge providerId="pc" />
        <AgentEngineBadge providerId="pa" />
        <AgentEngineBadge providerId="po" />
        <AgentEngineBadge providerId="missing" />
      </>,
    );
    expect(screen.getAllByTestId("agent-engine-badge").map((b) => b.textContent)).toEqual(["Codex", "Claude Code", "API"]);
  });
});
