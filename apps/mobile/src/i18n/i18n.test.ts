import * as fs from "node:fs";
import * as path from "node:path";

import { en } from "./en";
import { ru } from "./ru";

function keys(o: object, prefix = ""): string[] {
  return Object.entries(o).flatMap(([k, v]) => (typeof v === "string" ? [prefix + k] : keys(v as object, `${prefix}${k}.`)));
}

function walk(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e: fs.Dirent) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return walk(p);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });
}

describe("i18n", () => {
  it("en and ru have the same keys", () => {
    expect(keys(ru).sort()).toEqual(keys(en).sort());
  });

  it("no empty strings and interpolation variables match", () => {
    const enMap = new Map(keys(en).map((k) => [k, k.split(".").reduce<unknown>((o, p) => (o as Record<string, unknown>)[p], en) as string]));
    for (const k of keys(ru)) {
      const r = k.split(".").reduce<unknown>((o, p) => (o as Record<string, unknown>)[p], ru) as string;
      expect(r.trim()).not.toBe("");
      const vars = (s: string) => (s.match(/{{\s*\w+\s*}}/g) ?? []).sort();
      expect([k, vars(r)]).toEqual([k, vars(enMap.get(k) ?? "")]);
    }
  });

  it("every static t() key used in the app exists", () => {
    const all = new Set(keys(en));
    const files = [...walk(path.join(__dirname, "..")), path.join(__dirname, "../../App.tsx")];
    const missing: string[] = [];
    for (const f of files) {
      for (const m of fs.readFileSync(f, "utf8").matchAll(/\bt\("([a-zA-Z0-9_.]+)"/g)) {
        if (!all.has(m[1] as string)) missing.push(`${path.basename(f)}: ${m[1]}`);
      }
    }
    expect(missing).toEqual([]);
  });
});
