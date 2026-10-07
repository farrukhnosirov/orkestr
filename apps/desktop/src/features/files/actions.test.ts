/**
 * Panel intents across windows: opening a file / asking the Manager reaches
 * the panel even when it lives in a detached window (separate webview with
 * its own stores).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { registerWorkspace } from "../../components/workspaceActions";
import { useEditorStore } from "../../store/editor";
import { ASK_MANAGER_EVENT, askManager, hostPanelIntents, openInEditor, type PanelIntent } from "./actions";

/** In-process stand-in for the same-origin channel between windows. */
class FakeChannel {
  static open: FakeChannel[] = [];
  onmessage: ((e: MessageEvent) => void) | null = null;
  constructor(public name: string) {
    FakeChannel.open.push(this);
  }
  postMessage(data: unknown) {
    const copy = structuredClone(data);
    for (const other of FakeChannel.open) {
      if (other !== this && other.name === this.name) other.onmessage?.({ data: copy } as MessageEvent);
    }
  }
  close() {
    FakeChannel.open = FakeChannel.open.filter((c) => c !== this);
  }
}

/** A raw channel standing in for "the other window". */
function otherWindow(): { received: unknown[]; send: (intent: PanelIntent) => void; close: () => void } {
  const ch = new FakeChannel("orkestr.panel-intents");
  const received: unknown[] = [];
  ch.onmessage = (e) => received.push(e.data);
  return { received, send: (intent) => ch.postMessage(intent), close: () => ch.close() };
}

describe("panel intents", () => {
  const original = globalThis.BroadcastChannel;
  const openPanel = vi.fn();
  let dispose: (() => void) | null = null;

  beforeEach(() => {
    FakeChannel.open = [];
    (globalThis as { BroadcastChannel: unknown }).BroadcastChannel = FakeChannel;
    openPanel.mockReset();
    registerWorkspace({ openPanel, resetLayout: () => undefined, toggleMaximize: () => undefined });
    useEditorStore.setState({ tabs: [], activePath: null });
  });
  afterEach(() => {
    dispose?.();
    dispose = null;
    registerWorkspace(null);
    (globalThis as { BroadcastChannel: unknown }).BroadcastChannel = original;
  });

  it("opens locally when this window hosts the Code panel", () => {
    dispose = hostPanelIntents(() => true);
    const other = otherWindow();
    openInEditor("src/a.ts", { line: 3 });
    expect(useEditorStore.getState().activePath).toBe("src/a.ts");
    expect(other.received).toEqual([]);
    expect(openPanel).toHaveBeenCalledWith("code");
    other.close();
  });

  it("forwards to the detached Code window instead of the local store", () => {
    // Main window with Code popped out.
    dispose = hostPanelIntents((id) => id !== "code");
    const other = otherWindow();
    openInEditor("src/b.ts", { line: 12, column: 4 });
    expect(useEditorStore.getState().activePath).toBeNull();
    expect(other.received).toEqual([{ type: "openFile", path: "src/b.ts", at: { line: 12, column: 4 } }]);
    // The main window still focuses the detached window.
    expect(openPanel).toHaveBeenCalledWith("code");
    other.close();
  });

  it("the window hosting the panel applies forwarded intents; others ignore them", () => {
    // Detached Code window.
    dispose = hostPanelIntents((id) => id === "code");
    const other = otherWindow();
    other.send({ type: "openFile", path: "src/c.ts", at: { line: 7 } });
    expect(useEditorStore.getState().activePath).toBe("src/c.ts");

    const asked = vi.fn();
    window.addEventListener(ASK_MANAGER_EVENT, asked);
    other.send({ type: "askManager", detail: { text: "why?", projectId: "p1" } });
    expect(asked).not.toHaveBeenCalled(); // the Manager is not in this window
    window.removeEventListener(ASK_MANAGER_EVENT, asked);
    other.close();
  });

  it("ask-manager from a detached Code window reaches the main window's Manager", () => {
    // This test process plays the main window (hosts the Manager).
    dispose = hostPanelIntents((id) => id !== "code");
    const asked = vi.fn();
    window.addEventListener(ASK_MANAGER_EVENT, asked);
    const detachedCode = otherWindow();
    detachedCode.send({ type: "askManager", detail: { text: "explain", projectId: "p1", path: "x.ts", startLine: 1, endLine: 2 } });
    expect(asked).toHaveBeenCalledTimes(1);
    expect((asked.mock.calls[0]?.[0] as CustomEvent).detail).toMatchObject({ text: "explain", path: "x.ts" });
    expect(openPanel).toHaveBeenCalledWith("manager");
    window.removeEventListener(ASK_MANAGER_EVENT, asked);
    detachedCode.close();
  });

  it("a detached window forwards ask-manager instead of dispatching it to itself", () => {
    dispose = hostPanelIntents((id) => id === "code");
    const asked = vi.fn();
    window.addEventListener(ASK_MANAGER_EVENT, asked);
    const main = otherWindow();
    askManager({ text: "hi", projectId: null });
    expect(asked).not.toHaveBeenCalled();
    expect(main.received).toEqual([{ type: "askManager", detail: { text: "hi", projectId: null } }]);
    window.removeEventListener(ASK_MANAGER_EVENT, asked);
    main.close();
  });

  it("without a declared host everything stays local (HUD, tests)", () => {
    openInEditor("src/d.ts");
    expect(useEditorStore.getState().activePath).toBe("src/d.ts");
  });
});
