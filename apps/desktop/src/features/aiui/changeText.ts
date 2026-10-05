/**
 * Localized one-line descriptions of classified UI changes (see `patch.ts`).
 */
import { formatList, formatPercent, i18n, t } from "../../i18n";
import { humanizeId, idKey, type ChangeSummary } from "./patch";

const HUD_FIELDS = ["opacity", "showAgents", "showManagerInput"] as const;
const EFFECTS = ["blurPx", "glowIntensity", "panelOpacity", "animations"] as const;
const PRESETS = ["Coding", "MissionControl", "Monitoring", "Focus", "Custom"] as const;
const PERF = ["UltraVisual", "Balanced", "Performance", "BatterySaver"] as const;

function pick<T extends string>(list: readonly T[], v: string): T | null {
  return list.find((x) => x === v) ?? null;
}

/** Display name of a panel / widget id. */
export function idLabel(id: string): string {
  const key = `aiui:ids.${idKey(id)}`;
  if (i18n.exists(key)) return t(key as "aiui:ids.claude");
  return t("aiui:quoted", { name: humanizeId(id) });
}

function percent(v: number): string {
  return formatPercent(v);
}

/** The line for one change. */
export function changeText(c: ChangeSummary): string {
  switch (c.kind) {
    case "density":
      return t("aiui:change.density", { value: t(c.to === "Compact" ? "aiui:density.Compact" : "aiui:density.Comfortable") });
    case "panel": {
      const name = idLabel(c.id);
      if (c.hidden) return t("aiui:change.panelHidden", { name });
      if (c.side) return t("aiui:change.panelSide", { name, side: t(`aiui:side.${c.side}`) });
      return t("aiui:change.panelShown", { name });
    }
    case "panelReset":
      return t("aiui:change.panelReset", { name: idLabel(c.id) });
    case "widgetsHidden":
      return t("aiui:change.widgetsHidden", { count: c.ids.length, names: formatList(c.ids.map(idLabel)) });
    case "widgetsShown":
      return t("aiui:change.widgetsShown", { count: c.ids.length, names: formatList(c.ids.map(idLabel)) });
    case "backgroundDim":
      return t(c.to >= c.from ? "aiui:change.backgroundDarker" : "aiui:change.backgroundLighter", { percent: percent(c.to) });
    case "hud": {
      const f = pick(HUD_FIELDS, c.field);
      return t("aiui:change.hud", { field: t(f ? `aiui:hudField.${f}` : "aiui:hudField.unknown") });
    }
    case "token":
      return c.to === null ? t("aiui:change.tokenRemoved", { name: c.name }) : t("aiui:change.token", { name: c.name });
    case "effect": {
      const f = pick(EFFECTS, c.field);
      return t("aiui:change.effect", { name: t(f ? `aiui:effect.${f}` : "aiui:effect.unknown") });
    }
    case "themeMode":
      return t("aiui:change.themeMode", { value: t(c.to === "Light" ? "aiui:themeMode.Light" : "aiui:themeMode.Dark") });
    case "layoutPreset": {
      const p = pick(PRESETS, c.to) ?? "Custom";
      return t("aiui:change.layoutPreset", { value: t(`aiui:preset.${p}`) });
    }
    case "performanceMode": {
      const m = pick(PERF, c.to) ?? "Balanced";
      return t("aiui:change.performanceMode", { value: t(`performance:modes.${m}.name`) });
    }
    case "other":
      return t("aiui:change.other", { path: c.path });
  }
}
