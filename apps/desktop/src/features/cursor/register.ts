/**
 * Registers cursor capture mode (ADR-0050): Settings → Cursor mode, palette
 * commands (take control, stop) and the speaker for its Jarvis phrases.
 * Imported once from App.
 */
import { MousePointer2, OctagonX, Settings2 } from "lucide-react";

import { t } from "../../i18n";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { say } from "../voice/controller";
import { CursorSettingsSection } from "./CursorSettingsSection";
import { openCursorPanel, setCursorSpeaker, stopCursor } from "./store";

export { openCursorPanel };

let registered = false;

export function registerCursorFeature(): void {
  if (registered) return;
  registered = true;
  setCursorSpeaker((text) => void say(text));
  settingsSections.push({
    id: "cursor",
    get title() {
      return t("cursor:settings.title");
    },
    icon: MousePointer2,
    order: 306,
    component: CursorSettingsSection,
  });
  commands.push(
    {
      id: "cursor.start",
      get title() {
        return t("cursor:commands.start");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: MousePointer2,
      shortcut: "Ctrl+Alt+J",
      inTerminal: true,
      keywords: ["cursor", "mouse", "take control", "computer use", "jarvis", "курсор", "мышь", "управление", "захватить", "джарвис"],
      run: openCursorPanel,
    },
    {
      id: "cursor.stop",
      get title() {
        return t("cursor:commands.stop");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: OctagonX,
      keywords: ["cursor", "stop", "курсор", "стоп", "вернуть управление"],
      run: () => void stopCursor(),
    },
    {
      id: "cursor.settings",
      get title() {
        return t("cursor:commands.settings");
      },
      get group() {
        return t("assistant:commands.group");
      },
      icon: Settings2,
      keywords: ["cursor", "settings", "subscriptions", "api", "курсор", "настройки", "подписки", "ключи"],
      run: () => useSettingsStore.getState().openSettings("cursor"),
    },
  );
}
