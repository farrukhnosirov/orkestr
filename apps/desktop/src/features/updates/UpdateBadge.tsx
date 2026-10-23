/** Title-bar badge shown while an update is on offer; opens Settings → Updates. */
import "./updates.css";

import { ArrowDownCircle } from "lucide-react";

import { useT } from "../../i18n";
import { useSettingsStore } from "../settings/settingsStore";
import { hasUpdateBadge, useUpdatesStore } from "./store";

export function UpdateBadge() {
  const status = useUpdatesStore((s) => s.status);
  const openSettings = useSettingsStore((s) => s.openSettings);
  const { t } = useT("updates");
  if (!hasUpdateBadge(status) || !status?.available) return null;
  const ready = status.phase === "ready";
  const label = ready
    ? t("badge.ready", { version: status.available.version })
    : t("badge.available", { version: status.available.version });
  return (
    <button
      type="button"
      className="titlebar__ctl upd-badge"
      data-ready={ready || undefined}
      aria-label={label}
      title={label}
      onClick={() => openSettings("updates")}
    >
      <ArrowDownCircle size={15} />
      <span className="upd-badge__dot" aria-hidden="true" />
    </button>
  );
}
