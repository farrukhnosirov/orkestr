// Audit r2: every window runs its own store. Detached panels, the HUD and robot
// windows used to write `orkestr.activeProject` from setProjects (every
// reconnect) and removeProject (every ProjectDeleted) with the stale id they
// loaded at startup, so the HUD jumped to the wrong project and the next launch
// opened it. Only the main shell persists now; the others follow it.
import type { Project } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const KEY = "orkestr.activeProject";

const project = (id: string, createdAt: string): Project =>
  ({
    id,
    name: id,
    description: "",
    color: null,
    rootPath: null,
    archived: false,
    createdAt,
    updatedAt: createdAt,
    lastOpenedAt: null,
  }) as unknown as Project;

const all = [project("A", "2026-01-01T00:00:00Z"), project("B", "2026-01-02T00:00:00Z"), project("C", "2026-01-03T00:00:00Z")];

async function loadStore() {
  vi.resetModules();
  return import("./app");
}

beforeEach(() => {
  window.localStorage.clear();
  window.localStorage.setItem(KEY, "A");
});
afterEach(() => window.localStorage.clear());

describe("active project persistence across windows", () => {
  it("a secondary window never overwrites the main window's choice", async () => {
    const { useAppStore } = await loadStore();
    useAppStore.getState().setProjects(all);
    expect(useAppStore.getState().activeProjectId).toBe("A");
    // The main window switches to B.
    window.localStorage.setItem(KEY, "B");
    // Reconnect and a deleted project in the (stale) secondary window.
    useAppStore.getState().setProjects(all);
    useAppStore.getState().removeProject("C");
    useAppStore.getState().upsertProject({ ...all[0]!, archived: true } as Project);
    expect(window.localStorage.getItem(KEY)).toBe("B");
  });

  it("a secondary window follows the main window through storage events", async () => {
    const { useAppStore, followActiveProject } = await loadStore();
    useAppStore.getState().setProjects(all);
    const off = followActiveProject();
    window.dispatchEvent(new StorageEvent("storage", { key: KEY, newValue: "B" }));
    expect(useAppStore.getState().activeProjectId).toBe("B");
    window.dispatchEvent(new StorageEvent("storage", { key: "other", newValue: "C" }));
    expect(useAppStore.getState().activeProjectId).toBe("B");
    off();
    window.dispatchEvent(new StorageEvent("storage", { key: KEY, newValue: "C" }));
    expect(useAppStore.getState().activeProjectId).toBe("B");
    expect(window.localStorage.getItem(KEY)).toBe("A");
  });

  it("the main window persists switches and deletions", async () => {
    const { useAppStore, ownActiveProject } = await loadStore();
    ownActiveProject();
    useAppStore.getState().setProjects(all);
    useAppStore.getState().setActiveProject("C");
    expect(window.localStorage.getItem(KEY)).toBe("C");
    useAppStore.getState().removeProject("C");
    expect(window.localStorage.getItem(KEY)).toBe("A");
  });
});
