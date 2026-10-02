// @vitest-environment node
/**
 * Guard: no hardcoded user-visible strings in TSX. Every UI text goes through
 * `t()` (docs/i18n/README.md).
 *
 * `hardcoded-allowlist.json`:
 * - `literals`: exact texts that are the same in every language (brand,
 *   keyboard keys). Keep this list short.
 * - `files`: TEMPORARY list of files not converted yet (paths relative to
 *   `src/`). Remove a file from it in the same change that converts it; the
 *   test fails when a listed file is already clean, so the list only shrinks.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { findHardcodedStrings } from "./hardcoded";

const SRC = fileURLToPath(new URL("..", import.meta.url));
const allowlist = JSON.parse(readFileSync(new URL("./hardcoded-allowlist.json", import.meta.url), "utf8")) as {
  literals: string[];
  files: string[];
};
const literals = new Set(allowlist.literals);

function tsxFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      if (name === "locales" || name === "node_modules") continue;
      out.push(...tsxFiles(full));
    } else if (name.endsWith(".tsx") && !/\.test\.tsx$/.test(name)) {
      out.push(full);
    }
  }
  return out;
}

const rel = (full: string) => relative(SRC, full).split(sep).join("/");
const files = tsxFiles(SRC).sort();
const scan = (full: string) => findHardcodedStrings(full, readFileSync(full, "utf8"), literals);

describe("hardcoded UI strings", () => {
  it("finds them in a sample (scanner self-test)", () => {
    const found = findHardcodedStrings(
      "x.tsx",
      `const a = <div title="Hello" aria-label={ok ? "Yes" : "No"}>Some text {t("ns:key")} {"inline"} <Kbd>Ctrl</Kbd> 42 —</div>;`,
      new Set(["Ctrl"]),
    );
    expect(found.map((f) => f.text)).toEqual(["Hello", "Yes", "No", "Some text", "inline"]);
  });

  it("every converted file uses t() for user-visible text", () => {
    const listed = new Set(allowlist.files);
    const problems: string[] = [];
    for (const full of files) {
      const path = rel(full);
      if (listed.has(path)) continue;
      for (const f of scan(full)) problems.push(`${path}:${f.line} ${f.kind}${f.prop ? `(${f.prop})` : ""} "${f.text}"`);
    }
    expect(problems, `Hardcoded strings — move them to src/locales/{en,ru}/*.json:\n${problems.join("\n")}`).toEqual([]);
  });

  it("the allow-list only names unconverted files that exist", () => {
    const existing = new Set(files.map(rel));
    const stale = allowlist.files.filter((p) => !existing.has(p) || scan(join(SRC, p)).length === 0);
    expect(stale, "Remove these entries from src/i18n/hardcoded-allowlist.json (converted or gone)").toEqual([]);
  });
});
