// @vitest-environment node
/**
 * Guard: locale files are complete and consistent.
 * - every namespace exists in every language and has a type file;
 * - the same keys in en and ru (plural forms compared by base key);
 * - plural sets complete: en `_one/_other`, ru `_one/_few/_many/_other`;
 * - no empty or non-string values;
 * - the same `{{placeholders}}` and `<tags>` in every translation of a key.
 */
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const SRC = fileURLToPath(new URL("..", import.meta.url));
const LOCALES = join(SRC, "locales");
const LANGS = ["en", "ru"] as const;
const PLURAL_FORMS: Record<(typeof LANGS)[number], readonly string[]> = {
  en: ["one", "other"],
  ru: ["one", "few", "many", "other"],
};
const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

type Flat = Map<string, unknown>;

function flatten(obj: unknown, prefix = "", out: Flat = new Map()): Flat {
  if (obj && typeof obj === "object" && !Array.isArray(obj)) {
    for (const [k, v] of Object.entries(obj)) flatten(v, prefix ? `${prefix}.${k}` : k, out);
  } else {
    out.set(prefix, obj);
  }
  return out;
}

function load(lang: string, ns: string): Flat {
  return flatten(JSON.parse(readFileSync(join(LOCALES, lang, `${ns}.json`), "utf8")) as unknown);
}

/** Groups keys: plural forms (`x_one`, `x_few`…) collapse to `x` when `x_other` exists. */
function group(flat: Flat): Map<string, { forms: string[]; values: string[] }> {
  const out = new Map<string, { forms: string[]; values: string[] }>();
  for (const [key, value] of flat) {
    const m = PLURAL_SUFFIX.exec(key);
    const base = m && flat.has(key.replace(PLURAL_SUFFIX, "_other")) ? key.replace(PLURAL_SUFFIX, "") : key;
    const entry = out.get(base) ?? { forms: [], values: [] };
    if (base !== key && m?.[1]) entry.forms.push(m[1]);
    entry.values.push(String(value));
    out.set(base, entry);
  }
  return out;
}

const tokens = (values: string[]) =>
  [...new Set(values.flatMap((v) => [...v.matchAll(/\{\{\s*([\w.]+)[^}]*\}\}|<\/?(\w+)\s*\/?>/g)].map((m) => m[1] ?? `<${m[2]}>`)))]
    .filter((x) => x !== "count")
    .sort();

const namespaces = readdirSync(join(LOCALES, "en"))
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.slice(0, -5))
  .sort();

describe("locale files", () => {
  it("have the same namespaces in every language", () => {
    expect(namespaces.length).toBeGreaterThan(0);
    for (const lang of LANGS) {
      const own = readdirSync(join(LOCALES, lang))
        .filter((f) => f.endsWith(".json"))
        .map((f) => f.slice(0, -5))
        .sort();
      expect(own, `namespaces of ${lang}`).toEqual(namespaces);
    }
  });

  it("every namespace has a type file (src/i18n/namespaces/<ns>.ts)", () => {
    const missing = namespaces.filter((ns) => !existsSync(join(SRC, "i18n", "namespaces", `${ns}.ts`)));
    expect(missing).toEqual([]);
  });

  for (const ns of namespaces) {
    describe(ns, () => {
      const flats = Object.fromEntries(LANGS.map((l) => [l, load(l, ns)])) as Record<(typeof LANGS)[number], Flat>;
      const groups = Object.fromEntries(LANGS.map((l) => [l, group(flats[l])])) as Record<
        (typeof LANGS)[number],
        ReturnType<typeof group>
      >;

      it("has the same keys in en and ru", () => {
        const en = [...groups.en.keys()].sort();
        const ru = [...groups.ru.keys()].sort();
        expect(en.filter((k) => !groups.ru.has(k)), "missing in ru").toEqual([]);
        expect(ru.filter((k) => !groups.en.has(k)), "missing in en").toEqual([]);
      });

      it("has only non-empty string values", () => {
        for (const lang of LANGS) {
          const bad = [...flats[lang]].filter(([, v]) => typeof v !== "string" || v.trim() === "").map(([k]) => k);
          expect(bad, `${lang}: empty or non-string`).toEqual([]);
        }
      });

      it("has complete plural sets", () => {
        for (const lang of LANGS) {
          const wrong: string[] = [];
          for (const [base, entry] of groups[lang]) {
            const isPlural = entry.forms.length > 0 || groups.en.get(base)?.forms.length;
            if (!isPlural) continue;
            const missing = PLURAL_FORMS[lang].filter((f) => !entry.forms.includes(f));
            if (missing.length) wrong.push(`${base}: missing _${missing.join(", _")}`);
          }
          expect(wrong, `${lang} plural forms`).toEqual([]);
        }
      });

      it("keeps the same placeholders and tags in every language", () => {
        const diff: string[] = [];
        for (const [base, entry] of groups.en) {
          const other = groups.ru.get(base);
          if (!other) continue;
          const a = tokens(entry.values).join(",");
          const b = tokens(other.values).join(",");
          if (a !== b) diff.push(`${base}: en [${a}] ≠ ru [${b}]`);
        }
        expect(diff).toEqual([]);
      });
    });
  }
});
