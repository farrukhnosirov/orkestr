// VS Code extension launcher: cards with command buttons, run through the
// bridge, confirmation of dangerous commands, pins, palette rows.
import { OrkestrError } from "@orkestr/client";
import type { VsCodeExtensionCommands, VsCodeStatus } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
const handlers = new Map<string, (params: unknown) => unknown>();
const fakeClient = {
  call: (method: string, params: unknown) => {
    calls.push({ method, params });
    const h = handlers.get(method);
    if (!h) return Promise.reject(new Error(`unexpected ${method}`));
    try {
      return Promise.resolve(h(params));
    } catch (e) {
      return Promise.reject(e);
    }
  },
  subscribe: () => () => undefined,
};
vi.mock("../../core/client", () => ({
  getClient: () => fakeClient,
  maybeClient: () => fakeClient,
}));
const opened: string[] = [];
vi.mock("./windowControls", () => ({
  vscodeWindow: {
    open: (url: string) => {
      opened.push(url);
      return Promise.resolve();
    },
    close: () => Promise.resolve(),
    reload: () => Promise.resolve(),
    isOpen: () => Promise.resolve(false),
    openExternal: () => Promise.resolve(),
    onChange: () => Promise.resolve(() => undefined),
  },
}));

import { vscodeCommandItems } from "../../components/CommandPalette";
import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { ExtensionLauncher, LauncherHost } from "./ExtensionLauncher";
import { commandMatches, useLauncherStore, vscodePaletteQuery } from "./launcher";
import { commandMonogram, PinnedQuickActions } from "./PinnedCommands";
import { useVsCodeStore } from "./store";

const installed = {
  version: "1.135.06055",
  assetName: "x.tar.gz",
  sha256: "ab",
  sizeBytes: 1,
  installedAt: "2026-10-07T00:00:00Z",
  dir: "C:/data/vscode-server/1.135.06055",
};

const status = (patch: Partial<VsCodeStatus> = {}): VsCodeStatus => ({
  state: "running",
  target: "win32-x64",
  unsupportedReason: null,
  installed,
  running: null,
  progress: null,
  error: null,
  dataDir: "C:/data/vscode-server",
  config: { autoStop: true, idleStopSecs: 120, autoRestart: true, pinnedCommands: [] },
  bridgeConnected: true,
  ...patch,
});

const rainbow: VsCodeExtensionCommands = {
  id: "mechatroner.rainbow-csv",
  displayName: "Rainbow CSV",
  description: "Highlight CSV and TSV files",
  version: "3.6.0",
  publisher: "mechatroner",
  icon: null,
  categories: ["Formatters"],
  commands: [
    { id: "rainbow-csv.Align", title: "Align columns", category: "Rainbow CSV", hidden: false, runtimeOnly: false, dangerous: false, keybinding: "ctrl+alt+a" },
    { id: "rainbow-csv.DeleteCache", title: "Delete cache", category: "Rainbow CSV", hidden: false, runtimeOnly: false, dangerous: true, keybinding: null },
    { id: "rainbow-csv.secretToggle", title: "Secret Toggle", category: null, hidden: true, runtimeOnly: true, dangerous: false, keybinding: null },
  ],
  viewContainers: [{ id: "rainbow", title: "Rainbow", location: "activitybar", command: "workbench.view.extension.rainbow" }],
  hasSettings: true,
  dir: "C:/x",
};
const claude: VsCodeExtensionCommands = {
  ...rainbow,
  id: "anthropic.claude-code",
  displayName: "Claude Code",
  description: "Claude Code for VS Code",
  publisher: "anthropic",
  commands: [{ id: "claude-vscode.editor.open", title: "Open in New Tab", category: "Claude Code", hidden: false, runtimeOnly: false, dangerous: false, keybinding: null }],
  viewContainers: [{ id: "claude-sidebar", title: "Claude Code", location: "activitybar", command: "workbench.view.extension.claude-sidebar" }],
  hasSettings: false,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  opened.length = 0;
  handlers.clear();
  handlers.set("vscode.commands", () => ({ extensions: [claude, rainbow], bridgeConnected: true, workspaceTrusted: true }));
  handlers.set("vscode.execute", () => ({ result: null, durationMs: 3 }));
  handlers.set("vscode.openView", () => ({ result: null, durationMs: 3 }));
  handlers.set("vscode.status", () => status());
  useLauncherStore.setState({ extensions: null, workspaceTrusted: null, loading: false, error: null, running: null, opening: false, confirm: null, last: null });
  useVsCodeStore.setState({ status: status(), error: null, busy: null });
  useAppStore.setState({
    connection: { ...useAppStore.getState().connection, state: "open" },
    projects: [{ id: "p1", name: "alpha", rootPath: "C:/code/alpha" } as never],
    activeProjectId: "p1",
  });
});

afterEach(() => {
  cleanup();
});

describe("extension launcher cards", () => {
  it("shows a card per extension with its commands as buttons and runs one", async () => {
    render(<ExtensionLauncher />);
    const card = (await screen.findByText("Rainbow CSV", { selector: "h5" })).closest("article") as HTMLElement;
    expect(within(card).getByText("Highlight CSV and TSV files")).toBeTruthy();
    expect(within(card).getByText("Align columns")).toBeTruthy();
    expect(within(card).getByText("ctrl+alt+a")).toBeTruthy();
    expect(within(card).getByText("Hidden")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Extension settings" })).toBeTruthy();
    expect(screen.getByText("VS Code connected")).toBeTruthy();
    expect(calls.find((c) => c.method === "vscode.commands")?.params).toEqual({ locale: "en" });

    fireEvent.click(within(card).getByRole("button", { name: "Run «Rainbow CSV: Align columns»" }));
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.execute")).toBe(true));
    expect(calls.find((c) => c.method === "vscode.execute")?.params).toEqual({ commandId: "rainbow-csv.Align", waitMs: 60_000 });
    expect(opened).toHaveLength(0);
  });

  it("opens the extension's view container and its settings", async () => {
    render(<ExtensionLauncher />);
    const card = (await screen.findByText("Claude Code", { selector: "h5" })).closest("article") as HTMLElement;
    fireEvent.click(within(card).getByRole("button", { name: "Open extension panel" }));
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.openView")).toBe(true));
    expect(calls.find((c) => c.method === "vscode.openView")?.params).toEqual({ extensionId: "anthropic.claude-code", waitMs: 60_000 });
    const rainbowCard = screen.getByText("Rainbow CSV", { selector: "h5" }).closest("article") as HTMLElement;
    await waitFor(() => expect(useLauncherStore.getState().running).toBeNull());
    fireEvent.click(within(rainbowCard).getByRole("button", { name: "Extension settings" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "vscode.openView")).toHaveLength(2));
    expect(calls.filter((c) => c.method === "vscode.openView")[1]?.params).toEqual({
      extensionId: "mechatroner.rainbow-csv",
      waitMs: 60_000,
      settings: true,
    });
  });

  it("warns about Restricted Mode and offers to trust the folder", async () => {
    handlers.set("vscode.commands", () => ({ extensions: [rainbow], bridgeConnected: true, workspaceTrusted: false }));
    render(<ExtensionLauncher />);
    expect(await screen.findByText("VS Code is in Restricted Mode.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Trust folder…" }));
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.execute")).toBe(true));
    expect(calls.find((c) => c.method === "vscode.execute")?.params).toMatchObject({ commandId: "workbench.trust.manage" });
  });

  it("filters commands by search across extensions", async () => {
    render(<ExtensionLauncher />);
    await screen.findByText("Align columns");
    fireEvent.change(screen.getByLabelText("Search extension commands"), { target: { value: "new tab" } });
    expect(screen.getByText("Open in New Tab")).toBeTruthy();
    expect(screen.queryByText("Align columns")).toBeNull();
    fireEvent.change(screen.getByLabelText("Search extension commands"), { target: { value: "zzz" } });
    expect(screen.getByText("No commands match «zzz».")).toBeTruthy();
  });

  it("asks for confirmation when the core flags a command as dangerous", async () => {
    let n = 0;
    handlers.set("vscode.execute", (p) => {
      n += 1;
      if (!(p as { confirmed?: boolean }).confirmed) throw new OrkestrError("approval_required", "looks destructive");
      return { result: null, durationMs: 1 };
    });
    render(
      <>
        <ExtensionLauncher />
        <LauncherHost />
      </>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Run «Rainbow CSV: Delete cache»" }));
    expect(await screen.findByText("Run «Delete cache»?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await waitFor(() => expect(n).toBe(2));
    expect(calls.filter((c) => c.method === "vscode.execute")[1]?.params).toMatchObject({ commandId: "rainbow-csv.DeleteCache", confirmed: true });
    expect(await screen.findByText("«Delete cache» ran in VS Code")).toBeTruthy();
  });

  it("opens the VS Code window first when no window is connected", async () => {
    useVsCodeStore.setState({ status: status({ bridgeConnected: false }) });
    handlers.set("vscode.status", () => status({ bridgeConnected: false }));
    handlers.set("vscode.keepAlive", () => ({}));
    handlers.set("vscode.start", () => ({ url: "http://127.0.0.1:1/?tkn=x", origin: "http://127.0.0.1:1", folder: null, status: status() }));
    await useLauncherStore.getState().launch({ kind: "command", commandId: "rainbow-csv.Align", title: "Align columns" });
    expect(opened).toHaveLength(1);
    expect(calls.find((c) => c.method === "vscode.start")?.params).toEqual({ projectId: "p1" });
    expect(calls.find((c) => c.method === "vscode.execute")?.params).toMatchObject({ waitMs: 60_000 });
  });

  it("pins a command through the core config and shows it as a quick action", async () => {
    handlers.set("vscode.setConfig", (p) => status({ config: { ...status().config, pinnedCommands: (p as { pinnedCommands: string[] }).pinnedCommands } }));
    render(<ExtensionLauncher />);
    await screen.findByText("Align columns");
    fireEvent.click(screen.getAllByRole("button", { name: "Pin to the left rail and Mission Control" })[1] as HTMLElement);
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.setConfig")).toBe(true));
    expect(calls.find((c) => c.method === "vscode.setConfig")?.params).toEqual({ pinnedCommands: ["rainbow-csv.Align"] });
    cleanup();
    render(<PinnedQuickActions />);
    expect(await screen.findByRole("button", { name: /Rainbow CSV: Align columns/ })).toBeTruthy();
  });
});

describe("launcher helpers", () => {
  it("detects the VS Code palette prefix", () => {
    expect(vscodePaletteQuery("VS Code: align")).toBe(" align");
    expect(vscodePaletteQuery("vscode:")).toBe("");
    expect(vscodePaletteQuery("open panel")).toBeNull();
  });

  it("maps commands to palette rows grouped by extension", () => {
    const items = vscodeCommandItems([rainbow]);
    expect(items).toHaveLength(3);
    expect(items[0]).toMatchObject({ id: "vscode-cmd:rainbow-csv.Align", title: "Rainbow CSV: Align columns", group: "VS Code: Rainbow CSV", hint: "ctrl+alt+a" });
  });

  it("matches search terms against title, category, id and extension", () => {
    const cmd = rainbow.commands[0]!;
    expect(commandMatches(rainbow, cmd, "align rainbow")).toBe(true);
    expect(commandMatches(rainbow, cmd, "rainbow-csv.Align")).toBe(true);
    expect(commandMatches(rainbow, cmd, "claude")).toBe(false);
    expect(commandMonogram("Open in New Tab")).toBe("OI");
  });
});
