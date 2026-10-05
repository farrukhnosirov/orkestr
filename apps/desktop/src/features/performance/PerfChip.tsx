/**
 * Status-bar chip with the effective Performance Mode; marks automatic
 * switches (battery, frame budget) and opens Settings → Interface.
 */
import { clsx } from "clsx";
import { Gauge } from "lucide-react";

import { useT } from "../../i18n";
import { useSettingsStore } from "../settings/settingsStore";
import { usePerfStore } from "./perfStore";
import "./performance.css";

export const VISUAL_SECTION_ID = "interface";

export function PerfChip() {
  const mode = usePerfStore((s) => s.mode);
  const reason = usePerfStore((s) => s.reason);
  const openSettings = useSettingsStore((s) => s.openSettings);
  const { t } = useT("performance");
  const name = t(`modes.${mode}.name`);
  const auto = reason !== "user";
  return (
    <button
      type="button"
      className={clsx("perf-chip", auto && "perf-chip--auto")}
      title={auto ? `${t("statusChipTitle")} — ${t(`effective.${reason}`)}` : t("statusChipTitle")}
      onClick={() => openSettings(VISUAL_SECTION_ID)}
    >
      <Gauge size={11} aria-hidden="true" />
      <span>{auto ? t("statusChipAuto", { mode: name }) : t("statusChip", { mode: name })}</span>
    </button>
  );
}
