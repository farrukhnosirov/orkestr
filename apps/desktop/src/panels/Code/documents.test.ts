import { beforeEach, describe, expect, it, vi } from "vitest";

import { useEditorStore } from "../../store/editor";
import { registryFor } from "./documents";
import type { MonacoApi } from "./monaco";

vi.mock("./monaco", () => ({ modelUri: (projectId: string, path: string) => `orkestr://${projectId}/${path}` }));

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: () =>
      Promise.resolve({
        content: "one\n",
        binary: false,
        language: "typescript",
        lineEnding: "lf",
        hash: "h1",
        encoding: "utf8",
        readonly: false,
      }),
  }),
}));

/** Minimal text model: enough of Monaco's ITextModel for the registry. */
class FakeModel {
  private value: string;
  private version = 1;
  disposed = false;
  private readonly listeners: (() => void)[] = [];
  constructor(value: string) {
    this.value = value;
  }
  getValue() {
    return this.value;
  }
  setValue(v: string) {
    this.value = v;
    this.version += 1;
    for (const l of this.listeners) l();
  }
  getAlternativeVersionId() {
    return this.version;
  }
  onDidChangeContent(fn: () => void) {
    this.listeners.push(fn);
    return { dispose: () => undefined };
  }
  setEOL() {}
  dispose() {
    this.disposed = true;
  }
}

const fakeMonaco = {
  editor: {
    getModel: () => null,
    createModel: (content: string) => new FakeModel(content),
    EndOfLineSequence: { LF: 0, CRLF: 1 },
  },
} as unknown as MonacoApi;

const fake = (m: unknown) => m as FakeModel;

describe("document registries", () => {
  beforeEach(() => {
    useEditorStore.setState({ projectId: null, sessions: {}, tabs: [], activePath: null, agentModified: {} });
  });

  it("keeps unsaved edits of a project across a project switch", async () => {
    useEditorStore.getState().setProject("pa");
    useEditorStore.getState().openFile("a.ts");
    const a = registryFor(fakeMonaco, "pa");
    const doc = await a.load("a.ts");
    if (!doc) throw new Error("not loaded");
    fake(doc.model).setValue("edited\n");
    expect(useEditorStore.getState().tabs[0]?.dirty).toBe(true);

    useEditorStore.getState().setProject("pb");
    const b = registryFor(fakeMonaco, "pb");
    expect(b).not.toBe(a);
    expect(fake(doc.model).disposed).toBe(false);

    useEditorStore.getState().setProject("pa");
    const again = registryFor(fakeMonaco, "pa");
    expect(again).toBe(a);
    const back = again.get("a.ts");
    expect(back?.model.getValue()).toBe("edited\n");
    expect(back && again.isDirty(back)).toBe(true);
    expect(useEditorStore.getState().tabs.map((t) => [t.path, t.dirty])).toEqual([["a.ts", true]]);
  });

  it("a load finishing after a switch updates the parked project's tab only", async () => {
    useEditorStore.getState().setProject("pc");
    useEditorStore.getState().openFile("a.ts");
    const pending = registryFor(fakeMonaco, "pc").load("a.ts");
    useEditorStore.getState().setProject("pd");
    useEditorStore.getState().openFile("a.ts");
    await pending;
    expect(useEditorStore.getState().tabs[0]?.status).toBe("loading");
    expect(useEditorStore.getState().sessions.pc?.tabs[0]?.status).toBe("ready");
  });
});
