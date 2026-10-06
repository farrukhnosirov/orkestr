// The Code panel's document registry keeps unsaved Monaco models across a
// project switch (audit r1: switching project silently discarded edits).
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./monaco", () => ({ modelUri: (projectId: string, path: string) => `orkestr://${projectId}/${path}` }));
vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (_method: string, params: { path: string }) =>
      Promise.resolve({
        content: `disk:${params.path}`,
        binary: false,
        language: "plaintext",
        lineEnding: "lf",
        hash: "h",
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
