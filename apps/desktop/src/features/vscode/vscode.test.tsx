// VS Code panel: state rendering, consent before install, start → window,
// event handling and the pure helpers.
import type { Event, VsCodeStatus } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
const responses = new Map<string, unknown>();
const fakeClient = {
  call: (method: string, params: unknown) => {
    calls.push({ method, params });
    if (!responses.has(method)) return Promise.reject(new Error(`unexpected ${method}`));
    return Promise.resolve(responses.get(method));
  },
  subscribe: () => () => undefined,
};
vi.mock("../../core/client", () => ({
  getClient: () => fakeClient,
  maybeClient: () => fakeClient,
}));
const opened: { url: string; title: string }[] = [];
vi.mock("./windowControls", () => ({
  vscodeWindow: {
    open: (url: string, title: string) => {
      opened.push({ url, title });
      return Promise.resolve();
    },
    close: () => Promise.resolve(),
    reload: () => Promise.resolve(),
    isOpen: () => Promise.resolve(false),
    openExternal: () => Promise.resolve(),
    onChange: () => Promise.resolve(() => undefined),
  },
}));

import { i18n } from "../../i18n";
import VsCodePanel from "../../panels/VsCode";
import { useAppStore } from "../../store/app";
import { compareVersions, isServerActive } from "./logic";
import { useVsCodeStore } from "./store";

const status = (patch: Partial<VsCodeStatus> = {}): VsCodeStatus => ({
  state: "notInstalled",
  target: "win32-x64",
  unsupportedReason: null,
  installed: null,
  running: null,
  progress: null,
  error: null,
  dataDir: "C:/data/vscode-server",
  config: { autoStop: true, idleStopSecs: 120, autoRestart: true, pinnedCommands: [], embed: true, syncTheme: true, disabledExtensions: [], workspaceTrust: false },
  bridgeConnected: false,
  ...patch,
});

const installed = {
  version: "1.135.06055",
  assetName: "vscodium-reh-web-win32-x64-1.135.06055.tar.gz",
  sha256: "ab",
  sizeBytes: 129_574_089,
  installedAt: "2026-10-07T00:00:00Z",
  dir: "C:/data/vscode-server/1.135.06055",
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  opened.length = 0;
  responses.clear();
  useVsCodeStore.setState({ status: null, progress: null, release: null, releaseError: null, error: null, busy: null, logs: [], lastUrl: null, embedUrl: null });
  useAppStore.setState({
    connection: { ...useAppStore.getState().connection, state: "open" },
    projects: [{ id: "p1", name: "alpha", rootPath: "C:/code/alpha" } as never],
    activeProjectId: "p1",
  });
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("VS Code panel", () => {
  it("asks for consent before installing and shows size, license and registry", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.keepAlive", {});
    responses.set("vscode.checkUpdate", {
      version: "1.135.06055",
      assetName: installed.assetName,
      sizeBytes: 129_574_089,
      downloadUrl: "https://github.com/VSCodium/vscodium/releases/download/x",
      sha256Url: "https://github.com/VSCodium/vscodium/releases/download/x.sha256",
      publishedAt: null,
      releasePage: "https://github.com/VSCodium/vscodium/releases/tag/1.135.06055",
      license: "MIT",
      extensionRegistry: "https://open-vsx.org",
      updateAvailable: false,
    });
    responses.set("vscode.install", status({ state: "installing" }));
    render(<VsCodePanel projectId="p1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Install VS Code…" }));
    expect(await screen.findByText("MIT")).toBeTruthy();
    expect(screen.getByText("https://open-vsx.org")).toBeTruthy();
    expect(screen.getByText(/^\d[\d.,\s]* MB$/)).toBeTruthy();
    const confirm = screen.getByRole("button", { name: "Download and install" }) as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("I understand that extensions run as my user"));
    expect(confirm.disabled).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.install")).toBe(true));
    const install = calls.find((c) => c.method === "vscode.install");
    expect(install?.params).toEqual({ accept: true, version: "1.135.06055" });
  });

  it("starts the server for the project folder and opens the window (separate-window mode)", async () => {
    const windowMode = { ...status().config, embed: false };
    responses.set("vscode.status", status({ state: "stopped", installed, config: windowMode }));
    responses.set("vscode.keepAlive", {});
    responses.set("vscode.start", {
      url: "http://127.0.0.1:9900/?tkn=secret&folder=/c:/code/alpha",
      origin: "http://127.0.0.1:9900",
      folder: "/c:/code/alpha",
      status: status({
        state: "running",
        installed,
        config: windowMode,
        running: { port: 9900, pid: 1, startedAt: "2026-10-07T00:00:00Z", restarts: 0 },
      }),
    });
    render(<VsCodePanel projectId="p1" />);
    expect(await screen.findByText("Folder: C:/code/alpha")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open VS Code" }));
    await waitFor(() => expect(opened).toHaveLength(1));
    expect(calls.find((c) => c.method === "vscode.start")?.params).toEqual({ projectId: "p1" });
    expect(opened[0]?.title).toBe("VS Code — alpha");
    fireEvent.click(screen.getByRole("tab", { name: "Log" }));
    expect(await screen.findByText("Server: 127.0.0.1:9900")).toBeTruthy();
    expect(calls.some((c) => c.method === "vscode.keepAlive")).toBe(true);
  });

  it("follows status and progress events and shows errors with the log", async () => {
    responses.set("vscode.status", status({ state: "installing" }));
    responses.set("vscode.keepAlive", {});
    responses.set("vscode.logs", { lines: ["Orkestr: VS Code server exited: boom"] });
    render(<VsCodePanel projectId={null} />);
    await screen.findByText("Looking up the release…");
    const progress = { kind: "VsCodeProgress", payload: { progress: { phase: "downloading", done: 50, total: 100, version: "1" } } } as unknown as Event;
    act(() => useVsCodeStore.getState().applyEvent(progress));
    expect(await screen.findByText("50%")).toBeTruthy();
    const failed = { kind: "VsCodeStatusChanged", payload: { status: status({ state: "error", installed, error: "the server process exited (exit code: 7)" }) } } as unknown as Event;
    act(() => useVsCodeStore.getState().applyEvent(failed));
    expect(await screen.findByText("VS Code could not run")).toBeTruthy();
    expect(screen.getByText("the server process exited (exit code: 7)")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show log" }));
    expect(await screen.findByText(/exited: boom/)).toBeTruthy();
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    responses.set("vscode.status", status());
    responses.set("vscode.keepAlive", {});
    render(<VsCodePanel projectId="p1" />);
    expect(await screen.findByRole("button", { name: "Установить VS Code…" })).toBeTruthy();
    expect(screen.getByText("Не установлен")).toBeTruthy();
  });

  it("explains unsupported hosts", async () => {
    responses.set("vscode.status", status({ target: null, unsupportedReason: "VS Code server builds are not available for Other/X64" }));
    responses.set("vscode.keepAlive", {});
    render(<VsCodePanel projectId={null} />);
    expect(await screen.findByText(/not available on this computer/)).toBeTruthy();
  });
});

describe("helpers", () => {
  it("compares versions numerically", () => {
    expect(compareVersions("1.135.06055", "1.99.1")).toBe(1);
    expect(compareVersions("1.2", "1.2.0")).toBe(0);
    expect(compareVersions("1.2.3", "1.10")).toBe(-1);
  });
  it("knows which states have a server process", () => {
    expect(isServerActive("running")).toBe(true);
    expect(isServerActive("restarting")).toBe(true);
    expect(isServerActive("stopped")).toBe(false);
  });
});
