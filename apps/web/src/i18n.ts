/**
 * Web client i18n (en/ru). Resources are auto-discovered from
 * `locales/<lang>/<namespace>.json`; English is the key source.
 */
import i18next, { type Resource } from "i18next";
import { initReactI18next } from "react-i18next";

export const SUPPORTED_LANGUAGES = ["en", "ru"] as const;
export type UiLanguage = (typeof SUPPORTED_LANGUAGES)[number];

type Json = Record<string, unknown>;

const modules = import.meta.glob<Json>("./locales/*/*.json", { eager: true, import: "default" });

/** Builds the i18next resource tree `{ lng: { ns: json } }`. */
export function buildResources(files: Readonly<Record<string, Json>> = modules): Resource {
  const out: Resource = {};
  for (const [path, json] of Object.entries(files)) {
    const m = /\/locales\/([^/]+)\/([^/]+)\.json$/.exec(path);
    if (!m?.[1] || !m[2]) continue;
    const lang = (out[m[1]] ??= {});
    lang[m[2]] = json;
  }
  return out;
}

function detectLanguage(): UiLanguage {
  const nav = typeof navigator === "undefined" ? "en" : navigator.language.toLowerCase();
  return nav.startsWith("ru") ? "ru" : "en";
}

export const i18n = i18next.createInstance();

void i18n.use(initReactI18next).init({
  resources: buildResources(),
  lng: detectLanguage(),
  fallbackLng: "en",
  supportedLngs: [...SUPPORTED_LANGUAGES],
  defaultNS: "common",
  ns: ["common"],
  interpolation: { escapeValue: false },
  initAsync: false,
  react: { useSuspense: false },
});
