/**
 * Registers the theme feature in the shared registries: Settings →
 * Appearance and the palette commands. Imported once (App) for its side effects.
 */
import { Brush, Paintbrush } from "lucide-react";

import { t } from "../../i18n";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { openThemeEditor } from "./actions";
import { AppearanceSection } from "./AppearanceSection";

let registered = false;

export function registerThemeFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push({
    id: "appearance",
    get title() {
      return t("theme:section");
    },
    icon: Paintbrush,
    order: 5,
    component: AppearanceSection,
  });
  commands.push(
    {
      id: "theme.openEditor",
      get title() {
        return t("theme:commands.openEditor");
      },
      get group() {
        return t("theme:commands.group");
      },
      icon: Brush,
      keywords: ["theme", "editor", "colors", "appearance", "customize", "тема", "редактор", "цвета", "оформление", "внешний вид"],
      run: () => openThemeEditor(),
    },
    {
      id: "theme.openAppearance",
      get title() {
        return t("theme:commands.openAppearance");
      },
      get group() {
        return t("theme:commands.group");
      },
      icon: Paintbrush,
      keywords: ["appearance", "themes", "gallery", "оформление", "темы", "галерея"],
      run: () => useSettingsStore.getState().openSettings("appearance"),
    },
  );
}
