import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { registerWorkspace } from "../../components/workspaceActions";
import { i18n } from "../../i18n";
import { PANELS } from "../../panels/registry";
import { useSettingsStore } from "../settings/settingsStore";
import { commands } from "../commands/registry";
import { WindowLibrary } from "./WindowLibrary";
import { useWindowLibraryStore } from "./store";

const openPanel = vi.fn();

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

beforeEach(() => {
  openPanel.mockReset();
  registerWorkspace({ openPanel, resetLayout: () => {}, toggleMaximize: () => {} });
  useWindowLibraryStore.setState({ open: true, filter: "all", favorites: [], recent: [], pins: [] });
});

afterEach(() => {
  cleanup();
  registerWorkspace(null);
  act(() => useWindowLibraryStore.setState({ open: false }));
});

const search = () => screen.getByRole("textbox", { name: "Search the Window Library" });
const selected = () => screen.getAllByRole("option").find((o) => o.getAttribute("aria-selected") === "true");
const key = (k: string, extra: Partial<KeyboardEventInit> = {}) => fireEvent.keyDown(search(), { key: k, ...extra });

describe("Window Library", () => {
  it("shows a card with title, description and preview for every panel and window", () => {
    render(<WindowLibrary />);
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(PANELS.length + 3);
    const code = options.find((o) => o.getAttribute("data-wl-entry") === "code")!;
    expect(code.querySelector(".wl-card__title")?.textContent).toBe("Code");
    expect(code.textContent).toContain("Monaco editor");
    expect(code.querySelector("img")?.getAttribute("src")).toMatch(/code\.en\.dark\.webp/);
  });

  it("is opened by the registry command (Ctrl+Shift+L) and toggles closed again", () => {
    act(() => useWindowLibraryStore.setState({ open: false }));
    const cmd = commands.find((c) => c.id === "windowLibrary.open");
    expect(cmd?.shortcut).toBe("Ctrl+Shift+L");
    void cmd?.run({ projectId: null });
    expect(useWindowLibraryStore.getState().open).toBe(true);
    void cmd?.run({ projectId: null });
    expect(useWindowLibraryStore.getState().open).toBe(false);
  });

  it("navigates with the keyboard and opens the selection with Enter", () => {
    render(<WindowLibrary />);
    expect(selected()?.getAttribute("data-wl-entry")).toBe(PANELS[0]?.id);
    key("ArrowRight");
    key("ArrowRight");
    expect(selected()?.getAttribute("data-wl-entry")).toBe(PANELS[2]?.id);
    key("ArrowLeft");
    expect(selected()?.getAttribute("data-wl-entry")).toBe(PANELS[1]?.id);
    key("End");
    expect(selected()?.getAttribute("data-wl-entry")).toBe("layoutEditor");
    key("Home");
    key("ArrowDown"); // jsdom has no grid layout: one column
    expect(selected()?.getAttribute("data-wl-entry")).toBe(PANELS[1]?.id);
    key("Enter");
    expect(openPanel).toHaveBeenCalledWith(PANELS[1]?.id);
    expect(useWindowLibraryStore.getState().open).toBe(false);
    expect(useWindowLibraryStore.getState().recent[0]).toBe(PANELS[1]?.id);
  });

  it("searches in Russian while the UI is English and highlights the title", () => {
    render(<WindowLibrary />);
    fireEvent.change(search(), { target: { value: "терминал" } });
    expect(selected()?.getAttribute("data-wl-entry")).toBe("terminal");
    fireEvent.change(search(), { target: { value: "term" } });
    expect(selected()?.querySelector("mark")?.textContent).toBe("Term");
    fireEvent.change(search(), { target: { value: "qqqqzzzz" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/Nothing matches/)).toBeTruthy();
  });

  it("filters by category and cycles categories with PageDown", () => {
    render(<WindowLibrary />);
    fireEvent.click(screen.getByRole("button", { name: /Ecosystem/ }));
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-wl-entry")).sort()).toEqual(["extensions", "marketplace"]);
    key("PageDown");
    expect(useWindowLibraryStore.getState().filter).toBe("system");
    expect(screen.getAllByRole("option")).toHaveLength(3);
  });

  it("toggles favorites with Ctrl+D and lists them under Favorites", () => {
    render(<WindowLibrary />);
    key("ArrowRight");
    key("d", { ctrlKey: true, code: "KeyD" });
    expect(useWindowLibraryStore.getState().favorites).toEqual([PANELS[1]?.id]);
    fireEvent.click(screen.getByRole("button", { name: /^Favorites/ }));
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-wl-entry"))).toEqual([PANELS[1]?.id]);
  });

  it("detail pane offers every action; pin adds the panel to the sidebar", () => {
    render(<WindowLibrary />);
    const detail = document.querySelector(".wl__detail") as HTMLElement;
    expect(within(detail).getByRole("button", { name: /Open here/ })).toBeTruthy();
    expect(within(detail).getByRole("button", { name: "Open in a new window" })).toBeTruthy();
    expect(within(detail).getByRole("button", { name: "Add to layout" })).toBeTruthy();
    fireEvent.click(within(detail).getByRole("button", { name: "Pin to sidebar" }));
    expect(useWindowLibraryStore.getState().pins).toEqual([PANELS[0]?.id]);
    expect(within(detail).getByRole("button", { name: "Unpin from sidebar" })).toBeTruthy();
  });

  it("opens dialog windows (Settings) instead of docking them", () => {
    render(<WindowLibrary />);
    fireEvent.change(search(), { target: { value: "settings" } });
    expect(selected()?.getAttribute("data-wl-entry")).toBe("settings");
    key("Enter");
    expect(useSettingsStore.getState().open).toBe(true);
    expect(openPanel).not.toHaveBeenCalled();
    act(() => useSettingsStore.setState({ open: false }));
  });

  it("Escape clears the search first, then closes", () => {
    render(<WindowLibrary />);
    fireEvent.change(search(), { target: { value: "git" } });
    key("Escape");
    expect((search() as HTMLInputElement).value).toBe("");
    key("Escape");
    expect(useWindowLibraryStore.getState().open).toBe(false);
  });

  it("follows the UI language", async () => {
    await act(() => i18n.changeLanguage("ru"));
    render(<WindowLibrary />);
    expect(screen.getByRole("heading", { name: "Библиотека окон" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Экосистема/ })).toBeTruthy();
    const code = screen.getAllByRole("option").find((o) => o.getAttribute("data-wl-entry") === "code")!;
    expect(code.querySelector("img")?.getAttribute("src")).toMatch(/code\.ru\.dark\.webp/);
    await act(() => i18n.changeLanguage("en"));
  });
});
