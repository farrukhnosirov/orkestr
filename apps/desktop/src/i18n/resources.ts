/**
 * Translation resources, auto-discovered from `src/locales/<lang>/<namespace>.json`.
 *
 * Adding a namespace = adding `locales/en/<ns>.json` + `locales/ru/<ns>.json`
 * (+ a one-line type file in `i18n/namespaces/<ns>.ts`). Nothing here needs
 * editing: Vite bundles every JSON file matched by the glob below.
 */
import type { Resource } from "i18next";

/** Languages the UI ships in. English is the source language (and the key source). */
export const SUPPORTED_LANGUAGES = ["en", "ru"] as const;
export type UiLanguage = (typeof SUPPORTED_LANGUAGES)[number];
export const SOURCE_LANGUAGE: UiLanguage = "en";

type Json = Record<string, unknown>;

const modules = import.meta.glob<Json>("../locales/*/*.json", { eager: true, import: "default" });

/** `{ path → json }` of every locale file (exported for the guard tests). */
export const LOCALE_FILES: Readonly<Record<string, Json>> = modules;

function parsePath(path: string): { lng: string; ns: string } | null {
  const m = /\/locales\/([^/]+)\/([^/]+)\.json$/.exec(path);
  return m && m[1] && m[2] ? { lng: m[1], ns: m[2] } : null;
}

/** Builds the i18next resource tree `{ lng: { ns: json } }`. */
export function buildResources(files: Readonly<Record<string, Json>> = modules): Resource {
  const out: Resource = {};
  for (const [path, json] of Object.entries(files)) {
    const parsed = parsePath(path);
    if (!parsed) continue;
    const lang = (out[parsed.lng] ??= {});
    lang[parsed.ns] = json;
  }
  return out;
}

/** Namespaces present in the source language. */
export function namespacesOf(resources: Resource): string[] {
  return Object.keys(resources[SOURCE_LANGUAGE] ?? {}).sort();
}
