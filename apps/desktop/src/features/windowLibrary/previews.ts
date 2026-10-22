/**
 * Bundled preview screenshots of every library entry
 * (`assets/window-previews/<id>.<lang>.<mode>.webp`, produced by
 * `scripts/smoke/window-library-previews.mjs` from the real app). Vite emits
 * them as separate hashed files; only the URLs live in the JS bundle.
 */
import type { UiLanguage } from "../../i18n";

export type PreviewMode = "dark" | "light";

const FILES = import.meta.glob<string>("../../assets/window-previews/*.webp", { eager: true, query: "?url", import: "default" });

/** `id.lang.mode` → URL. */
export const PREVIEWS: ReadonlyMap<string, string> = new Map(
  Object.entries(FILES).flatMap(([path, url]) => {
    const m = /\/([^/]+)\.(en|ru)\.(dark|light)\.webp$/.exec(path);
    return m ? [[`${m[1]}.${m[2]}.${m[3]}`, url] as const] : [];
  }),
);

/**
 * Preview URL for an entry: the exact language + theme mode first, then the
 * same mode in English, then the other mode. `undefined` when none exists.
 */
export function previewUrl(id: string, lang: UiLanguage, mode: PreviewMode, previews: ReadonlyMap<string, string> = PREVIEWS): string | undefined {
  const other: PreviewMode = mode === "dark" ? "light" : "dark";
  for (const key of [`${id}.${lang}.${mode}`, `${id}.en.${mode}`, `${id}.${lang}.${other}`, `${id}.en.${other}`]) {
    const url = previews.get(key);
    if (url) return url;
  }
  return undefined;
}
