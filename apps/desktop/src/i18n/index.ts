/**
 * Orkestr UI internationalisation (i18next + react-i18next).
 *
 * - Resources: `src/locales/<lang>/<namespace>.json`, auto-loaded (see `resources.ts`).
 * - Keys are type-checked against the English files (`namespaces/<ns>.ts`).
 * - React: `const { t } = useT("shell")`; outside React: `t("shell:rail.label")`.
 * - Registries (commands, panels, settings sections) use getters so titles
 *   are re-read on every render: `get title() { return t("shell:commands.x"); }`.
 * - Formatting: `useFormat()` / `format.ts` (Intl-based numbers, money, dates,
 *   relative time, bytes, durations) and i18next formatters inside strings:
 *   `{{size, bytes}}`, `{{ms, duration}}`, `{{amount, usd}}`, `{{ts, relative}}`.
 *
 * See docs/i18n/README.md and the glossary in docs/i18n/glossary.md.
 */
import i18n from "i18next";
import { useMemo } from "react";
import { initReactI18next, useTranslation } from "react-i18next";

import { formatBytes, formatCurrency, formatDuration, formatRelativeTime, formattersFor, type Formatters } from "./format";
import { buildResources, namespacesOf, SOURCE_LANGUAGE, SUPPORTED_LANGUAGES, type UiLanguage } from "./resources";

export { SUPPORTED_LANGUAGES, type UiLanguage } from "./resources";
export * from "./format";

/** The stored preference (`AppSettings.language`). */
export type LanguagePreference = UiLanguage | "system";

export const LANGUAGE_STORAGE_KEY = "orkestr.language";

export function isUiLanguage(value: string | null | undefined): value is UiLanguage {
  return (SUPPORTED_LANGUAGES as readonly string[]).includes(value ?? "");
}

/**
 * The OS / browser language mapped to a supported one: Russian when the
 * system prefers Russian, otherwise English.
 */
export function systemLanguage(languages?: readonly string[]): UiLanguage {
  const list =
    languages ??
    (typeof navigator === "undefined" ? [] : navigator.languages?.length ? navigator.languages : [navigator.language]);
  for (const tag of list) {
    const primary = (tag ?? "").toLowerCase().split("-")[0];
    if (isUiLanguage(primary)) return primary;
  }
  return SOURCE_LANGUAGE;
}

/** Resolves a stored preference (`ru`, `en`, `system`, or an unknown tag) to a UI language. */
export function resolveLanguage(preference: string | null | undefined, languages?: readonly string[]): UiLanguage {
  const primary = (preference ?? "").toLowerCase().split("-")[0];
  return isUiLanguage(primary) ? primary : systemLanguage(languages);
}

/** Normalises a stored value to a preference (`de` etc. → `system`). */
export function toPreference(value: string | null | undefined): LanguagePreference {
  return isUiLanguage(value) ? value : "system";
}

function readCachedPreference(): string | null {
  try {
    return window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
  } catch {
    return null;
  }
}

function writeCachedPreference(pref: LanguagePreference): void {
  try {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, pref);
  } catch {
    /* storage unavailable */
  }
}

const resources = buildResources();

void i18n.use(initReactI18next).init({
  resources,
  // Before settings arrive: last preference seen on this machine, else the system language.
  lng: resolveLanguage(typeof window === "undefined" ? null : readCachedPreference()),
  fallbackLng: SOURCE_LANGUAGE,
  supportedLngs: [...SUPPORTED_LANGUAGES],
  ns: namespacesOf(resources),
  defaultNS: "common",
  fallbackNS: "common",
  // `{{addr}}`: the Jarvis form of address (", сэр" / ""), kept current by features/jarvis/persona.ts
  interpolation: { escapeValue: false, defaultVariables: { addr: "" } },
  returnNull: false,
  initAsync: false,
  react: { useSuspense: false, bindI18n: "languageChanged loaded" },
});

// Formatters usable inside strings: "{{size, bytes}}", "{{ms, duration}}", "{{amount, usd}}", "{{ts, relative}}".
i18n.services.formatter?.add("bytes", (value: number, lng) => formatBytes(value, lng));
i18n.services.formatter?.add("duration", (value: number, lng) => formatDuration(value, lng));
i18n.services.formatter?.add("usd", (value: number, lng) => formatCurrency(value, "USD", lng));
i18n.services.formatter?.add("relative", (value: string | number | Date, lng) => formatRelativeTime(value, Date.now(), lng));

export { i18n };

/** Global `t` for non-React code (stores, registries, notifications). Keys use `ns:key`. */
export const t: typeof i18n.t = i18n.t.bind(i18n) as typeof i18n.t;

/** The current UI language. */
export function currentLanguage(): UiLanguage {
  return resolveLanguage(i18n.resolvedLanguage ?? i18n.language);
}

/** `useTranslation` for one or more namespaces (type-safe keys, re-renders on language change). */
export const useT = useTranslation;

/** The current UI language (re-renders on change). */
export function useLanguage(): UiLanguage {
  const { i18n: inst } = useTranslation();
  return resolveLanguage(inst.resolvedLanguage ?? inst.language);
}

/** Locale-aware formatters bound to the current language (re-renders on change). */
export function useFormat(): Formatters {
  const lng = useLanguage();
  return useMemo(() => formattersFor(lng), [lng]);
}

type LanguageListener = (lng: UiLanguage) => void;
const listeners = new Set<LanguageListener>();

/**
 * Side effects of a language change that live outside i18next (html `lang`,
 * native tray menu, design-system defaults). Registered by the app shell.
 */
export function onLanguageApplied(listener: LanguageListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

let currentPreference: LanguagePreference = toPreference(typeof window === "undefined" ? null : readCachedPreference());

/** The preference last applied (`system` until settings say otherwise). */
export function languagePreference(): LanguagePreference {
  return currentPreference;
}

/**
 * Applies a preference live (no reload): resolves `system`, switches i18next,
 * remembers it locally for the next start and notifies listeners. Persisting
 * to the core (`settings.update`) is the caller's job.
 */
export async function applyLanguagePreference(preference: string | null | undefined): Promise<UiLanguage> {
  currentPreference = toPreference(preference);
  writeCachedPreference(currentPreference);
  const lng = resolveLanguage(currentPreference);
  if (i18n.language !== lng) await i18n.changeLanguage(lng);
  for (const l of listeners) {
    try {
      l(lng);
    } catch (e) {
      console.warn("language listener failed", e);
    }
  }
  return lng;
}
