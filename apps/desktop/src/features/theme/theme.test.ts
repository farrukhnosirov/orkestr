// @vitest-environment jsdom
import type { ThemeConfig } from "@orkestr/protocol";
import { BUILTIN_THEMES } from "@orkestr/ui";
import { describe, expect, it } from "vitest";

import en from "../../locales/en/theme.json";
import ru from "../../locales/ru/theme.json";
import { frameDue, particleCount, rng, scenePalette } from "./backgrounds/scene";
import {
  COLOR_GROUPS,
  EDITOR_TOKENS,
  FONT_SIZES,
  MONO_FONTS,
  RADII,
  ROBOT_SKINS,
  SANS_FONTS,
  SHADOW_PRESETS,
  SOUND_EVENTS,
  SOUND_PRESETS,
  duplicateTheme,
  editDraft,
  exportFileName,
  familyScale,
  prepareImport,
  resetDraft,
  savesAsNew,
  shadowPresetOf,
  slugify,
  startDraft,
  uniqueThemeId,
  withFamilyScale,
  withToken,
} from "./logic";
import { activeTheme } from "./themeStore";

const nebula = (): ThemeConfig => structuredClone(BUILTIN_THEMES["nebula"] as ThemeConfig);

function get(obj: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((o, k) => (o && typeof o === "object" ? (o as Record<string, unknown>)[k] : undefined), obj);
}

describe("ids, duplication, import", () => {
  it("slugifies to valid ids", () => {
    expect(slugify("My Theme 2!")).toBe("my-theme-2");
    expect(slugify("42 rays")).toBe("theme-42-rays");
    expect(slugify("Тёмная")).toBe("theme");
  });
  it("never reuses built-in or taken ids", () => {
    expect(uniqueThemeId("Nebula", new Set())).toBe("nebula-2");
    expect(uniqueThemeId("Mine", new Set(["mine", "mine-2"]))).toBe("mine-3");
  });
  it("duplicates under a new id and name", () => {
    const copy = duplicateTheme(nebula(), "Nebula (copy)", new Set(["nebula"]));
    expect(copy.id).toBe("nebula-copy");
    expect(copy.name).toBe("Nebula (copy)");
    expect(copy.tokens).toEqual(nebula().tokens);
  });
  it("imports never overwrite silently", () => {
    expect(prepareImport(nebula(), new Set()).id).toBe("nebula-2");
    const mine = { ...nebula(), id: "mine", name: "Mine" };
    expect(prepareImport(mine, new Set(["mine"])).id).toBe("mine-2");
    expect(prepareImport(mine, new Set()).id).toBe("mine");
    expect(exportFileName(mine)).toBe("mine.orkestr-theme.json");
  });
});

describe("token families", () => {
  it("scales and reads back", () => {
    const tokens = withFamilyScale({}, RADII, 1.5);
    expect(tokens["radius-md"]).toBe("15px");
    expect(familyScale(tokens, RADII)).toBe(1.5);
    expect(withFamilyScale(tokens, RADII, 1)).toEqual({});
    expect(familyScale({}, FONT_SIZES)).toBe(1);
  });
  it("sets and removes single tokens", () => {
    expect(withToken({ a: "1" }, "b", "2")).toEqual({ a: "1", b: "2" });
    expect(withToken({ a: "1" }, "a", null)).toEqual({});
    expect(withToken({ a: "1" }, "a", "  ")).toEqual({});
  });
  it("recognises shadow presets", () => {
    expect(shadowPresetOf({})).toBe("medium");
    expect(shadowPresetOf({ ...SHADOW_PRESETS.deep })).toBe("deep");
    expect(shadowPresetOf({ "shadow-panel": "0 0 1px #000" })).toBeNull();
  });
});

describe("draft state machine", () => {
  it("previews only valid drafts and tracks dirtiness", () => {
    let s = startDraft(nebula());
    expect(s.dirty).toBe(false);
    s = editDraft(s, (t) => ({ ...t, tokens: { ...t.tokens, "color-accent": "#ff00aa" } }));
    expect(s.dirty).toBe(true);
    expect(s.issues).toEqual([]);
    expect(s.preview.tokens["color-accent"]).toBe("#ff00aa");
    // Illegible text: the draft shows it, the preview keeps the last valid version.
    s = editDraft(s, (t) => ({ ...t, tokens: { ...t.tokens, "color-text": "#05060f" } }));
    expect(s.issues.map((i) => i.path)).toContain("theme.contrast.text-bg");
    expect(s.draft.tokens["color-text"]).toBe("#05060f");
    expect(s.preview.tokens["color-text"]).toBeUndefined();
    s = resetDraft(s);
    expect(s.dirty).toBe(false);
    expect(s.draft).toEqual(s.base);
  });
  it("saves built-ins and new themes as new entries", () => {
    const s = startDraft(nebula());
    expect(savesAsNew(s, new Set())).toBe(true);
    const mine = startDraft({ ...nebula(), id: "mine" });
    expect(savesAsNew(mine, new Set(["mine"]))).toBe(false);
    expect(savesAsNew(mine, new Set())).toBe(true);
  });
  it("active theme: editor preview > config > fallback", () => {
    const fallback = nebula();
    const carbon = structuredClone(BUILTIN_THEMES["carbon"] as ThemeConfig);
    const config = { schemaVersion: 1, theme: carbon, layout: { preset: "Coding" as const, docking: null }, performanceMode: "Balanced" as const };
    expect(activeTheme({ editorOpen: false, editor: null, config: null, fallback }).id).toBe("nebula");
    expect(activeTheme({ editorOpen: false, editor: null, config, fallback }).id).toBe("carbon");
    const editor = startDraft({ ...carbon, id: "draft" });
    expect(activeTheme({ editorOpen: true, editor, config, fallback }).id).toBe("draft");
  });
});

describe("background scene helpers", () => {
  it("caps frames", () => {
    expect(frameDue(0, null, 0)).toBe(true);
    expect(frameDue(10, 0, 30)).toBe(false);
    expect(frameDue(34, 0, 30)).toBe(true);
    expect(frameDue(1000, 0, 0)).toBe(false);
  });
  it("bounds particle counts", () => {
    expect(particleCount(1920 * 1080, 0.5)).toBe(249);
    expect(particleCount(100, 1)).toBe(12);
    expect(particleCount(8000 * 8000, 1)).toBe(260);
  });
  it("picks a palette", () => {
    expect(scenePalette(["#ff0000", "nope"], [])).toEqual([{ r: 255, g: 0, b: 0, a: 1 }]);
    expect(scenePalette([], ["#00ff00"])).toEqual([{ r: 0, g: 255, b: 0, a: 1 }]);
    expect(scenePalette([], ["var(--x)"])).toHaveLength(1);
  });
  it("is deterministic", () => {
    const a = rng(1);
    const b = rng(1);
    expect([a(), a()]).toEqual([b(), b()]);
  });
});

describe("theme i18n coverage", () => {
  it("labels every editor token, group, preset and event in both languages", () => {
    const keys = [
      ...EDITOR_TOKENS.map((k) => `tokens.${k}`),
      ...COLOR_GROUPS.map((g) => `groups.${g.id}`),
      ...SANS_FONTS.map((f) => `fonts.${f.id}`),
      ...MONO_FONTS.map((f) => `fonts.${f.id}`),
      ...SOUND_EVENTS.map((e) => `soundEvents.${e}`),
      ...SOUND_PRESETS.map((p) => `soundPresets.${p}`),
      ...ROBOT_SKINS.map((s) => `robotSkins.${s}`),
      ...Object.keys(BUILTIN_THEMES).map((id) => `builtin.${id}`),
    ];
    for (const key of keys) {
      expect(typeof get(en, key), `en ${key}`).toBe("string");
      expect(typeof get(ru, key), `ru ${key}`).toBe("string");
    }
  });
});
