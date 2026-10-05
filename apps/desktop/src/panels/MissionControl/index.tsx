/**
 * Mission Control (§32, §68.22): the main operations screen. Mounted as a
 * dockable panel and, via Ctrl+Shift+D, as a full-screen layout over the
 * workspace (`MissionControlOverlay`, mounted once in `App.tsx`).
 */
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef } from "react";

import { useT } from "../../i18n";
import type { PanelProps } from "../types";
import { MissionDashboard } from "./Dashboard";
import "./missionControl.css";
import { useMissionControlStore } from "./store";
import { useVisualBudget } from "./visual";

export { setArrangementStorage, type ArrangementStorage } from "./arrangement";
export { registerWidget, widgets, type WidgetManifest } from "./widgets/registry";

/** Mission Control as a dockable panel. */
export default function MissionControlPanel(_props: PanelProps) {
  return <MissionDashboard variant="panel" />;
}

/** Full-screen Mission Control (Ctrl+Shift+D, Esc to leave). */
export function MissionControlOverlay() {
  const { t } = useT("missionControl");
  const open = useMissionControlStore((s) => s.fullscreen);
  const budget = useVisualBudget();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return undefined;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      // Let open dialogs / menus inside the overlay handle Esc first.
      if (document.querySelector("[role='dialog'][aria-modal='true']")) return;
      e.preventDefault();
      useMissionControlStore.getState().setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus();
    };
  }, [open]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="mc-overlay"
          ref={ref}
          className="mc-overlay"
          role="region"
          aria-label={t("overlay.label")}
          tabIndex={-1}
          initial={budget.animate ? { opacity: 0, scale: 0.985 } : false}
          animate={{ opacity: 1, scale: 1 }}
          exit={budget.animate ? { opacity: 0, scale: 0.985 } : { opacity: 0 }}
          transition={{ duration: budget.animate ? 0.22 : 0, ease: [0.2, 0.8, 0.2, 1] }}
        >
          <MissionDashboard variant="fullscreen" />
        </motion.div>
      )}
    </AnimatePresence>
  );
}
