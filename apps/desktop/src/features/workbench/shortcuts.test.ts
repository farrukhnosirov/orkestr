// File-menu shortcuts: physical-key matching (Russian layout), VS Code chord
// Ctrl+K Ctrl+O, no clashes with the rest of the registry; Save via the bus;
// Close project keeps the Welcome page up.
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../core/client", () => ({
  getClient: () => ({ call: () => Promise.resolve({}) }),
  openProject: () => Promise.resolve(),
  maybeClient: () => null,
}));

import type { Project } from "@orkestr/protocol";

import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import { chordCommand, isModifierKey, shortcutCommand } from "../commands/dispatch";
import { chordSteps, matchesShortcut } from "../commands/keys";
import { commands } from "../commands/registry";
import { i18n } from "../../i18n";
import { registerSaveAsHandler, registerSaveHandler, requestSave, requestSaveAs, resetSaveBus } from "./saveBus";

const ev = (key: string, code: string, mods: Partial<KeyboardEvent> = {}) => ({
  key,
  code,
  ctrlKey: true,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  target: null,
  ...mods,
});

const ctx = { projectId: "p1" };

beforeEach(() => {
  resetSaveBus();
  useAppStore.setState({
    projects: [{ id: "p1", name: "a", rootPath: "C:\\a", archived: false, createdAt: "", lastOpenedAt: null } as unknown as Project],
    activeProjectId: "p1",
    projectClosed: false,
  });
  useEditorStore.setState({ projectId: "p1", tabs: [], activePath: null, parked: {} });
});

describe("file shortcuts", () => {
  it("work on the Russian layout through the physical key", () => {
    expect(shortcutCommand(ev("щ", "KeyO"), commands, ctx)?.id).toBe("file.openFile");
    expect(shortcutCommand(ev("т", "KeyN"), commands, ctx)?.id).toBe("file.newFile");
    expect(shortcutCommand(ev("ы", "KeyS"), commands, ctx)?.id).toBe("file.save");
    expect(shortcutCommand(ev("Ы", "KeyS", { shiftKey: true }), commands, ctx)?.id).toBe("file.saveAll");
    expect(shortcutCommand(ev("а", "KeyF", { ctrlKey: false, altKey: true }), commands, ctx)?.id).toBe("file.menu");
  });

  it("Ctrl+K Ctrl+O is a chord: never a single key, found after the Ctrl+K prefix", () => {
    expect(chordSteps("Ctrl+K Ctrl+O")).toEqual(["Ctrl+K", "Ctrl+O"]);
    expect(matchesShortcut(ev("o", "KeyO"), "Ctrl+K Ctrl+O")).toBe(false);
    // Plain Ctrl+O stays "Open file", not the chord.
    expect(shortcutCommand(ev("o", "KeyO"), commands, ctx)?.id).toBe("file.openFile");
    expect(chordCommand("Ctrl+K", ev("щ", "KeyO"), commands, ctx)?.id).toBe("file.openFolder");
    expect(chordCommand("Ctrl+K", ev("o", "KeyO", { ctrlKey: false }), commands, ctx)).toBeNull();
    expect(chordCommand("Ctrl+K", ev("p", "KeyP"), commands, ctx)).toBeNull();
    expect(isModifierKey("Control") && isModifierKey("Shift") && !isModifierKey("o")).toBe(true);
  });

  it("no two registry commands share a shortcut", () => {
    const seen = new Map<string, string>();
    for (const c of commands) {
      if (!c.shortcut) continue;
      const k = c.shortcut.toLowerCase();
      expect(seen.get(k), `${c.id} clashes with ${seen.get(k)} on ${c.shortcut}`).toBeUndefined();
      seen.set(k, c.id);
    }
  });

  it("file commands are searchable in both languages", () => {
    for (const id of ["file.openFolder", "file.openFile", "file.newFile", "file.save", "file.saveAs", "file.clone", "file.closeProject"]) {
      const kw = commands.find((c) => c.id === id)?.keywords ?? [];
      expect(kw.some((k) => /[a-z]/.test(k)), id).toBe(true);
      expect(kw.some((k) => /[а-я]/.test(k)), id).toBe(true);
    }
  });

  it("project-dependent commands are disabled without a project", () => {
    const none = { projectId: null };
    for (const id of ["file.newFile", "file.newFolder", "file.save", "file.saveAs", "file.reveal", "file.closeProject"]) {
      const c = commands.find((x) => x.id === id);
      expect(c?.enabled?.(none), id).toBe(false);
    }
    expect(shortcutCommand(ev("n", "KeyN"), commands, none)).toBeNull();
  });
});

describe("File menu", () => {
  it("every entry has a command and a label in both languages; hidden items leave no stray separators", async () => {
    const { FILE_MENU, visibleEntries } = await import("./FileMenu");
    const cmds = FILE_MENU.filter((e) => e.kind === "cmd");
    for (const e of cmds) {
      expect(commands.some((c) => c.id === e.command), e.command).toBe(true);
      for (const lng of ["en", "ru"]) expect(i18n.exists(`workbench:menu.${e.label}`, { lng }), `${lng} ${e.label}`).toBe(true);
    }
    const browser = visibleEntries(FILE_MENU, (id) => id !== "app.exit");
    expect(browser.at(-1)?.kind).not.toBe("sep");
    expect(visibleEntries([{ kind: "sep" }, { kind: "recent" }, { kind: "sep" }, { kind: "sep" }, { kind: "recent" }, { kind: "sep" }], () => true)).toEqual([
      { kind: "recent" },
      { kind: "sep" },
      { kind: "recent" },
    ]);
  });
});

describe("save bus", () => {
  it("delivers to the mounted Code panel, queues while it mounts, ignores clean editors", () => {
    expect(requestSave(false)).toBe("nothing");
    useEditorStore.setState({ tabs: [{ path: "a.ts", dirty: true } as never] });
    expect(requestSave(true)).toBe("queued");
    const got: boolean[] = [];
    const off = registerSaveHandler((all) => got.push(all));
    expect(got).toEqual([true]); // the queued request is delivered on mount
    expect(requestSave(false)).toBe("saved");
    expect(got).toEqual([true, false]);
    off();
    expect(requestSave(false)).toBe("queued");
  });

  it("Save As goes to the mounted Code panel only", () => {
    expect(requestSaveAs()).toBe(false);
    let n = 0;
    const off = registerSaveAsHandler(() => (n += 1));
    expect(requestSaveAs()).toBe(true);
    expect(n).toBe(1);
    off();
    expect(requestSaveAs()).toBe(false);
  });
});

describe("close project", () => {
  it("shows no project and does not auto-pick one on reload or events", () => {
    useAppStore.getState().closeProject();
    expect(useAppStore.getState().activeProjectId).toBeNull();
    expect(useAppStore.getState().projectClosed).toBe(true);
    const p2 = { id: "p2", name: "b", rootPath: null, archived: false, createdAt: "", lastOpenedAt: null } as unknown as Project;
    useAppStore.getState().upsertProject(p2);
    useAppStore.getState().setProjects(useAppStore.getState().projects);
    expect(useAppStore.getState().activeProjectId).toBeNull();
    useAppStore.getState().setActiveProject("p2");
    expect(useAppStore.getState().projectClosed).toBe(false);
    expect(useAppStore.getState().activeProjectId).toBe("p2");
  });
});
