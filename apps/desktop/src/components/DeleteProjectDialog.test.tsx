// Projects can be deleted from the UI (audit r1): rail context menu and the
// `project.delete` command open a confirmation that calls `projects.delete`.
import type { Project } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
/** Overrides the next responses (e.g. a `project_busy` conflict). */
const failures: unknown[] = [];
vi.mock("../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      const failure = failures.shift();
      if (failure) return Promise.reject(failure);
      return Promise.resolve({ projectId: (params as { projectId: string }).projectId });
    },
  }),
  openProject: () => Promise.resolve(),
  maybeClient: () => null,
}));

import { OrkestrError } from "@orkestr/client";

import { commands } from "../features/commands/registry";
import { i18n } from "../i18n";
import { useAppStore } from "../store/app";
import { useEditorStore } from "../store/editor";
import { ProjectRail } from "./ProjectRail";

const project = (id: string, name: string, rootPath: string | null = null): Project =>
  ({
    id,
    name,
    description: "",
    color: null,
    rootPath,
    archived: false,
    createdAt: `2026-01-0${id}T00:00:00Z`,
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt: null,
  }) as unknown as Project;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  failures.length = 0;
  useAppStore.setState({
    projects: [project("1", "alpha", "C:/code/alpha"), project("2", "beta")],
    projectsLoaded: true,
    activeProjectId: "1",
    deleteProjectId: null,
  });
  useEditorStore.setState({ projectId: "1", tabs: [], activePath: null, parked: {} });
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("delete project", () => {
  it("rail context menu → confirm → projects.delete, files kept", async () => {
    render(<ProjectRail />);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Open project alpha" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete project…" }));
    expect(screen.getByText(/C:\/code\/alpha stays as it is/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete project" }));
    });
    expect(calls).toEqual([{ method: "projects.delete", params: { projectId: "1" } }]);
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(["2"]);
    expect(useAppStore.getState().deleteProjectId).toBeNull();
  });

  it("cancel does not delete; the command opens the dialog and warns about unsaved tabs", async () => {
    useEditorStore.setState({
      parked: { "2": { tabs: [{ path: "a.ts", status: "ready", dirty: true, stale: false, deleted: false, language: "ts", agentLines: [] }], activePath: "a.ts" } },
    });
    render(<ProjectRail />);
    const cmd = commands.find((c) => c.id === "project.delete");
    expect(cmd?.enabled?.({ projectId: null })).toBe(false);
    act(() => void cmd?.run({ projectId: "2" }));
    expect(screen.getByText(/1 unsaved file/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls).toEqual([]);
    expect(useAppStore.getState().deleteProjectId).toBeNull();
  });

  it("Russian strings", async () => {
    await i18n.changeLanguage("ru");
    render(<ProjectRail />);
    act(() => useAppStore.getState().setDeleteProjectId("2"));
    expect(screen.getByText("Удалить проект «beta»?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Удалить проект" })).toBeTruthy();
  });

  it("a project with running work offers to stop it first (audit r2)", async () => {
    failures.push(new OrkestrError("conflict", "busy", { reason: "project_busy", agents: 1 }));
    render(<ProjectRail />);
    act(() => useAppStore.getState().setDeleteProjectId("1"));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete project" }));
    });
    expect(screen.getByText(/still has running work/)).toBeTruthy();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(["1", "2"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop work and delete" }));
    });
    expect(calls).toEqual([
      { method: "projects.delete", params: { projectId: "1" } },
      { method: "projects.delete", params: { projectId: "1", stopWork: true } },
    ]);
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(["2"]);
  });
});
