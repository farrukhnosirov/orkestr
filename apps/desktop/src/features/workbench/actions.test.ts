// File → Open folder / Open file: an existing project is reused, a new folder
// becomes a project named after it; files inside a project open in Code.
import type { Project } from "@orkestr/protocol";
import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
const opened: string[] = [];
let nextProject: Project | null = null;

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      if (method === "projects.create") return Promise.resolve({ project: nextProject });
      if (method === "files.create") return Promise.resolve({ entry: { path: params["path"] } });
      return Promise.resolve({});
    },
  }),
  openProject: (id: string) => {
    opened.push(id);
    return Promise.resolve();
  },
  maybeClient: () => null,
}));

const panelIntents: unknown[] = [];
vi.mock("../files/actions", () => ({
  openInEditor: (path: string) => panelIntents.push({ type: "openFile", path }),
  saveEditor: (all: boolean) => panelIntents.push({ type: "saveFiles", all }),
}));

import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import { createEntry, openFileFromDisk, openFolderAsProject, openOutsideFile } from "./actions";
import { useWorkbenchStore } from "./store";

const project = (id: string, name: string, rootPath: string | null): Project =>
  ({
    id,
    name,
    description: "",
    color: null,
    rootPath,
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt: null,
  }) as unknown as Project;

beforeEach(() => {
  calls.length = 0;
  opened.length = 0;
  panelIntents.length = 0;
  nextProject = null;
  useAppStore.setState({
    projects: [project("p1", "alpha", "C:\\Code\\alpha"), project("p2", "beta", null)],
    projectsLoaded: true,
    activeProjectId: "p2",
    projectClosed: false,
  });
  useEditorStore.setState({ projectId: "p2", tabs: [], activePath: null, parked: {} });
  useWorkbenchStore.setState({ outsideFile: null });
});

describe("open folder as project", () => {
  it("switches to the project that already uses the folder", async () => {
    const r = await openFolderAsProject("c:/code/ALPHA/");
    expect(r).toEqual({ project: expect.objectContaining({ id: "p1" }), created: false });
    expect(opened).toEqual(["p1"]);
    expect(calls.filter((c) => c.method === "projects.create")).toEqual([]);
  });

  it("creates a project named after a new folder and opens it", async () => {
    nextProject = project("p3", "my-app", "D:\\Work\\my-app");
    const r = await openFolderAsProject("D:\\Work\\my-app\\");
    expect(r.created).toBe(true);
    const create = calls.find((c) => c.method === "projects.create");
    expect(create?.params).toMatchObject({ name: "my-app", rootPath: "D:\\Work\\my-app\\" });
    expect(typeof create?.params["color"]).toBe("string");
    expect(useAppStore.getState().projects.some((p) => p.id === "p3")).toBe(true);
    expect(opened).toEqual(["p3"]);
  });
});

describe("open file", () => {
  it("opens a file inside a project in that project's Code panel", async () => {
    expect(await openFileFromDisk("C:\\Code\\alpha\\src\\main.ts")).toBe("opened");
    expect(opened).toEqual(["p1"]);
    expect(useEditorStore.getState().projectId).toBe("p1");
    expect(panelIntents).toEqual([{ type: "openFile", path: "src/main.ts" }]);
  });

  it("offers to open the folder of a file outside every project", async () => {
    expect(await openFileFromDisk("E:\\notes\\todo.md")).toBe("outside");
    expect(useWorkbenchStore.getState().outsideFile).toEqual({ path: "E:\\notes\\todo.md", folder: "E:\\notes" });
    expect(opened).toEqual([]);

    nextProject = project("p4", "notes", "E:\\notes");
    await openOutsideFile({ path: "E:\\notes\\todo.md", folder: "E:\\notes" });
    expect(opened).toEqual(["p4"]);
    expect(panelIntents).toEqual([{ type: "openFile", path: "todo.md" }]);
  });
});

describe("new file", () => {
  it("creates the file through files.create and opens it", async () => {
    await createEntry("p1", "file", "\\src\\new.ts");
    expect(calls.at(-1)).toEqual({ method: "files.create", params: { projectId: "p1", path: "src/new.ts" } });
    expect(panelIntents).toEqual([{ type: "openFile", path: "src/new.ts" }]);
  });

  it("creates folders through files.mkdir", async () => {
    await createEntry("p1", "folder", "src/components");
    expect(calls.at(-1)).toEqual({ method: "files.mkdir", params: { projectId: "p1", path: "src/components" } });
  });
});
