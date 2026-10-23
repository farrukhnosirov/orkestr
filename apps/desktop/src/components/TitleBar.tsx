import { Kbd } from "@orkestr/ui";
import { Copy, LibraryBig, Minus, PictureInPicture2, Search, Settings, Square, X } from "lucide-react";
import { useEffect, useState } from "react";

import { ModeToggle } from "../features/assistant/ModeToggle";
import { useAssistantStore } from "../features/assistant/store";
import { EnginesIndicator } from "../features/engines/EnginesIndicator";
import { LayoutSwitcher } from "../features/layout/LayoutSwitcher";
import { hudControls } from "../features/hud/hudWindow";
import { NotificationBell } from "../features/notifications/NotificationCenter";
import { ApprovalsBadge } from "../features/security/ApprovalsInbox";
import { EmergencyStopButton } from "../features/security/EmergencyStop";
import { useSettingsStore } from "../features/settings/settingsStore";
import { FileMenu } from "../features/workbench/FileMenu";
import { UpdateBadge } from "../features/updates";
import { MicIndicator } from "../features/voice";
import { openWindowLibrary, WINDOW_LIBRARY_SHORTCUT } from "../features/windowLibrary";
import { useT } from "../i18n";
import { isTauri, windowControls } from "../lib/tauri";
import { useActiveProject, useAppStore } from "../store/app";
import { Logo } from "./Logo";

/** Custom frameless title bar (§28): drag region, palette trigger, window controls. */
export function TitleBar() {
  const project = useActiveProject();
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  const openSettings = useSettingsStore((s) => s.openSettings);
  const [maximized, setMaximized] = useState(false);
  const native = isTauri();
  // Assistant mode (ADR-0035): compact bar — brand, mode switch, stop, window controls.
  const compact = useAssistantStore((s) => s.mode === "assistant");
  const { t } = useT("shell");
  const { t: tHud } = useT("hud");
  const { t: tLib } = useT("windowLibrary");

  useEffect(() => {
    if (!native) return;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      windowControls.isMaximized().then(setMaximized, () => {});
    };
    sync();
    windowControls.onResized(sync).then((u) => {
      unlisten = u;
    }, () => {});
    return () => unlisten?.();
  }, [native]);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        <Logo size={20} />
        <span className="titlebar__wordmark" data-tauri-drag-region>
          Orkestr
        </span>
        {project && !compact && (
          <>
            <span className="titlebar__sep" data-tauri-drag-region>
              /
            </span>
            <span className="titlebar__project" title={project.name} data-tauri-drag-region>
              <span className="titlebar__project-dot" style={{ background: project.color ?? undefined }} />
              <span className="titlebar__project-name" data-tauri-drag-region>
                {project.name}
              </span>
            </span>
          </>
        )}
      </div>

      <ModeToggle />

      {/* VS Code-style File menu (project & file management). */}
      {!compact && <FileMenu />}

      {!compact && (
        <button
          type="button"
          className="titlebar__search"
          aria-label={t("titleBar.search")}
          title={t("titleBar.search")}
          onClick={() => setPaletteOpen(true)}
        >
          <Search size={14} />
          <span>{t("titleBar.search")}</span>
          <span className="titlebar__search-keys">
            <Kbd>Ctrl</Kbd>
            <Kbd>K</Kbd>
          </span>
        </button>
      )}

      {/* Window Library (Wave 8): gallery of every panel and window */}
      <button
        type="button"
        className="titlebar__ctl titlebar__library"
        aria-label={tLib("open")}
        title={`${tLib("title")} (${WINDOW_LIBRARY_SHORTCUT})`}
        onClick={openWindowLibrary}
      >
        <LibraryBig size={15} />
      </button>

      {/* layout-multimonitor (Phase 3): layout presets / editor */}
      {!compact && <LayoutSwitcher />}

      <div className="titlebar__spacer" data-tauri-drag-region />

      <div className="sec-titlebar-group">
        {!compact && <ApprovalsBadge />}
        <EmergencyStopButton />
      </div>

      <div className="titlebar__controls">
        <UpdateBadge />
        {/* Claude / Codex subscription status (parallel engines) */}
        {!compact && <EnginesIndicator />}
        <MicIndicator />
        {!compact && <NotificationBell />}
        {native && !compact && (
          <button
            type="button"
            className="titlebar__ctl titlebar__hud"
            aria-label={tHud("titleBar.collapse")}
            title={tHud("titleBar.collapseTitle", { keys: "Ctrl+Shift+H" })}
            onClick={() => void hudControls.collapse()}
          >
            <PictureInPicture2 size={15} />
          </button>
        )}
        <button type="button" className="titlebar__ctl" aria-label={t("titleBar.settings")} title={t("titleBar.settings")} onClick={() => openSettings()}>
          <Settings size={15} />
        </button>
      </div>

      {native && (
        <div className="titlebar__controls">
          <button type="button" className="titlebar__ctl" aria-label={t("titleBar.minimize")} onClick={() => void windowControls.minimize()}>
            <Minus size={15} />
          </button>
          {!compact && (
            <button
              type="button"
              className="titlebar__ctl"
              aria-label={maximized ? t("titleBar.restore") : t("titleBar.maximize")}
              onClick={() => void windowControls.toggleMaximize()}
            >
              {maximized ? <Copy size={13} style={{ transform: "scaleX(-1)" }} /> : <Square size={12} />}
            </button>
          )}
          <button
            type="button"
            className="titlebar__ctl titlebar__ctl--close"
            aria-label={t("titleBar.close")}
            onClick={() => void windowControls.close()}
          >
            <X size={16} />
          </button>
        </div>
      )}
    </header>
  );
}
