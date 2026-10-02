/**
 * Core shell commands (§54): project, settings, views, window, safety.
 * Shared by the command palette, keyboard shortcuts and (Phase 3) voice.
 */
import {
  Activity,
  BellOff,
  FolderPlus,
  FolderSearch,
  Maximize,
  OctagonX,
  ScrollText,
  Settings,
  Wand2,
} from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { shellControls } from "../../lib/tauri";
import { useAppStore } from "../../store/app";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { useOnboardingStore } from "../onboarding/onboardingStore";
import { SEARCH_PREFIX, usePaletteStore } from "../search/paletteStore";
import { callOptional } from "../settings/optionalRpc";
import { useSettingsStore } from "../settings/settingsStore";
import type { AppCommand } from "./registry";

/**
 * Emergency stop of all AI activity (§52) — `emergency.stopAll` when the
 * core implements it. Returns whether the stop was executed.
 */
export async function stopAllAi(): Promise<boolean> {
  const notify = useNotificationsStore.getState().notify;
  try {
    const r = await callOptional("emergency.stopAll", {});
    if (r.available) {
      notify("Critical", "All AI activity stopped", "Emergency stop executed by you.");
      return true;
    }
    notify("Info", "Nothing to stop", "No agents can run in this build yet (emergency.stopAll is not available).");
    return false;
  } catch (e) {
    notify("Error", "Emergency stop failed", e instanceof Error ? e.message : String(e));
    return false;
  }
}

export const coreCommands: AppCommand[] = [
  {
    id: "project.create",
    title: "Create Project",
    group: "Projects",
    icon: FolderPlus,
    keywords: ["new", "add", "import", "folder"],
    run: () => useAppStore.getState().setCreateProjectOpen(true),
  },
  {
    id: "project.switch",
    title: "Switch Project…",
    group: "Projects",
    icon: FolderSearch,
    keywords: ["open", "go to", "search", "find"],
    shortcut: "Ctrl+P",
    run: () => usePaletteStore.getState().openPalette(SEARCH_PREFIX),
  },
  {
    id: "settings.open",
    title: "Open Settings",
    group: "General",
    icon: Settings,
    keywords: ["preferences", "options", "configure", "tray", "autostart"],
    shortcut: "Ctrl+,",
    run: () => useSettingsStore.getState().openSettings(),
  },
  {
    id: "onboarding.open",
    title: "Run Setup Wizard",
    group: "General",
    icon: Wand2,
    keywords: ["onboarding", "welcome", "first run", "checks"],
    run: () => useOnboardingStore.getState().openWizard(0),
  },
  {
    id: "notifications.toggleDnd",
    title: "Toggle Do Not Disturb",
    group: "General",
    icon: BellOff,
    keywords: ["notifications", "quiet", "mute", "dnd"],
    enabled: () => useSettingsStore.getState().settings !== null,
    run: async () => {
      const s = useSettingsStore.getState().settings;
      if (s) await useSettingsStore.getState().update({ doNotDisturb: !s.doNotDisturb });
    },
  },
  {
    id: "view.missionControl",
    title: "Mission Control",
    group: "Views",
    icon: Activity,
    keywords: ["monitoring", "overview", "dashboard", "phase 3"],
    run: () => workspaceActions.openPanel("monitoring"),
  },
  {
    id: "view.logs",
    title: "Open Logs",
    group: "Views",
    icon: ScrollText,
    keywords: ["events", "log", "debug", "stream"],
    run: () => workspaceActions.openPanel("logs"),
  },
  {
    id: "window.fullscreen",
    title: "Toggle Fullscreen",
    group: "Window",
    icon: Maximize,
    keywords: ["full screen", "maximize", "zen"],
    shortcut: "F11",
    run: () => shellControls.toggleFullscreen(),
  },
  {
    id: "ai.stopAll",
    title: "Stop All AI",
    group: "Safety",
    icon: OctagonX,
    keywords: ["emergency", "kill", "halt", "panic", "stop agents"],
    shortcut: "Ctrl+Shift+Backspace",
    run: async () => {
      await stopAllAi();
    },
  },
];
