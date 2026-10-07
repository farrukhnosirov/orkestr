// File menu (keyboard accessible, VS Code style) and the Welcome page shown
// while no project is open.
import type { Project } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const opened: string[] = [];
vi.mock("../../core/client", () => ({
  getClient: () => ({ call: () => Promise.resolve({}) }),
  openProject: (id: string) => {
    opened.push(id);
    return Promise.resolve();
  },
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { FileMenu } from "./FileMenu";
import { useWorkbenchStore } from "./store";
import { WelcomePage } from "./WelcomePage";

const project = (id: string, name: string, lastOpenedAt: string | null, rootPath: string | null = `C:\\Code\\${name}`): Project =>
  ({
    id,
    name,
    description: "",
    color: null,
    rootPath,
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt,
  }) as unknown as Project;

const flush = () => act(() => new Promise<void>((r) => requestAnimationFrame(() => r())));

beforeEach(async () => {
  await i18n.changeLanguage("en");
  opened.length = 0;
  useAppStore.setState({
    projects: [
      project("a", "alpha", "2026-03-01T00:00:00Z"),
      project("b", "beta", "2026-05-01T00:00:00Z"),
      project("c", "gamma", null),
    ],
    projectsLoaded: true,
    activeProjectId: null,
    projectClosed: true,
    createProjectOpen: false,
  });
  useWorkbenchStore.setState({ menuOpen: false, cloneOpen: false, hiddenRecent: {}, entryPrompt: null, pathPrompt: null });
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("File menu", () => {
  it("opens from the trigger, focuses the first item and moves with the arrows", async () => {
    render(<FileMenu />);
    const trigger = screen.getByRole("menuitem", { name: "File" });
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(trigger);
    await flush();
    const menu = screen.getByRole("menu", { name: "File menu" });
    expect(document.activeElement?.textContent).toContain("New project…");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement?.textContent).toContain("Open folder…");
    expect(document.activeElement?.textContent).toContain("Ctrl+K Ctrl+O");
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    // Wraps to the last enabled item (no project: Close project/Reveal/New file are disabled; Exit is desktop-only).
    expect(document.activeElement?.textContent).toContain("Recent projects");
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("disables project actions while no project is open", async () => {
    render(<FileMenu />);
    act(() => useWorkbenchStore.getState().setMenuOpen(true));
    await flush();
    const item = (name: RegExp) => screen.getByRole("menuitem", { name });
    expect((item(/^New file/) as HTMLButtonElement).disabled).toBe(true);
    expect((item(/^Close project/) as HTMLButtonElement).disabled).toBe(true);
    expect((item(/^Open file…/) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("menuitem", { name: /^Exit/ })).toBeNull(); // not inside the desktop shell
  });

  it("lists recent projects in a submenu (→ opens, ← closes) and opens one", async () => {
    render(<FileMenu />);
    act(() => useWorkbenchStore.getState().setMenuOpen(true));
    await flush();
    const recent = screen.getByRole("menuitem", { name: /Recent projects/ });
    recent.focus();
    fireEvent.keyDown(recent, { key: "ArrowRight" });
    await flush();
    const sub = screen.getByRole("menu", { name: "Recent projects" });
    const items = [...sub.querySelectorAll("[role='menuitem']")].map((el) => el.textContent ?? "");
    expect(items[0]).toContain("beta"); // most recent first
    expect(items[1]).toContain("alpha");
    expect(items).toHaveLength(2); // never-opened projects are not "recent"
    expect(document.activeElement?.textContent).toContain("beta");
    fireEvent.keyDown(sub, { key: "ArrowLeft" });
    expect(screen.queryByRole("menu", { name: "Recent projects" })).toBeNull();
    expect(document.activeElement).toBe(recent);
    fireEvent.keyDown(recent, { key: "ArrowRight" });
    await flush();
    fireEvent.click(screen.getByRole("menuitem", { name: /alpha/ }));
    expect(opened).toEqual(["a"]);
    expect(useWorkbenchStore.getState().menuOpen).toBe(false);
  });

  it("runs commands: Clone repository opens its dialog", async () => {
    render(<FileMenu />);
    act(() => useWorkbenchStore.getState().setMenuOpen(true));
    await flush();
    fireEvent.click(screen.getByRole("menuitem", { name: /Clone repository/ }));
    expect(useWorkbenchStore.getState().cloneOpen).toBe(true);
    expect(useWorkbenchStore.getState().menuOpen).toBe(false);
  });

  it("is translated", async () => {
    await i18n.changeLanguage("ru");
    render(<FileMenu />);
    act(() => useWorkbenchStore.getState().setMenuOpen(true));
    await flush();
    expect(screen.getByRole("menuitem", { name: "Файл" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Открыть папку…/ })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Недавние проекты/ })).toBeTruthy();
  });
});

describe("Welcome page", () => {
  it("shows the start actions and the recent list, most recent first", () => {
    render(<WelcomePage />);
    expect(screen.getByRole("heading", { name: "Welcome to Orkestr" })).toBeTruthy();
    for (const name of ["New project…", "Open folder…", "Clone repository…", "Open file…"]) {
      expect(screen.getByRole("button", { name: new RegExp(name) })).toBeTruthy();
    }
    const recent = screen.getAllByRole("button", { name: /^Open (alpha|beta)$/ }).map((b) => b.getAttribute("aria-label"));
    expect(recent).toEqual(["Open beta", "Open alpha"]);
  });

  it("opens a recent project and removes one from the list", () => {
    render(<WelcomePage />);
    fireEvent.click(screen.getByRole("button", { name: "Open alpha" }));
    expect(opened).toEqual(["a"]);
    fireEvent.click(screen.getByRole("button", { name: "Remove beta from the recent list" }));
    expect(screen.queryByRole("button", { name: "Open beta" })).toBeNull();
    // Opening it again later brings it back.
    act(() => useAppStore.getState().upsertProject(project("b", "beta", "2026-06-01T00:00:00Z")));
    expect(screen.getByRole("button", { name: "Open beta" })).toBeTruthy();
  });

  it("starts the flows: new project dialog and clone dialog", () => {
    render(<WelcomePage />);
    fireEvent.click(screen.getByRole("button", { name: /New project…/ }));
    expect(useAppStore.getState().createProjectOpen).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /Clone repository…/ }));
    expect(useWorkbenchStore.getState().cloneOpen).toBe(true);
  });

  it("shows loading and empty states", () => {
    useAppStore.setState({ projectsLoaded: false });
    const { unmount } = render(<WelcomePage />);
    expect(screen.getByText("Loading projects…")).toBeTruthy();
    unmount();
    useAppStore.setState({ projectsLoaded: true, projects: [] });
    render(<WelcomePage />);
    expect(screen.getByText("Projects you open will appear here.")).toBeTruthy();
  });

  it("Open folder without the desktop shell asks for a typed path", () => {
    render(<WelcomePage />);
    fireEvent.click(screen.getByRole("button", { name: /Open folder…/ }));
    expect(useWorkbenchStore.getState().pathPrompt).toBe("openFolder");
  });
});
