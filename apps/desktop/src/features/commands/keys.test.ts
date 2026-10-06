import { describe, expect, it } from "vitest";

import { codeKey, matchesShortcut, shortcutKey } from "./keys";
import { commands } from "./registry";

const base = { ctrlKey: true, metaKey: false, shiftKey: false, altKey: false };

describe("layout-independent shortcuts", () => {
  it("maps physical codes to US keys", () => {
    expect(codeKey("KeyK")).toBe("k");
    expect(codeKey("Digit3")).toBe("3");
    expect(codeKey("Comma")).toBe(",");
    expect(codeKey("Backquote")).toBe("`");
    expect(codeKey("ArrowLeft")).toBeNull();
  });

  it("uses the physical key for non-Latin characters only", () => {
    expect(shortcutKey({ key: "л", code: "KeyK" })).toBe("k");
    expect(shortcutKey({ key: "Б", code: "Comma" })).toBe(",");
    expect(shortcutKey({ key: "k", code: "KeyK" })).toBe("k");
    // AZERTY: the label wins for Latin layouts (A is on the physical Q key).
    expect(shortcutKey({ key: "a", code: "KeyQ" })).toBe("a");
    expect(shortcutKey({ key: "Escape", code: "Escape" })).toBe("escape");
  });

  it("matches the Russian-layout events of every registered shortcut", () => {
    // Ctrl+K (palette), Ctrl+P, Ctrl+Shift+F/T/M/D, Ctrl+Alt+P, Ctrl+, on ЙЦУКЕН.
    expect(matchesShortcut({ ...base, key: "л", code: "KeyK" }, "Ctrl+K")).toBe(true);
    expect(matchesShortcut({ ...base, key: "з", code: "KeyP" }, "Ctrl+P")).toBe(true);
    expect(matchesShortcut({ ...base, shiftKey: true, key: "А", code: "KeyF" }, "Ctrl+Shift+F")).toBe(true);
    expect(matchesShortcut({ ...base, shiftKey: true, key: "Е", code: "KeyT" }, "Ctrl+Shift+T")).toBe(true);
    expect(matchesShortcut({ ...base, shiftKey: true, key: "В", code: "KeyD" }, "Ctrl+Shift+D")).toBe(true);
    expect(matchesShortcut({ ...base, altKey: true, key: "з", code: "KeyP" }, "Ctrl+Alt+P")).toBe(true);
    expect(matchesShortcut({ ...base, key: "б", code: "Comma" }, "Ctrl+,")).toBe(true);
    expect(matchesShortcut({ ...base, key: "л", code: "KeyK" }, "Ctrl+P")).toBe(false);
    expect(matchesShortcut({ ...base, shiftKey: true, key: "л", code: "KeyK" }, "Ctrl+K")).toBe(false);

    const usKey = (k: string): { key: string; code: string } => {
      const code = /^[a-z]$/.test(k) ? `Key${k.toUpperCase()}` : /^[0-9]$/.test(k) ? `Digit${k}` : k === "," ? "Comma" : k === "`" ? "Backquote" : "";
      return { key: k, code };
    };
    const RU: Record<string, string> = { k: "л", p: "з", f: "а", t: "е", m: "ь", d: "в", h: "р", ",": "б", "`": "ё" };
    for (const cmd of commands) {
      if (!cmd.shortcut) continue;
      const parts = cmd.shortcut.split("+");
      const last = (parts[parts.length - 1] ?? "").toLowerCase();
      const ru = RU[last];
      if (!ru) continue;
      const mods = { ctrlKey: parts.includes("Ctrl"), metaKey: false, shiftKey: parts.includes("Shift"), altKey: parts.includes("Alt") };
      expect(matchesShortcut({ ...mods, ...usKey(last), key: ru }, cmd.shortcut), cmd.shortcut).toBe(true);
    }
  });
});
