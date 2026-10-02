import type { Event, FileEntry, Project } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";

const PID = "0190f5c2-0000-7000-8000-000000000042";

type Handler = (ev: Event) => void;
const calls: { method: string; params: Record<string, unknown> }[] = [];
const handlers: Handler[] = [];
const listings: Record<string, FileEntry[]> = {};

const fake = {
  state: "open",
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (method === "files.list") {
      const path = (params.path as string | undefined) ?? "";
      return { path, entries: listings[path] ?? [], truncated: false };
    }
    if (method === "files.watch" || method === "files.unwatch") return { watching: true, subscribers: 1 };
    if (method === "files.rename") return { from: params.from, entry: entry(params.to as string) };
    return {};
  }),
  subscribe: vi.fn((_topics: string[], handler: Handler) => {
    handlers.push(handler);
    return () => undefined;
  }),
};

vi.mock("../../core/client", () => ({
  getClient: () => fake,
  maybeClient: () => fake,
}));

vi.mock("../../components/workspaceActions", () => ({
  workspaceActions: { openPanel: vi.fn(), resetLayout: vi.fn() },
}));

function entry(path: string, kind: "file" | "dir" = "file"): FileEntry {
  const name = path.split("/").pop() ?? path;
  return { name, path, kind, symlink: false, size: 1, modified: null, ignored: false, hidden: name.startsWith("."), readonly: false };
}

function project(rootPath: string | null): Project {
  return {
    id: PID,
    name: "Demo",
    slug: "demo",
    description: "",
    rootPath,
    repositoryId: null,
    privacy: { ai: { mode: "CloudAllowed" }, sync: "Allowed" },
    classification: "Internal",
    approvalMode: "Ask",
    budget: null,
    color: null,
    icon: null,
    tags: [],
    archived: false,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
    lastOpenedAt: null,
  };
}

beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  calls.length = 0;
  handlers.length = 0;
  listings[""] = [entry("src", "dir"), entry("README.md")];
  listings.src = [entry("src/main.rs")];
  window.localStorage.clear();
  useAppStore.setState({ connection: { ...useAppStore.getState().connection, state: "open" } });
  useEditorStore.getState().setProject(null);
  useEditorStore.getState().setProject(PID);
});

afterEach(() => cleanup());

// Imported once at module scope (after the mocks): transforming the panel
// inside the first test can exceed the per-test timeout on a loaded machine.
const { default: FilesPanel } = await import("./index");

async function renderPanel(root: string | null = "C:\\work\\demo") {
  useAppStore.getState().setProjects([project(root)]);
  useAppStore.getState().setActiveProject(PID);
  return render(<FilesPanel projectId={PID} />);
}

describe("Files panel", () => {
  it("asks for a folder when the project has none", async () => {
    await renderPanel(null);
    expect(screen.getByText("Connect a project folder")).toBeTruthy();
    expect(screen.getByLabelText("Project folder")).toBeTruthy();
  });

  it("lists, expands, opens files and watches the project", async () => {
    await renderPanel();
    await screen.findByText("README.md");
    expect(calls.some((c) => c.method === "files.watch" && c.params.projectId === PID)).toBe(true);
    const rows = screen.getAllByRole("treeitem");
    expect(rows.map((r) => r.textContent)).toEqual(["src", "README.md"]);

    fireEvent.click(screen.getByText("src"));
    await screen.findByText("main.rs");
    expect(calls.some((c) => c.method === "files.list" && c.params.path === "src")).toBe(true);

    fireEvent.click(screen.getByText("main.rs"));
    expect(useEditorStore.getState().activePath).toBe("src/main.rs");
  });

  it("supports keyboard navigation and the context menu", async () => {
    await renderPanel();
    await screen.findByText("README.md");
    const tree = screen.getByRole("tree");
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(screen.getByText("README.md").closest("[role='treeitem']")?.getAttribute("aria-selected")).toBe("true");
    fireEvent.contextMenu(screen.getByText("README.md"));
    const menu = screen.getByRole("menu");
    expect([...menu.querySelectorAll("[role='menuitem']")].map((b) => b.textContent)).toContain("Rename…F2");
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("refreshes the affected folder and marks agent changes on FileChanged", async () => {
    await renderPanel();
    await screen.findByText("README.md");
    listings[""] = [entry("src", "dir"), entry("README.md"), entry("agent.ts")];
    const ev = {
      seq: 1,
      id: "e1",
      ts: "2026-10-02T00:00:00Z",
      projectId: PID,
      taskId: null,
      agentId: null,
      nodeId: null,
      correlationId: null,
      kind: "FileChanged",
      payload: { path: "agent.ts", oldPath: null, change: "Created", actor: "agent", agentId: null },
    } as unknown as Event;
    act(() => {
      for (const h of handlers) h(ev);
    });
    await waitFor(() => expect(screen.getByText("agent.ts")).toBeTruthy());
    expect(useEditorStore.getState().agentModified["agent.ts"]).toBeDefined();
    expect(screen.getByTitle("Changed by an agent")).toBeTruthy();
  });

  it("renames inline with F2", async () => {
    await renderPanel();
    await screen.findByText("README.md");
    const tree = screen.getByRole("tree");
    fireEvent.keyDown(tree, { key: "End" });
    fireEvent.keyDown(tree, { key: "F2" });
    const input = screen.getByLabelText("Rename README.md") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "GUIDE.md" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(calls.some((c) => c.method === "files.rename" && c.params.to === "GUIDE.md")).toBe(true));
  });

  it("renders in Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    try {
      await renderPanel();
      await screen.findByText("README.md");
      expect(screen.getByRole("tree", { name: "Файлы проекта" })).toBeTruthy();
      expect(screen.getByRole("button", { name: "Свернуть всё" })).toBeTruthy();
      fireEvent.contextMenu(screen.getByText("README.md"));
      const items = [...screen.getByRole("menu").querySelectorAll("[role='menuitem']")].map((b) => b.textContent);
      expect(items).toContain("Переименовать…F2");
      expect(items).toContain("Копировать относительный путь");
    } finally {
      await act(async () => {
        await applyLanguagePreference("en");
      });
    }
  });
});
