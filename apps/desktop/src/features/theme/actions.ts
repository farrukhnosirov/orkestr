/** UI actions shared by the section, the palette and the editor. */
import type { ThemeConfig } from "@orkestr/protocol";

import { useSettingsStore } from "../settings/settingsStore";
import { useThemeStore } from "./themeStore";

/** Opens the editor over the live app (closes the Settings overlay so the preview is visible). */
export function openThemeEditor(theme?: ThemeConfig): void {
  useSettingsStore.getState().closeSettings();
  useThemeStore.getState().openEditor(theme);
}

/** Opens a theme file in the editor (not saved until the user saves). */
export function importThemeIntoEditor(json: string): { ok: true } | { ok: false; issues: string[] } {
  const r = useThemeStore.getState().importIntoEditor(json);
  if (r.ok) useSettingsStore.getState().closeSettings();
  return r;
}
