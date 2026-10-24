/**
 * Slim title bar of the Assistant-mode shell (ADR-0035): drag region, logo +
 * "Assistant", compact mic / wake indicator, emergency Stop (icon), switch to
 * Orchestra mode, settings and the window controls (minimize, close — close
 * behaves like the main window's: hide to tray when that is on). Fits windows
 * from 400 px wide.
 */
import { clsx } from "clsx";
import { LayoutDashboard, Minus, OctagonX, Settings, X } from "lucide-react";

import { Logo } from "../../components/Logo";
import { useT } from "../../i18n";
import { isTauri, windowControls } from "../../lib/tauri";
import { STOP_SHORTCUT } from "../security/EmergencyStop";
import { useSecurityStore } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import { MicIndicator } from "../voice";
import { useAssistantStore } from "./store";
import "./assistant.css";

export function AssistantTitleBar() {
  const { t } = useT("assistant");
  const { t: tShell } = useT("shell");
  const { t: tSec } = useT("security");
  const setMode = useAssistantStore((s) => s.setMode);
  const openSettings = useSettingsStore((s) => s.openSettings);
  const setConfirmStop = useSecurityStore((s) => s.setConfirmStopOpen);
  const stopped = useSecurityStore((s) => s.flags?.allStopped ?? false);
  const native = isTauri();
  const stopTitle = stopped ? tSec("emergency.buttonEngagedTitle") : tSec("emergency.buttonTitle", { shortcut: STOP_SHORTCUT });

  return (
    <header className="as-titlebar" data-tauri-drag-region data-testid="assistant-titlebar">
      <div className="as-titlebar__brand" data-tauri-drag-region>
        <Logo size={18} />
        <span className="as-titlebar__name" data-tauri-drag-region>
          {t("bar.title")}
        </span>
      </div>
      <div className="as-titlebar__drag" data-tauri-drag-region />
      <div className="as-titlebar__tools">
        <MicIndicator />
        <button
          type="button"
          className={clsx("as-titlebar__btn as-titlebar__stop", stopped && "as-titlebar__stop--engaged")}
          aria-label={stopped ? tSec("emergency.buttonEngagedAria") : tSec("emergency.buttonAria")}
          title={stopTitle}
          onClick={() => setConfirmStop(true)}
        >
          <OctagonX size={15} />
        </button>
        <button
          type="button"
          className="as-titlebar__btn"
          aria-label={t("mode.toOrchestra")}
          title={t("mode.orchestraHint", { keys: "Ctrl+Alt+A" })}
          onClick={() => setMode("orchestra")}
        >
          <LayoutDashboard size={15} />
        </button>
        <button type="button" className="as-titlebar__btn" aria-label={tShell("titleBar.settings")} title={tShell("titleBar.settings")} onClick={() => openSettings()}>
          <Settings size={15} />
        </button>
      </div>
      {native && (
        <div className="as-titlebar__win">
          <button type="button" className="as-titlebar__win-btn" aria-label={tShell("titleBar.minimize")} title={tShell("titleBar.minimize")} onClick={() => void windowControls.minimize()}>
            <Minus size={15} />
          </button>
          <button
            type="button"
            className="as-titlebar__win-btn as-titlebar__win-btn--close"
            aria-label={tShell("titleBar.close")}
            title={tShell("titleBar.close")}
            onClick={() => void windowControls.close()}
          >
            <X size={16} />
          </button>
        </div>
      )}
    </header>
  );
}
