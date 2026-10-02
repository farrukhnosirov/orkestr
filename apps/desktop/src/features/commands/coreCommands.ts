/**
 * Core shell commands (§54): project, settings, views, window, safety.
 * Shared by the command palette, keyboard shortcuts and (Phase 3) voice.
 */
import {
  Activity,
  BellOff,
  Expand,
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
import { stopAllAi as stopAllAiNow, useSecurityStore } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import type { AppCommand } from "./registry";

/**
 * Emergency stop of all AI activity (§52): `emergency.stopAll`, handled
 * locally in the core. Used by the tray, which acts without a confirmation
 * dialog (the user explicitly picked "Stop All AI"). Returns whether the
 * stop was executed.
 */
export async function stopAllAi(): Promise<boolean> {
  const notify = useNotificationsStore.getState().notify;
  try {
    const r = await stopAllAiNow();
    notify(
      "Critical",
      "All AI activity stopped",
      `Emergency stop executed by you: ${r.agentsStopped} agent(s) stopped, ${r.approvalsCancelled} pending approval(s) cancelled.`,
    );
    return true;
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
    // Ctrl+P is "Go to File…" (files area), as in common editors.
    shortcut: "Ctrl+Alt+P",
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
    id: "view.toggleMaximize",
    title: "Toggle Maximize Panel",
    group: "Views",
    icon: Expand,
    keywords: ["maximize", "zoom", "focus", "panel", "restore"],
    run: () => workspaceActions.toggleMaximize(),
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
    // Opens the security confirmation dialog (stop + re-arm live there).
    run: () => useSecurityStore.getState().setConfirmStopOpen(true),
  },
];
