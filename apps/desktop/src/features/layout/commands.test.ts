import { afterAll, describe, expect, it } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { commands, matchesShortcut } from "../commands/registry";
import { layoutCommands } from "./commands";
import { layoutName } from "./names";
import { registerLayoutFeature } from "./register";
import { BUILTIN_LAYOUT_IDS } from "./schema";

describe("layout commands", () => {
  afterAll(async () => {
    await applyLanguagePreference("en");
  });

  it("registers once, with unique ids and shortcuts", () => {
    registerLayoutFeature();
    registerLayoutFeature();
    const ids = commands.map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const c of layoutCommands) expect(ids.filter((id) => id === c.id)).toHaveLength(1);
    const shortcuts = commands.map((c) => c.shortcut).filter((s): s is string => !!s);
    expect(new Set(shortcuts.map((s) => s.toLowerCase())).size).toBe(shortcuts.length);
  });

  it("switches presets with Ctrl+1..5 and has Save / Editor commands", () => {
    const byKey = (key: string) =>
      layoutCommands.find((c) => c.shortcut && matchesShortcut({ key, code: `Digit${key}`, ctrlKey: true, metaKey: false, shiftKey: false, altKey: false }, c.shortcut));
    expect(["1", "2", "3", "4", "5"].map((k) => byKey(k)?.id)).toEqual([
      "layout.switch.coding",
      "layout.switch.mission-control",
      "layout.switch.monitoring",
      "layout.switch.focus",
      "layout.switch.review",
    ]);
    expect(layoutCommands.map((c) => c.id)).toEqual(expect.arrayContaining(["layout.saveAs", "layout.editor", "layout.lock", "layout.detach", "layout.spread"]));
  });

  it("titles and preset names follow the language", async () => {
    await applyLanguagePreference("ru");
    expect(layoutName("coding")).toBe("Разработка");
    expect(layoutName("mission-control")).toBe("Центр управления");
    expect(layoutName("u-x", "Моя")).toBe("Моя");
    const ru = layoutCommands.map((c) => c.title);
    await applyLanguagePreference("en");
    const en = layoutCommands.map((c) => c.title);
    expect(layoutName("coding")).toBe("Coding");
    for (const [i, title] of en.entries()) {
      expect(title).not.toMatch(/^layout:|commands\./);
      expect(ru[i]).not.toBe(title);
    }
    for (const id of BUILTIN_LAYOUT_IDS) expect(layoutName(id)).not.toContain("presets.");
  });

  it("is searchable in both languages", () => {
    const save = layoutCommands.find((c) => c.id === "layout.saveAs");
    expect(save?.keywords).toEqual(expect.arrayContaining(["save", "сохранить"]));
    const spread = layoutCommands.find((c) => c.id === "layout.spread");
    expect(spread?.keywords).toEqual(expect.arrayContaining(["monitor", "монитор"]));
  });
});
