// The Code panel's document registry keeps unsaved Monaco models across a
// project switch (audit r1: switching project silently discarded edits).
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./monaco", () => ({ modelUri: (projectId: string, path: string) => `orkestr://${projectId}/${path}` }));
const disk = vi.hoisted(() => ({ hash: "h" }));
vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (_method: string, params: { path: string }) =>
      Promise.resolve({
        content: `disk:${params.path}`,
        binary: false,
        language: "plaintext",
        lineEnding: "lf",
        hash: disk.hash,
        encoding: "utf8",
        readonly: false,
      }),
  }),
}));

import { useEditorStore } from "../../store/editor";
import { pruneParkedDocuments, registryFor } from "./documents";
import type { MonacoApi } from "./monaco";

class FakeModel {
  version = 1;
  disposed = false;
  private listeners: (() => void)[] = [];
  constructor(
    private text: string,
    readonly uri: string,
  ) {}
  getValue() {
    return this.text;
  }
  setValue(text: string) {
    this.text = text;
    this.version++;
    for (const l of this.listeners) l();
  }
  getAlternativeVersionId() {
    return this.version;
  }
  setEOL() {}
  onDidChangeContent(fn: () => void) {
    this.listeners.push(fn);
    return { dispose: () => undefined };
  }
  dispose() {
    this.disposed = true;
  }
}

const models = new Map<string, FakeModel>();
const monaco = {
  editor: {
    getModel: (uri: string) => models.get(uri) ?? null,
    createModel: (text: string, _lang: string, uri: string) => {
      const m = new FakeModel(text, uri);
      models.set(uri, m);
      return m;
    },
    EndOfLineSequence: { LF: 0, CRLF: 1 },
  },
} as unknown as MonacoApi;

describe("document registry across project switches", () => {
  beforeEach(() => {
    models.clear();
    disk.hash = "h";
    useEditorStore.setState({ projectId: "A", tabs: [], activePath: null, parked: {} });
  });

  it("keeps dirty models of the previous project and frees clean ones", async () => {
    const a = registryFor(monaco, "A");
    useEditorStore.getState().openFile("dirty.rs");
    useEditorStore.getState().openFile("clean.rs");
    const dirty = await a.load("dirty.rs");
    const clean = await a.load("clean.rs");
    (dirty!.model as unknown as FakeModel).setValue("unsaved edit");
    expect(a.isDirty(dirty!)).toBe(true);

    useEditorStore.getState().setProject("B");
    const b = registryFor(monaco, "B");
    expect(b).not.toBe(a);
    expect((clean!.model as unknown as FakeModel).disposed).toBe(true);
    expect((dirty!.model as unknown as FakeModel).disposed).toBe(false);

    useEditorStore.getState().setProject("A");
    const back = registryFor(monaco, "A");
    expect(back).toBe(a);
    expect(back.get("dirty.rs")?.model.getValue()).toBe("unsaved edit");
    expect(useEditorStore.getState().tabs.find((t) => t.path === "dirty.rs")?.dirty).toBe(true);
  });

  it("frees parked documents of projects that no longer exist", async () => {
    const a = registryFor(monaco, "A");
    useEditorStore.getState().openFile("f.rs");
    const doc = await a.load("f.rs");
    (doc!.model as unknown as FakeModel).setValue("edit");
    registryFor(monaco, "B");
    pruneParkedDocuments(new Set(["B"]));
    expect((doc!.model as unknown as FakeModel).disposed).toBe(true);
    expect(registryFor(monaco, "A")).not.toBe(a);
  });
});

describe("parked project sessions (merged from audit r1 fe-panels-b)", () => {
  beforeEach(() => {
    models.clear();
    disk.hash = "h";
    useEditorStore.setState({ projectId: null, tabs: [], activePath: null, agentModified: {}, parked: {} });
  });

  it("restores the unsaved tab and model after switching away and back", async () => {
    useEditorStore.getState().setProject("pa");
    useEditorStore.getState().openFile("a.ts");
    const a = registryFor(monaco, "pa");
    const doc = await a.load("a.ts");
    (doc!.model as unknown as FakeModel).setValue("edited");
    expect(useEditorStore.getState().tabs[0]?.dirty).toBe(true);

    useEditorStore.getState().setProject("pb");
    expect(registryFor(monaco, "pb")).not.toBe(a);
    useEditorStore.getState().setProject("pa");
    const again = registryFor(monaco, "pa");
    expect(again).toBe(a);
    expect(again.get("a.ts")?.model.getValue()).toBe("edited");
    expect(useEditorStore.getState().tabs.map((t) => [t.path, t.dirty])).toEqual([["a.ts", true]]);
  });

  it("a load finishing after a switch never touches the active project's tab", async () => {
    useEditorStore.getState().setProject("pc");
    useEditorStore.getState().openFile("a.ts");
    const pending = registryFor(monaco, "pc").load("a.ts");
    useEditorStore.getState().setProject("pd");
    useEditorStore.getState().openFile("a.ts");
    await pending;
    expect(useEditorStore.getState().tabs[0]?.status).toBe("loading");
  });

  it("resync flags a parked dirty document stale when the disk moved on", async () => {
    useEditorStore.getState().setProject("pe");
    useEditorStore.getState().openFile("s.ts");
    const e = registryFor(monaco, "pe");
    const doc = await e.load("s.ts");
    (doc!.model as unknown as FakeModel).setValue("mine");
    useEditorStore.getState().setProject("pf");
    registryFor(monaco, "pf");
    disk.hash = "h2";
    useEditorStore.getState().setProject("pe");
    const back = registryFor(monaco, "pe");
    await back.resync();
    const tab = useEditorStore.getState().tabs.find((t) => t.path === "s.ts");
    expect(tab?.stale).toBe(true);
    expect(back.get("s.ts")?.model.getValue()).toBe("mine");
  });
});
