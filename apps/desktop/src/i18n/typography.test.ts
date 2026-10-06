// @vitest-environment node
/**
 * Guard (audit r1): Russian strings follow the glossary and typography rules
 * that are easy to break by hand:
 * - roadmap phases are «этап N», never «фаза N» (docs/i18n/glossary.md);
 * - percentages are formatted with `fmt.percent` (Russian «80 %» with a
 *   non-breaking space), so no translation appends a literal `%` to a
 *   placeholder.
 */
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { applyLanguagePreference, formatPercent, t } from "./index";

const LOCALES = fileURLToPath(new URL("../locales", import.meta.url));

function strings(lang: string): [string, string][] {
  const out: [string, string][] = [];
  const walk = (v: unknown, path: string) => {
    if (typeof v === "string") out.push([path, v]);
    else if (v && typeof v === "object") for (const [k, x] of Object.entries(v)) walk(x, path ? `${path}.${k}` : k);
  };
  for (const file of readdirSync(join(LOCALES, lang))) {
    if (file.endsWith(".json")) walk(JSON.parse(readFileSync(join(LOCALES, lang, file), "utf8")) as unknown, file.replace(/\.json$/, ":"));
  }
  return out;
}

describe("locale typography", () => {
  it("Russian uses «этап» for roadmap phases", () => {
    expect(strings("ru").filter(([, v]) => /\bфаз[аеыуой]\s+\d/i.test(v))).toEqual([]);
  });

  it("no translation appends a literal % to a placeholder", () => {
    for (const lang of ["en", "ru"]) expect(strings(lang).filter(([, v]) => /\}\}\s?%/.test(v))).toEqual([]);
  });

  it("the Mission Control budget alert formats its percentage like the rest of the app", async () => {
    await applyLanguagePreference("ru");
    const line = t("missionControl:alerts.kind.budget", { percent: formatPercent(0.8), spent: "8 $", limit: "10 $" });
    expect(line.replace(/ /g, " ")).toBe("Бюджет израсходован на 80 %: 8 $ из 10 $");
    await applyLanguagePreference("en");
  });
});
