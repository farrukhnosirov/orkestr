// Quick access to VS Code: Ctrl+Shift+X entry point (native panel hook +
// VS Code fallback), File menu entry, palette commands, extension installs.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
const responses = new Map<string, unknown>();
const fakeClient = {
  call: (method: string, params: unknown, _opts?: unknown) => {
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
import { shortcutCommand } from "../commands/dispatch";
import { commands } from "../commands/registry";
import { FILE_MENU } from "../workbench/FileMenu";
import { EXTENSIONS_SHORTCUT, hasNativeExtensionsPanel, NATIVE_EXTENSIONS_PANEL_ID, openExtensions } from "./quickAccess";
import { useVsCodeStore } from "./store";

const opened: string[] = [];

beforeEach(() => {
  opened.length = 0;
  calls.length = 0;
  responses.clear();
  registerWorkspace({ openPanel: (id) => opened.push(id), resetLayout: () => undefined, toggleMaximize: () => undefined });
  useVsCodeStore.setState({ view: "main", viewNonce: 0, extensions: null, extensionsError: null, installingExtension: null });
});
afterEach(() => registerWorkspace(null));

describe("extensions entry point", () => {
  it("Ctrl+Shift+X runs extensions.open (also on the Russian layout)", () => {
    expect(EXTENSIONS_SHORTCUT).toBe("Ctrl+Shift+X");
    const ev = { key: "Ч", code: "KeyX", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false, target: null };
    expect(shortcutCommand(ev, commands, { projectId: null })?.id).toBe("extensions.open");
  });

  it("opens the native Extensions panel", () => {
    expect(NATIVE_EXTENSIONS_PANEL_ID).toBe("extensions");
    expect(hasNativeExtensionsPanel()).toBe(true);
    openExtensions();
    expect(opened).toEqual(["extensions"]);
  });

  it("'Install extension…' opens the VS Code panel's Extensions section", () => {
    commands.find((c) => c.id === "vscode.installExtension")?.run({ projectId: null });
    expect(opened).toEqual(["vscode"]);
    expect(useVsCodeStore.getState().view).toBe("extensions");
    expect(useVsCodeStore.getState().viewNonce).toBe(1);
  });

  it("the File menu and the palette expose VS Code", () => {
    expect(FILE_MENU.some((e) => e.kind === "cmd" && e.command === "vscode.openProject")).toBe(true);
    for (const id of ["vscode.open", "vscode.openProject", "vscode.installExtension", "extensions.open"]) {
      const c = commands.find((x) => x.id === id);
      expect(c, id).toBeDefined();
      expect(c?.keywords?.some((k) => /[a-z]/.test(k)), id).toBe(true);
      expect(c?.keywords?.some((k) => /[а-я]/.test(k)), id).toBe(true);
    }
  });

  it("install extension sends the id and stores the returned list", async () => {
    responses.set("vscode.installExtension", { extensions: [{ id: "anthropic.claude-code", version: "2.0.0" }] });
    expect(await useVsCodeStore.getState().installExtension("  anthropic.claude-code ")).toBe(true);
    expect(calls).toEqual([{ method: "vscode.installExtension", params: { id: "anthropic.claude-code" } }]);
    expect(useVsCodeStore.getState().extensions?.[0]?.id).toBe("anthropic.claude-code");
    expect(useVsCodeStore.getState().installingExtension).toBeNull();
  });

  it("a failed install is reported, not swallowed", async () => {
    expect(await useVsCodeStore.getState().installExtension("bad")).toBe(false);
    expect(useVsCodeStore.getState().extensionsError).toBeTruthy();
  });
});
