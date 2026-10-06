/**
 * Display text of a speech bubble. The bubble spec carries the core's raw
 * values (an agent's activity line is stored in English); they are
 * localized here, at display time, so a language switch also applies.
 */
import { t } from "../../../i18n";
import { agentActivity } from "../../../lib/coreText";
import type { BubbleSpec } from "./bubble";

/** Interpolation values of a bubble with the core's activity strings localized. */
export function bubbleValues(spec: BubbleSpec): Record<string, string | number> {
  const values: Record<string, string | number> = { ...spec.values };
  for (const k of ["activity", "detail"] as const) {
    const v = values[k];
    if (typeof v === "string") values[k] = agentActivity(v);
  }
  return values;
}

/** The text a bubble shows (and a robot with its voice on says). */
export function bubbleText(spec: BubbleSpec): string {
  return t(`robots:bubble.${spec.key}`, bubbleValues(spec));
}
