// Audit r2: registry shortcuts ran in a window capture listener regardless of
// focus, so Ctrl+P (shell previous-history) typed into a terminal opened Quick
// Open and never reached the PTY.
import { describe, expect, it } from "vitest";

import { isTerminalTarget, shortcutCommand } from "./dispatch";
import type { AppCommand } from "./registry";

const cmd = (id: string, shortcut: string, extra: Partial<AppCommand> = {}): AppCommand => ({
  id,
  title: id,
  group: "g",
  shortcut,
  run: () => {},
  ...extra,
});

const list: AppCommand[] = [
  cmd("files.quickOpen", "Ctrl+P"),
  cmd("layout.coding", "Ctrl+1"),
  cmd("terminal.open", "Ctrl+`", { inTerminal: true }),
  cmd("ai.stopAll", "Ctrl+Shift+Backspace", { inTerminal: true }),
  cmd("disabled", "Ctrl+Shift+Q", { enabled: () => false }),
];

function targets() {
  document.body.innerHTML = `
    <div class="xterm"><textarea class="xterm-helper-textarea"></textarea></div>
    <div class="monaco-editor"><textarea></textarea></div>`;
  return {
    terminal: document.querySelector(".xterm textarea")!,
    editor: document.querySelector(".monaco-editor textarea")!,
  };
}

const key = (k: string, code: string, target: EventTarget | null, mods: Partial<KeyboardEvent> = {}) => ({
  key: k,
  code,
  ctrlKey: true,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  target,
  ...mods,
});

const ctx = { projectId: "p1" };

describe("shortcutCommand", () => {
  it("Ctrl+P runs Quick Open outside a terminal", () => {
    const { editor } = targets();
    expect(shortcutCommand(key("p", "KeyP", editor), list, ctx)?.id).toBe("files.quickOpen");
    expect(shortcutCommand(key("p", "KeyP", document.body), list, ctx)?.id).toBe("files.quickOpen");
  });

  it("Ctrl+P / Ctrl+1 typed in a terminal are left to the shell", () => {
    const { terminal } = targets();
    expect(shortcutCommand(key("p", "KeyP", terminal), list, ctx)).toBeNull();
    // Cyrillic layout: Ctrl+P arrives as "з".
    expect(shortcutCommand(key("з", "KeyP", terminal), list, ctx)).toBeNull();
    expect(shortcutCommand(key("1", "Digit1", terminal), list, ctx)).toBeNull();
  });

  it("allow-listed commands still fire from a terminal", () => {
    const { terminal } = targets();
    expect(shortcutCommand(key("`", "Backquote", terminal), list, ctx)?.id).toBe("terminal.open");
    expect(shortcutCommand(key("Backspace", "Backspace", terminal, { shiftKey: true }), list, ctx)?.id).toBe("ai.stopAll");
  });

  it("disabled commands never fire", () => {
    expect(shortcutCommand(key("q", "KeyQ", document.body, { shiftKey: true }), list, ctx)).toBeNull();
  });

  it("isTerminalTarget tolerates non-element targets", () => {
    expect(isTerminalTarget(null)).toBe(false);
    expect(isTerminalTarget(window)).toBe(false);
  });

  it("the real registry keeps the emergency stop and terminal toggle reachable from a terminal", async () => {
    const { commands } = await import("./registry");
    const byId = new Map(commands.map((c) => [c.id, c]));
    expect(byId.get("ai.stopAll")?.inTerminal).toBe(true);
    expect(byId.get("terminal.open")?.inTerminal).toBe(true);
    expect(byId.get("files.quickOpen")?.inTerminal).toBeFalsy();
  }, 60_000);
});
