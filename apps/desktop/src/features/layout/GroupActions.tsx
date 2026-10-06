/**
 * Header actions of a dock group (dockview `rightHeaderActionsComponent`):
 * float / dock back, detach into a window, maximize / restore.
 */
import type { IDockviewHeaderActionsProps } from "dockview-react";
import { AppWindow, Maximize2, Minimize2, PictureInPicture2, SquareArrowDownLeft } from "lucide-react";
import { useEffect, useState } from "react";

import { useT } from "../../i18n";
import * as layout from "./controller";
import { useLayoutStore } from "./store";
import { canDetach } from "./windowsApi";

export function GroupActions({ group, containerApi, activePanel }: IDockviewHeaderActionsProps) {
  const { t } = useT("layout");
  const locked = useLayoutStore((s) => s.locked);
  const [maximized, setMaximized] = useState(() => containerApi.hasMaximizedGroup() && group.api.isMaximized());
  const [floating, setFloating] = useState(group.api.location.type === "floating");

  useEffect(() => {
    const a = containerApi.onDidMaximizedGroupChange(() => setMaximized(group.api.isMaximized()));
    const b = group.api.onDidLocationChange((e) => setFloating(e.location.type === "floating"));
    return () => {
      a.dispose();
      b.dispose();
    };
  }, [containerApi, group]);

  const panelId = activePanel?.id;
  if (!panelId) return null;
  return (
    <div className="dock-actions" onMouseDown={(e) => e.stopPropagation()}>
      {!locked && (
        <>
          {floating ? (
            <button type="button" className="dock-actions__btn" title={t("actions.dock")} aria-label={t("actions.dock")} onClick={() => layout.dockPanel(panelId)}>
              <SquareArrowDownLeft size={13} />
            </button>
          ) : (
            <button type="button" className="dock-actions__btn dock-actions__btn--secondary" title={t("actions.float")} aria-label={t("actions.float")} onClick={() => layout.floatPanel(panelId)}>
              <PictureInPicture2 size={13} />
            </button>
          )}
          {canDetach() && (
            <button type="button" className="dock-actions__btn dock-actions__btn--secondary" title={t("actions.detach")} aria-label={t("actions.detach")} onClick={() => void layout.detachPanel(panelId)}>
              <AppWindow size={13} />
            </button>
          )}
        </>
      )}
      {!floating && (
        <button
          type="button"
          className="dock-actions__btn"
          title={maximized ? t("actions.restore") : t("actions.maximize")}
          aria-label={maximized ? t("actions.restore") : t("actions.maximize")}
          onClick={() => layout.toggleMaximize(panelId)}
        >
          {maximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
        </button>
      )}
    </div>
  );
}
