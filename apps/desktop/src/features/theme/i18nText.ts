/**
 * Localized text for theme data. Built-in theme names are brand names and
 * stay as they are; built-in descriptions are translated. User themes show
 * exactly what the user typed.
 */
import type { ThemeConfig } from "@orkestr/protocol";
import { isBuiltinTheme } from "@orkestr/ui";

import { t } from "../../i18n";

const BUILTIN_IDS = ["nebula", "midnight-pro", "aurora", "carbon", "solar-light", "high-contrast"] as const;
type BuiltinId = (typeof BUILTIN_IDS)[number];

const isKnownBuiltin = (id: string): id is BuiltinId => (BUILTIN_IDS as readonly string[]).includes(id);

export function themeDescription(theme: ThemeConfig, builtin = isBuiltinTheme(theme.id)): string {
  if (builtin && isKnownBuiltin(theme.id)) return t(`theme:builtin.${theme.id}`);
  return theme.description;
}

/** Triggers a download of a text file (theme export). */
export function downloadText(fileName: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Reads a user-picked file as text. */
export function readFileText(file: File): Promise<string> {
  return file.text();
}
