// VS Code inside the Orkestr window (ADR-0043): frame URL rewriting, theme
// mapping, the embedded panel (auto-start, sandboxed frame, bridge signals,
// theme sync), the extension manager and "Open in VS Code".
import type { Event, VsCodeExtension, VsCodeStatus } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
const responses = new Map<string, unknown>();
const fakeClient = {
  call: (method: string, params: unknown) => {
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
import {
  EMBED_SANDBOX,
  embedHostFor,
  embedUrlFor,
  isLightScheme,
  over,
  parseColor,
  SYNCED_COLOR_IDS,
  toHex,
  workbenchColors,
} from "./embed";
import { ExtensionsManager, visibleExtensions } from "./ExtensionsManager";
import { useLauncherStore } from "./launcher";
import { absolutePath, openFileInVsCode } from "./quickAccess";
import { useVsCodeStore, VSCODE_FOCUS_OUT_EVENT } from "./store";

const installed = {
  version: "1.140.06880",
  assetName: "vscodium-reh-web-win32-x64-1.140.06880.tar.gz",
  sha256: "ab",
  sizeBytes: 1,
  installedAt: "2026-10-20T00:00:00Z",
  dir: "C:/data/vscode-server/1.140.06880",
};
const running = { port: 9900, pid: 1, startedAt: "2026-10-23T00:00:00Z", restarts: 0 };

const status = (patch: Partial<VsCodeStatus> = {}): VsCodeStatus => ({
  state: "stopped",
  target: "win32-x64",
  unsupportedReason: null,
  installed,
  running: null,
  progress: null,
  error: null,
  dataDir: "C:/data/vscode-server",
  config: { autoStop: true, idleStopSecs: 120, autoRestart: true, pinnedCommands: [], embed: true, syncTheme: true, disabledExtensions: [], workspaceTrust: false },
  bridgeConnected: false,
  ...patch,
});

const startResult = (patch: Partial<VsCodeStatus> = {}) => ({
  url: "http://127.0.0.1:9900/?tkn=secret&folder=/c:/code/alpha",
  origin: "http://127.0.0.1:9900",
  folder: "/c:/code/alpha",
  status: status({ state: "running", running, ...patch }),
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  opened.length = 0;
  responses.clear();
  responses.set("vscode.keepAlive", {});
  useVsCodeStore.setState({
    status: null,
    error: null,
    busy: null,
    lastUrl: null,
    embedUrl: null,
    embedProjectId: null,
    embedKey: 0,
    embedUnsupported: false,
    extensions: null,
    extensionsError: null,
    managingExtension: null,
    installingExtension: null,
  });
  useLauncherStore.setState({ extensions: null, error: null });
  useAppStore.setState({
    connection: { ...useAppStore.getState().connection, state: "open" },
    projects: [{ id: "p1", name: "alpha", rootPath: "C:/code/alpha" } as never],
    activeProjectId: "p1",
  });
});

afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute("style");
});

describe("frame URL", () => {
  it("uses a same-site vscode.* host for the packaged app and the loopback in dev", () => {
    expect(embedHostFor("tauri.localhost")).toBe("vscode.tauri.localhost");
    expect(embedHostFor("127.0.0.1")).toBe("127.0.0.1");
    expect(embedHostFor("localhost")).toBe("localhost");
    expect(embedHostFor("orkestr.example.com")).toBeNull();
    expect(embedHostFor("evil.localhost/../x")).toBeNull();
  });

  it("rewrites only loopback http server URLs and keeps token and folder", () => {
    const url = "http://127.0.0.1:9900/?tkn=secret&folder=/c:/code/alpha";
    expect(embedUrlFor(url, "tauri.localhost")).toBe("http://vscode.tauri.localhost:9900/?tkn=secret&folder=/c:/code/alpha");
    expect(embedUrlFor(url, "app.example.com")).toBeNull();
    expect(embedUrlFor("https://127.0.0.1:9900/", "tauri.localhost")).toBeNull();
    expect(embedUrlFor("http://10.0.0.5:9900/", "tauri.localhost")).toBeNull();
    expect(embedUrlFor("not a url", "tauri.localhost")).toBeNull();
  });

  it("never lets the frame navigate the Orkestr window", () => {
    expect(EMBED_SANDBOX).toContain("allow-scripts");
    expect(EMBED_SANDBOX).not.toContain("allow-top-navigation");
  });
});

describe("theme mapping", () => {
  it("parses hex and rgb(a) colors", () => {
    expect(parseColor("#abc")).toEqual({ r: 170, g: 187, b: 204, a: 1 });
    expect(toHex(parseColor("rgba(255, 0, 0, 0.5)")!)).toBe("#ff000080");
    expect(toHex(parseColor("rgb(1 2 3 / 50%)")!)).toBe("#01020380");
    expect(parseColor("var(--x)")).toBeNull();
    expect(toHex(over({ r: 255, g: 255, b: 255, a: 0.5 }, { r: 0, g: 0, b: 0, a: 1 }))).toBe("#808080");
  });

  it("maps tokens to solid workbench backgrounds and keeps translucent highlights", () => {
    const tokens: Record<string, string> = {
      bg: "#05070f",
      "bg-elevated": "#0a0d1c",
      surface: "rgba(255, 255, 255, 0.04)",
      "surface-hover": "rgba(70, 90, 170, 0.16)",
      "surface-active": "rgba(38, 50, 100, 0.5)",
      text: "#e8ecff",
      "text-muted": "#9aa3c8",
      accent: "#7c8cff",
      "accent-soft": "rgba(124, 140, 255, 0.2)",
      border: "rgba(136, 156, 255, 0.12)",
    };
    const colors = workbenchColors((t) => tokens[t] ?? null);
    expect(colors["editor.background"]).toBe("#0a0d1c");
    expect(colors["sideBar.background"]).toBe("#05070f");
    expect(colors["list.hoverBackground"]).toMatch(/^#[0-9a-f]{6}$/); // composited over bg
    expect(colors["editor.selectionBackground"]).toBe("#7c8cff33"); // highlight keeps alpha
    expect(colors["button.background"]).toBe("#7c8cff");
    expect(colors["button.foreground"]).toBeUndefined(); // text-inverse missing → base theme
    expect(Object.keys(colors).every((k) => SYNCED_COLOR_IDS.includes(k))).toBe(true);
  });

  it("detects light schemes by the preferred keyword", () => {
    expect(isLightScheme("light")).toBe(true);
    expect(isLightScheme("light dark")).toBe(true);
    expect(isLightScheme("dark")).toBe(false);
    expect(isLightScheme("")).toBe(false);
  });
});

describe("embedded VS Code panel", () => {
  it("starts on open and shows the workbench in a sandboxed same-site frame", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    render(<VsCodePanel projectId="p1" />);
    const frame = (await screen.findByTitle("VS Code — alpha")) as HTMLIFrameElement;
    expect(calls.find((c) => c.method === "vscode.start")?.params).toEqual({ projectId: "p1" });
    // jsdom serves the page from `localhost`: the frame host follows the page.
    expect(frame.getAttribute("src")).toBe("http://localhost:9900/?tkn=secret&folder=/c:/code/alpha");
    expect(frame.getAttribute("sandbox")).toBe(EMBED_SANDBOX);
    expect(frame.getAttribute("allow")).toContain("clipboard-read");
    expect(opened).toHaveLength(0);
    // Exactly one start: the frame is not reloaded by status updates.
    act(() =>
      useVsCodeStore.getState().applyEvent({
        kind: "VsCodeStatusChanged",
        payload: { status: status({ state: "running", running, bridgeConnected: true }) },
      } as unknown as Event),
    );
    expect(calls.filter((c) => c.method === "vscode.start")).toHaveLength(1);
  });

  it("keeps the frame mounted while other tabs are open", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    responses.set("vscode.listExtensions", { extensions: [] });
    responses.set("vscode.commands", { extensions: [], bridgeConnected: false, workspaceTrusted: null });
    render(<VsCodePanel projectId="p1" />);
    await screen.findByTitle("VS Code — alpha");
    fireEvent.click(screen.getByRole("tab", { name: "Extensions" }));
    expect(await screen.findByText("Installed extensions (0)")).toBeTruthy();
    expect(screen.getByTitle("VS Code — alpha")).toBeTruthy();
  });

  it("does not restart after the user stops it", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    responses.set("vscode.stop", status({ state: "stopped" }));
    render(<VsCodePanel projectId="p1" />);
    await screen.findByTitle("VS Code — alpha");
    act(() => {
      useVsCodeStore.setState({ status: status({ state: "running", running }) });
    });
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.stop")).toBe(true));
    expect(await screen.findByRole("button", { name: "Start VS Code" })).toBeTruthy();
    expect(calls.filter((c) => c.method === "vscode.start")).toHaveLength(1);
  });

  it("reloads the frame after a crash restart", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    render(<VsCodePanel projectId="p1" />);
    await screen.findByTitle("VS Code — alpha");
    act(() => useVsCodeStore.setState({ status: status({ state: "running", running }) }));
    const key = useVsCodeStore.getState().embedKey;
    act(() =>
      useVsCodeStore.setState({ status: status({ state: "running", running: { ...running, startedAt: "2026-10-23T00:05:00Z", restarts: 1 } }) }),
    );
    await waitFor(() => expect(useVsCodeStore.getState().embedKey).toBe(key + 1));
  });

  it("pushes the Orkestr theme once the bridge is connected", async () => {
    document.documentElement.style.setProperty("--ok-color-bg", "#05070f");
    document.documentElement.style.setProperty("--ok-color-accent", "#7c8cff");
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult({ bridgeConnected: true }));
    responses.set("vscode.syncTheme", {});
    render(<VsCodePanel projectId="p1" />);
    await screen.findByTitle("VS Code — alpha");
    await waitFor(() => expect(calls.some((c) => c.method === "vscode.syncTheme")).toBe(true), { timeout: 2000 });
    const params = calls.find((c) => c.method === "vscode.syncTheme")?.params as { kind: string; colors: Record<string, string> };
    expect(params.kind).toBe("dark");
    expect(params.colors["sideBar.background"]).toBe("#05070f");
    expect(params.colors.focusBorder).toBe("#7c8cff");
  });

  it("opens the Orkestr palette when VS Code sends the palette signal", async () => {
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    render(<VsCodePanel projectId="p1" />);
    await screen.findByTitle("VS Code — alpha");
    act(() => useVsCodeStore.getState().applyEvent({ kind: "VsCodeBridgeSignal", payload: { signal: "showPalette" } } as unknown as Event));
    expect(useAppStore.getState().paletteOpen).toBe(true);
    useAppStore.getState().setPaletteOpen(false);
    // focusOrkestr is a plain window event the frame host listens to.
    const seen = vi.fn();
    window.addEventListener(VSCODE_FOCUS_OUT_EVENT, seen);
    act(() => useVsCodeStore.getState().applyEvent({ kind: "VsCodeBridgeSignal", payload: { signal: "focusOrkestr" } } as unknown as Event));
    window.removeEventListener(VSCODE_FOCUS_OUT_EVENT, seen);
    expect(seen).toHaveBeenCalledOnce();
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    responses.set("vscode.status", status());
    responses.set("vscode.start", startResult());
    render(<VsCodePanel projectId="p1" />);
    expect(await screen.findByRole("tab", { name: "Редактор" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Расширения" })).toBeTruthy();
    await i18n.changeLanguage("en");
  });
});

const ext = (id: string, patch: Partial<VsCodeExtension> = {}): VsCodeExtension => ({
  id,
  version: "1.0.0",
  disabled: false,
  restartRequired: false,
  ...patch,
});

describe("extension manager", () => {
  it("hides the bridge and lists disabled extensions last", () => {
    const list = visibleExtensions([ext("b.two", { disabled: true }), ext("orkestr.orkestr-bridge"), ext("a.one")]);
    expect(list.map((e) => e.id)).toEqual(["a.one", "b.two"]);
  });

  it("disables, asks to restart and uninstalls after confirmation", async () => {
    useVsCodeStore.setState({ status: status({ state: "running", running }) });
    responses.set("vscode.listExtensions", { extensions: [ext("redhat.vscode-yaml"), ext("orkestr.orkestr-bridge")] });
    responses.set("vscode.commands", {
      extensions: [
        {
          id: "redhat.vscode-yaml",
          displayName: "YAML",
          description: "YAML Language Support",
          version: "1.0.0",
          publisher: "redhat",
          icon: null,
          categories: [],
          commands: [],
          viewContainers: [],
          hasSettings: true,
          dir: "x",
        },
      ],
      bridgeConnected: true,
      workspaceTrusted: true,
    });
    responses.set("vscode.setExtensionEnabled", { extensions: [ext("redhat.vscode-yaml", { disabled: true, restartRequired: true })] });
    responses.set("vscode.uninstallExtension", { extensions: [] });
    render(<ExtensionsManager projectId="p1" installed />);
    expect(await screen.findByText("YAML")).toBeTruthy();
    expect(screen.queryByText("orkestr.orkestr-bridge")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "vscode.setExtensionEnabled")?.params).toEqual({ id: "redhat.vscode-yaml", enabled: false }),
    );
    expect(await screen.findByText("Restart needed")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Restart VS Code" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(calls.find((c) => c.method === "vscode.uninstallExtension")?.params).toEqual({ id: "redhat.vscode-yaml" }));
    expect(await screen.findByText("No extensions installed yet.")).toBeTruthy();
  });
});

describe("open in VS Code", () => {
  it("resolves project-relative paths", () => {
    expect(absolutePath("src/main.rs", "C:\\code\\alpha")).toBe("C:\\code\\alpha\\src\\main.rs");
    expect(absolutePath("src/main.rs", "/home/u/alpha/")).toBe("/home/u/alpha/src/main.rs");
    expect(absolutePath("D:/x/y.ts", null)).toBe("D:/x/y.ts");
    expect(absolutePath("a.ts", null)).toBeNull();
  });

  it("reveals the line in the connected workbench", async () => {
    useVsCodeStore.setState({
      status: status({ state: "running", running, bridgeConnected: true }),
      embedUrl: "http://localhost:9900/?tkn=x",
      embedProjectId: "p1",
    });
    responses.set("vscode.openFile", { result: null, durationMs: 3 });
    const err = await openFileInVsCode({ projectId: "p1", path: "src/lib.rs", line: 42, column: 7 });
    expect(err).toBeNull();
    expect(calls.find((c) => c.method === "vscode.openFile")?.params).toMatchObject({ path: "C:/code/alpha/src/lib.rs", line: 42, column: 7 });
    expect(calls.some((c) => c.method === "vscode.start")).toBe(false);
  });
});
