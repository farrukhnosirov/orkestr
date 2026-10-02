/**
 * Localized error text. Core errors carry a structured `code` (contract §4);
 * the UI renders a translated label for the code and keeps the core's
 * technical detail (English, from logs-friendly messages) after it.
 */
import { OrkestrError } from "@orkestr/client";
import i18n from "i18next";

import { currentLanguage, SUPPORTED_LANGUAGES } from "./index";

/** Translated label for an RPC / client error code, or null when unknown. */
export function errorCodeLabel(code: string, lng?: string): string | null {
  const key = `common:errors.codes.${code}`;
  if (!i18n.exists(key, lng ? { lng } : undefined)) return null;
  return i18n.t(key as "common:errors.generic", lng ? { lng } : undefined);
}

/**
 * Human-readable message for any thrown value. In English the core's own
 * message is already readable and is returned as is; in other languages the
 * translated code label leads and the technical detail follows.
 */
export function describeError(e: unknown, lng: string = currentLanguage()): string {
  if (e instanceof OrkestrError) {
    const label = errorCodeLabel(e.code, lng);
    if (!label) return e.message || i18n.t("common:errors.generic", { lng });
    if (lng === SUPPORTED_LANGUAGES[0]) return e.message || label;
    return e.message ? i18n.t("common:errors.withDetail", { lng, message: label, detail: e.message }) : label;
  }
  if (e instanceof Error) return e.message || i18n.t("common:errors.generic", { lng });
  return String(e);
}
