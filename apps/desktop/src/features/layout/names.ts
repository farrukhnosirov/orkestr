/** Localized names of layouts (built-ins are translated; user layouts keep their name). */
import { t } from "../../i18n";
import { isBuiltinLayoutId } from "./schema";

const KEYS = {
  coding: "coding",
  "mission-control": "missionControl",
  monitoring: "monitoring",
  focus: "focus",
  review: "review",
  custom: "custom",
} as const;

/** i18n key segment of a built-in preset (`mission-control` → `missionControl`). */
export function presetKey(id: keyof typeof KEYS): (typeof KEYS)[keyof typeof KEYS] {
  return KEYS[id];
}

export function layoutName(id: string, storedName?: string): string {
  if (isBuiltinLayoutId(id)) return t(`layout:presets.${presetKey(id)}.name`);
  return storedName ?? id;
}

export function layoutDescription(id: string): string | null {
  return isBuiltinLayoutId(id) ? t(`layout:presets.${presetKey(id)}.description`) : null;
}
