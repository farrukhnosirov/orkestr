/**
 * Registers the computer assistant (ADR-0035): Settings → Assistant and
 * palette commands (mode switch Ctrl+Alt+A, stop, command sets). Imported
 * once from App. The mode switch also exists in the title bar and by voice
 * ("режим ассистента" / "assistant mode", catalogue `mode.*`).
 */
import { Bot, LayoutDashboard, OctagonX, Wand2 } from "lucide-react";
import { useEffect } from "react";

import { t } from "../../i18n";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { refreshHistory, stopAssistant } from "./api";
import { CommandSetsSection } from "./CommandSetsSection";
import { useAssistantStore } from "./store";
import { applyWindowMode } from "./windowMode";

let registered = false;

export function registerAssistantFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push({
    id: "assistant",
    get title() {
      return t("assistant:settings.title");
    },
    icon: Wand2,
    order: 305,
    component: CommandSetsSection,
  });
  commands.push(
    {
      id: "assistant.toggleMode",
      get title() {
        return useAssistantStore.getState().mode === "assistant" ? t("assistant:commands.toOrchestra") : t("assistant:commands.toAssistant");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: Bot,
      shortcut: "Ctrl+Alt+A",
      inTerminal: true,
      keywords: ["assistant", "orchestra", "mode", "computer", "jarvis", "ассистент", "оркестр", "режим", "компьютер"],
      run: () => useAssistantStore.getState().toggleMode(),
    },
    {
      id: "assistant.stop",
      get title() {
        return t("assistant:commands.stop");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: OctagonX,
      keywords: ["stop", "cancel", "timer", "стоп", "отмена", "таймер", "ассистент"],
      run: () => stopAssistant(),
    },
    {
      id: "assistant.sets",
      get title() {
        return t("assistant:commands.sets");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: LayoutDashboard,
      keywords: ["commands", "catalogue", "laitis", "sets", "команды", "каталог", "наборы команд"],
      run: () => useSettingsStore.getState().openSettings("assistant"),
    },
  );
}

/** Applies the window geometry of the current mode and keeps it in sync. */
export function useAssistantMode(ready: boolean): void {
  const mode = useAssistantStore((s) => s.mode);
  useEffect(() => {
    void applyWindowMode(mode);
    document.documentElement.dataset["okUiMode"] = mode;
    if (ready && mode === "assistant") void refreshHistory();
  }, [mode, ready]);
}
