// @vitest-environment node
/**
 * Guard (Wave 8, Window Library): every registered panel — and every dialog
 * window listed in the library — has a category, a card summary, a longer
 * description and tags in English AND Russian, and bundled preview
 * screenshots for both languages in dark and light themes
 * (`scripts/smoke/window-library-previews.mjs` produces them). A new panel
 * without them fails here.
 */
import { existsSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { beforeAll, describe, expect, it } from "vitest";

import { entryKeys, libraryEntries, tr } from "../features/windowLibrary/catalog";
import { i18n } from "../i18n";
import { PANELS } from "./registry";
import { PANEL_CATEGORIES } from "./types";

const PREVIEW_DIR = fileURLToPath(new URL("../assets/window-previews/", import.meta.url));
const LANGS = ["en", "ru"] as const;
const MODES = ["dark", "light"] as const;
/** Previews are bundled: keep each one small. */
const MAX_PREVIEW_BYTES = 60 * 1024;

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

describe("Window Library metadata", () => {
  it("every panel has a known category", () => {
    for (const p of PANELS) expect(PANEL_CATEGORIES, p.id).toContain(p.category);
  });

  it("every panel and window has a summary, details and tags in en and ru", () => {
    const problems: string[] = [];
    for (const e of libraryEntries()) {
      const keys = entryKeys(e.id, e.kind);
      for (const lang of LANGS) {
        for (const [field, key] of Object.entries(keys)) {
          const text = tr(lang, key);
          if (!text || text === key || text.endsWith(`.${field}`)) problems.push(`${lang} ${e.id}.${field} missing`);
        }
        const summary = tr(lang, keys.summary);
        if (summary.length > 120) problems.push(`${lang} ${e.id}.summary too long for two card lines (${summary.length})`);
        if (tr(lang, keys.details).length <= summary.length) problems.push(`${lang} ${e.id}.details should say more than the summary`);
      }
      if (tr("en", keys.summary) === tr("ru", keys.summary)) problems.push(`${e.id}.summary is not translated`);
    }
    expect(problems).toEqual([]);
  });

  it("registry getters return the localized texts", async () => {
    const code = PANELS.find((p) => p.id === "code");
    expect(code?.summary).toMatch(/Monaco/);
    expect(code?.tags.length).toBeGreaterThan(1);
    await i18n.changeLanguage("ru");
    expect(code?.summary).toMatch(/Редактор/);
    await i18n.changeLanguage("en");
  });

  it("every entry has small preview screenshots for en/ru × dark/light", () => {
    const missing: string[] = [];
    const tooBig: string[] = [];
    for (const e of libraryEntries()) {
      for (const lang of LANGS) {
        for (const mode of MODES) {
          const file = `${PREVIEW_DIR}${e.id}.${lang}.${mode}.webp`;
          if (!existsSync(file)) missing.push(`${e.id}.${lang}.${mode}.webp`);
          else if (statSync(file).size > MAX_PREVIEW_BYTES) tooBig.push(`${e.id}.${lang}.${mode}.webp`);
        }
      }
    }
    expect(missing, "run scripts/smoke/window-library-previews.mjs").toEqual([]);
    expect(tooBig).toEqual([]);
  });

  it("previews are real WebP images", () => {
    const head = readFileSync(`${PREVIEW_DIR}code.en.dark.webp`).subarray(0, 12).toString("latin1");
    expect(head.startsWith("RIFF")).toBe(true);
    expect(head.slice(8)).toBe("WEBP");
  });
});
