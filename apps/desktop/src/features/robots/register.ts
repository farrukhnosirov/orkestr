/**
 * Registers the robots feature in the shared registries: Settings → Robots
 * (with the Robot Designer) and palette commands. Imported once from App.
 */
import { Bot, Palette } from "lucide-react";

import { t } from "../../i18n";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { RobotsSettings } from "./designer/RobotsSettings";
import { openRobotDesigner, useRobotsStore } from "./store";

let registered = false;

export function registerRobotsFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push({
    id: "robots",
    get title() {
      return t("robots:settings.sectionTitle");
    },
    icon: Bot,
    order: 30,
    component: RobotsSettings,
  });
  commands.push(
    {
      id: "robots.toggle",
      get title() {
        return useRobotsStore.getState().config?.enabled === false ? t("robots:commands.show") : t("robots:commands.hide");
      },
      get group() {
        return t("robots:commands.group");
      },
      icon: Bot,
      keywords: ["robot", "floating", "desktop", "avatar", "робот", "плавающие", "рабочий стол", "аватар"],
      enabled: () => useRobotsStore.getState().config !== null,
      run: async () => {
        const store = useRobotsStore.getState();
        if (store.config) await store.patch({ enabled: !store.config.enabled });
      },
    },
    {
      id: "robots.designer",
      get title() {
        return t("robots:commands.designer");
      },
      get group() {
        return t("robots:commands.group");
      },
      icon: Palette,
      keywords: ["robot", "designer", "skin", "avatar", "робот", "дизайнер", "скин", "внешний вид"],
      run: () => openRobotDesigner(null),
    },
  );
}

export { RobotHost } from "./host/RobotHost";
export { openRobotDesigner } from "./store";
