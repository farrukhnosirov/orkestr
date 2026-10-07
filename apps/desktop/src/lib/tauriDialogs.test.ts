// Native pickers (File → Open, "Browse…"): only inside the desktop shell;
// in a browser they resolve to null without touching the dialog plugin.
import { beforeEach, describe, expect, it, vi } from "vitest";

let inShell = false;
const dialogCalls: unknown[] = [];
let dialogResult: unknown = null;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: () => Promise.resolve(null),
  isTauri: () => inShell,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (options: unknown) => {
    dialogCalls.push(options);
    return Promise.resolve(dialogResult);
  },
}));

import { canPickPaths, pickFile, pickFolder } from "./tauri";

beforeEach(() => {
  inShell = false;
  dialogCalls.length = 0;
  dialogResult = null;
});

describe("native pickers", () => {
  it("fall back to null (and hide Browse) outside the desktop shell", async () => {
    expect(canPickPaths()).toBe(false);
    expect(await pickFolder({ title: "x" })).toBeNull();
    expect(await pickFile()).toBeNull();
    expect(dialogCalls).toEqual([]);
  });

  it("open a single-folder dialog in the shell", async () => {
    inShell = true;
    dialogResult = "C:\\Code\\my-app";
    expect(canPickPaths()).toBe(true);
    expect(await pickFolder({ title: "Open folder", defaultPath: "C:\\Code" })).toBe("C:\\Code\\my-app");
    expect(dialogCalls).toEqual([{ directory: true, multiple: false, title: "Open folder", defaultPath: "C:\\Code" }]);
  });

  it("open a single-file dialog and normalize the result", async () => {
    inShell = true;
    dialogResult = ["C:\\a.txt", "C:\\b.txt"];
    expect(await pickFile()).toBe("C:\\a.txt");
    expect(dialogCalls).toEqual([{ directory: false, multiple: false }]);
    dialogResult = null; // cancelled
    expect(await pickFile()).toBeNull();
    dialogResult = "";
    expect(await pickFolder()).toBeNull();
  });
});
