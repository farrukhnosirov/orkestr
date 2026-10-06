// Project switches must never drop unsaved editor tabs (audit r1: data loss).
import { beforeEach, describe, expect, it } from "vitest";

import { projectsWithUnsavedTabs, useEditorStore } from "./editor";

const reset = () =>
  useEditorStore.setState({ projectId: null, tabs: [], activePath: null, reveal: null, agentModified: {}, parked: {}, diff: false });

describe("editor store project switch", () => {
  beforeEach(reset);

  it("parks dirty tabs when leaving a project and restores them on return", () => {
    const s = useEditorStore.getState();
    s.setProject("A");
    s.openFile("src/main.rs");
    s.openFile("README.md");
    s.updateTab("src/main.rs", { status: "ready", dirty: true });
    s.updateTab("README.md", { status: "ready" });
    s.activate("src/main.rs");

    s.setProject("B");
    let st = useEditorStore.getState();
    expect(st.tabs).toEqual([]);
    expect(st.parked.A?.tabs.map((t) => t.path)).toEqual(["src/main.rs"]);
    expect(projectsWithUnsavedTabs(st)).toEqual(["A"]);

    st.setProject("A");
    st = useEditorStore.getState();
    expect(st.tabs.map((t) => [t.path, t.dirty])).toEqual([["src/main.rs", true]]);
    expect(st.activePath).toBe("src/main.rs");
    expect(st.parked).toEqual({});
  });

  it("drops clean projects entirely and forgets deleted ones", () => {
    const s = useEditorStore.getState();
    s.setProject("A");
    s.openFile("a.txt");
    s.setProject("B");
    expect(useEditorStore.getState().parked).toEqual({});

    s.openFile("b.txt");
    useEditorStore.getState().updateTab("b.txt", { dirty: true });
    useEditorStore.getState().setProject("A");
    expect(Object.keys(useEditorStore.getState().parked)).toEqual(["B"]);
    useEditorStore.getState().forgetProject("B");
    expect(useEditorStore.getState().parked).toEqual({});
  });

  it("restores parked tabs after switching through 'no project'", () => {
    const s = useEditorStore.getState();
    s.setProject("A");
    s.openFile("x.ts");
    useEditorStore.getState().updateTab("x.ts", { dirty: true });
    useEditorStore.getState().setProject(null);
    useEditorStore.getState().setProject("A");
    expect(useEditorStore.getState().tabs.map((t) => t.path)).toEqual(["x.ts"]);
  });
});
