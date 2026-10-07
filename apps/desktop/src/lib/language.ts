/**
 * Language switching for the shell: applies a preference live, mirrors it to
 * the parts i18next does not reach (`<html lang>`, the native tray menu, the
 * design-system defaults) and persists it to the core settings.
 */
import { setUiStrings } from "@orkestr/ui";

import { maybeClient } from "../core/client";
import { useSettingsStore } from "../features/settings/settingsStore";
import {
  applyLanguagePreference,
  currentLanguage,
  type LanguagePreference,
  onLanguageApplied,
  t,
  type UiLanguage,
} from "../i18n";
import { shellControls } from "./tauri";

/**
 * Tells the core which language the UI shows (it cannot resolve `system`
 * itself). Before the connection is open this is skipped: the client reports
 * the language on every (re)connect.
 */
export function reportUiLanguage(lng: UiLanguage): void {
  const c = maybeClient();
  if (!c || c.state !== "open") return;
  c.call("settings.setUiLanguage", { language: lng }).catch((e: unknown) => console.warn("core language", e));
}

function syncSurroundings(lng: UiLanguage): void {
  if (typeof document !== "undefined") document.documentElement.lang = lng;
  setUiStrings({ loading: t("common:state.loading") });
  shellControls.setUiLanguage(lng).catch((e: unknown) => console.warn("tray language", e));
  reportUiLanguage(lng);
}

let installed = false;

/** Installs the side effects once and applies them for the current language. */
export function installLanguageEffects(): void {
  if (installed) return;
  installed = true;
  onLanguageApplied(syncSurroundings);
  syncSurroundings(currentLanguage());
}

/**
 * Switches the UI language now and saves it (`settings.update {language}`).
 * When saving fails the UI keeps the new language for this session and the
 * settings store reports the error.
 */
export async function switchLanguage(preference: LanguagePreference): Promise<boolean> {
  await applyLanguagePreference(preference);
  const store = useSettingsStore.getState();
  if (!store.settings || store.settings.language === preference) return true;
  return store.update({ language: preference });
}

/** The other supported language (for the "Switch Language" command). */
export function nextLanguage(): UiLanguage {
  return currentLanguage() === "ru" ? "en" : "ru";
}
