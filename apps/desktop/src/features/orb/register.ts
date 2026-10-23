/**
 * Registers the orb feature (ADR-0045): Settings → Orb and palette
 * commands (show / hide the orb, toggle the Agent Hub with a hotkey).
 */
import { CircleDot, LayoutPanelLeft } from "lucide-react";

import { t } from "../../i18n";
import { useAiUiStore } from "../aiui/aiuiStore";
import { prefsOf } from "../aiui/prefs";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { ORB_HUB_SHORTCUT, orbPrefsOf } from "./model";
import { OrbSettings } from "./OrbSettings";
import { orbShell } from "./shell";

export { ORB_HUB_SHORTCUT };

const orbEnabled = () => orbPrefsOf(prefsOf(useAiUiStore.getState().config).orb).enabled;

const setEnabled = (on: boolean) =>
  useAiUiStore.getState().applyUserPatch([{ op: "replace", path: "/prefs/orb/enabled", value: on }], on ? t("orb:summaries.shown") : t("orb:summaries.hidden"));

let registered = false;

export function registerOrbFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push({
    id: "orb",
    get title() {
      return t("orb:settings.sectionTitle");
    },
    icon: CircleDot,
    order: 30,
    component: OrbSettings,
  });
  commands.push(
    {
      id: "orb.toggle",
      get title() {
        return orbEnabled() ? t("orb:commands.hide") : t("orb:commands.show");
      },
      get group() {
        return t("orb:commands.group");
      },
      icon: CircleDot,
      keywords: ["orb", "sphere", "jarvis", "floating", "agents", "сфера", "шар", "джарвис", "агенты", "плавающий"],
      enabled: () => useAiUiStore.getState().config !== null,
      run: async () => {
        await setEnabled(!orbEnabled());
      },
    },
    {
      id: "orb.hub",
      get title() {
        return t("orb:commands.hub");
      },
      get group() {
        return t("orb:commands.group");
      },
      icon: LayoutPanelLeft,
      keywords: ["orb", "hub", "agents", "stop", "approve", "хаб", "агенты", "остановить", "одобрить"],
      shortcut: ORB_HUB_SHORTCUT,
      enabled: () => orbShell.available(),
      run: async () => {
        if (!orbEnabled()) {
          await setEnabled(true);
          // The host opens the window on the next render; give it a moment.
          await new Promise((r) => window.setTimeout(r, 600));
        }
        await orbShell.toggleHub();
      },
    },
  );
}

export { OrbHost } from "./OrbHost";
