/**
 * Startup refusals of the shell (ADR-0048): `database_newer: …` means the
 * data was written by a newer Orkestr (downgrade protection). Returns a
 * localized explanation, or `null` for other errors.
 */
import { t } from "../../i18n";

export function describeBootError(error: string | null | undefined): string | null {
  if (!error) return null;
  const m = /^database_newer:\s*([\s\S]*)$/.exec(error.trim());
  if (!m) return null;
  return t("updates:downgrade.blocked", { detail: m[1] ?? "" });
}
