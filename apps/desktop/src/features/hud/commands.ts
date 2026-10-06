/**
 * Commands of the HUD / Performance / AI-editable UI features (§54): toggle
 * and collapse into the HUD, pick a performance mode, undo / redo the last
 * UI change. Appended to the command registry (palette, shortcuts, voice).
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { Gauge, PanelTopClose, PictureInPicture2, Redo2, Undo2 } from "lucide-react";
import { useEffect } from "react";

import { t } from "../../i18n";
import { useAiUiStore } from "../aiui/aiuiStore";
import { canRedo, canUndo } from "../aiui/undo";
import { matchesShortcut } from "../commands/keys";
import type { AppCommand } from "../commands/registry";
import { PERF_MODES } from "../performance/policy";
import { useSettingsStore } from "../settings/settingsStore";
import { hudControls } from "./hudWindow";

/** Shortcut that toggles the HUD (works in the main window and in the HUD itself). */
export const HUD_SHORTCUT = "Ctrl+Shift+H";

function perfCommand(mode: PerformanceMode): AppCommand {
  return {
    id: `performance.mode.${mode}`,
    get title() {
      return t("performance:commands.setMode", { mode: t(`performance:modes.${mode}.name`) });
    },
    get group() {
      return t("hud:commands.group");
    },
    icon: Gauge,
    keywords: ["performance", "mode", "battery", "visual", "производительность", "режим", "батарея", "эффекты"],
    enabled: () => useSettingsStore.getState().settings !== null,
    run: () => {
      void useSettingsStore.getState().update({ performanceMode: mode });
    },
  };
}

export const hudCommands: AppCommand[] = [
  {
    id: "hud.toggle",
    get title() {
      return t("hud:commands.toggle");
    },
    get group() {
      return t("hud:commands.group");
    },
    icon: PictureInPicture2,
    keywords: ["hud", "ambient", "overlay", "mini", "always on top", "мини", "поверх окон", "компактный"],
    shortcut: HUD_SHORTCUT,
    enabled: () => hudControls.available(),
    run: () => {
      void hudControls.toggle();
    },
  },
  {
    id: "hud.collapse",
    get title() {
      return t("hud:commands.collapse");
    },
    get group() {
      return t("hud:commands.group");
    },
    icon: PanelTopClose,
    keywords: ["hud", "collapse", "minimize", "ambient", "свернуть", "мини"],
    enabled: () => hudControls.available(),
    run: () => {
      void hudControls.collapse();
    },
  },
  ...PERF_MODES.map(perfCommand),
  {
    id: "ui.undo",
    get title() {
      return t("performance:commands.undoUi");
    },
    get group() {
      return t("hud:commands.group");
    },
    icon: Undo2,
    keywords: ["undo", "ui", "interface", "revert", "отменить", "интерфейс"],
    enabled: () => canUndo(useAiUiStore.getState().undo),
    run: () => {
      void useAiUiStore.getState().undoLast();
    },
  },
  {
    id: "ui.redo",
    get title() {
      return t("performance:commands.redoUi");
    },
    get group() {
      return t("hud:commands.group");
    },
    icon: Redo2,
    keywords: ["redo", "ui", "interface", "повторить", "интерфейс"],
    enabled: () => canRedo(useAiUiStore.getState().undo),
    run: () => {
      void useAiUiStore.getState().redoLast();
    },
  },
];

/** The HUD window's own shortcut (the main window dispatches through the registry). */
export function useHudShortcut(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (matchesShortcut(e, "Ctrl+Shift+H")) {
        e.preventDefault();
        void hudControls.hide();
      } else if (e.key === "Escape" && !e.defaultPrevented && !(e.target instanceof HTMLInputElement && e.target.value)) {
        void hudControls.hide();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
