// Shortcuts work on non-Latin keyboard layouts (audit r1: Ctrl+K and every
// letter shortcut did nothing with the Russian layout active).
import { describe, expect, it } from "vitest";

import { keyMatches, physicalCode } from "./keys";
import { matchesShortcut } from "./registry";

const ev = (key: string, code: string, mods: Partial<Record<"ctrlKey" | "shiftKey" | "altKey" | "metaKey", boolean>> = {}) => ({
  key,
  code,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe("layout-independent shortcuts", () => {
  it("maps shortcut keys to physical codes", () => {
    expect(physicalCode("k")).toBe("KeyK");
    expect(physicalCode(",")).toBe("Comma");
    expect(physicalCode("1")).toBe("Digit1");
    expect(physicalCode("f11")).toBeNull();
  });

  it("matches the Russian layout by physical key", () => {
    expect(keyMatches(ev("л", "KeyK"), "k")).toBe(true);
    expect(matchesShortcut(ev("з", "KeyP", { ctrlKey: true }), "Ctrl+P")).toBe(true);
    expect(matchesShortcut(ev("А", "KeyF", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+F")).toBe(true);
    expect(matchesShortcut(ev("б", "Comma", { ctrlKey: true }), "Ctrl+,")).toBe(true);
    expect(matchesShortcut(ev("Р", "KeyH", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+H")).toBe(true);
    expect(matchesShortcut(ev("ё", "Backquote", { ctrlKey: true }), "Ctrl+`")).toBe(true);
  });

  it("keeps matching Latin layouts by character", () => {
    expect(matchesShortcut(ev("k", "KeyK", { ctrlKey: true }), "Ctrl+K")).toBe(true);
    // Dvorak: the K character sits on the physical V key.
    expect(matchesShortcut(ev("k", "KeyV", { ctrlKey: true }), "Ctrl+K")).toBe(true);
    expect(matchesShortcut(ev("t", "KeyK", { ctrlKey: true }), "Ctrl+K")).toBe(false);
    expect(matchesShortcut(ev("F11", "F11"), "F11")).toBe(true);
    expect(matchesShortcut(ev("Backspace", "Backspace", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+Backspace")).toBe(true);
  });

  it("still requires exact modifiers", () => {
    expect(matchesShortcut(ev("л", "KeyK"), "Ctrl+K")).toBe(false);
    expect(matchesShortcut(ev("л", "KeyK", { ctrlKey: true, shiftKey: true }), "Ctrl+K")).toBe(false);
  });
});
