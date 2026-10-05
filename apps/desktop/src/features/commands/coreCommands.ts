/**
 * Core shell commands (§54): project, settings, views, window, safety.
 * Shared by the command palette, keyboard shortcuts and (Phase 3) voice.
 */
import {
  Activity,
  BellOff,
  Languages,
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
import { useMissionControlStore } from "../../panels/MissionControl/store";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { nextLanguage, switchLanguage } from "../../lib/language";
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
      t("shell:stopAll.doneTitle"),
      t("shell:stopAll.doneBody", {
        agents: t("shell:stopAll.agents", { count: r.agentsStopped }),
        approvals: t("shell:stopAll.approvals", { count: r.approvalsCancelled }),
      }),
    );
    return true;
  } catch (e) {
    notify("Error", t("shell:stopAll.failedTitle"), describeError(e));
    return false;
  }
}

export const coreCommands: AppCommand[] = [
  {
    id: "project.create",
    get title() {
      return t("shell:commands.projectCreate");
    },
    get group() {
      return t("shell:commandGroups.projects");
    },
    icon: FolderPlus,
    keywords: ["new", "add", "import", "folder", "создать", "новый", "проект", "папка"],
    run: () => useAppStore.getState().setCreateProjectOpen(true),
  },
  {
    id: "project.switch",
    get title() {
      return t("shell:commands.projectSwitch");
    },
    get group() {
      return t("shell:commandGroups.projects");
    },
    icon: FolderSearch,
    keywords: ["open", "go to", "search", "find", "открыть", "перейти", "проект", "найти"],
    // Ctrl+P is "Go to File…" (files area), as in common editors.
    shortcut: "Ctrl+Alt+P",
    run: () => usePaletteStore.getState().openPalette(SEARCH_PREFIX),
  },
  {
    id: "settings.open",
    get title() {
      return t("shell:commands.settingsOpen");
    },
    get group() {
      return t("shell:commandGroups.general");
    },
    icon: Settings,
    keywords: ["preferences", "options", "configure", "tray", "autostart", "настройки", "параметры", "трей", "автозапуск"],
    shortcut: "Ctrl+,",
    run: () => useSettingsStore.getState().openSettings(),
  },
  {
    id: "onboarding.open",
    get title() {
      return t("shell:commands.onboardingOpen");
    },
    get group() {
      return t("shell:commandGroups.general");
    },
    icon: Wand2,
    keywords: ["onboarding", "welcome", "first run", "checks", "мастер", "настройка", "приветствие", "проверки"],
    run: () => useOnboardingStore.getState().openWizard(0),
  },
  {
    id: "notifications.toggleDnd",
    get title() {
      return t("shell:commands.toggleDnd");
    },
    get group() {
      return t("shell:commandGroups.general");
    },
    icon: BellOff,
    keywords: ["notifications", "quiet", "mute", "dnd", "не беспокоить", "уведомления", "тишина"],
    enabled: () => useSettingsStore.getState().settings !== null,
    run: async () => {
      const s = useSettingsStore.getState().settings;
      if (s) await useSettingsStore.getState().update({ doNotDisturb: !s.doNotDisturb });
    },
  },
  {
    id: "view.missionControl",
    get title() {
      return t("shell:commands.missionControl");
    },
    get group() {
      return t("shell:commandGroups.views");
    },
    icon: Activity,
    keywords: ["mission control", "monitoring", "overview", "dashboard", "центр управления", "мониторинг", "обзор", "дашборд"],
    // mission-control (Phase 3): full-screen Mission Control, toggled.
    shortcut: "Ctrl+Shift+D",
    run: () => useMissionControlStore.getState().toggleFullscreen(),
  },
  {
    id: "view.logs",
    get title() {
      return t("shell:commands.openLogs");
    },
    get group() {
      return t("shell:commandGroups.views");
    },
    icon: ScrollText,
    keywords: ["events", "log", "debug", "stream", "журнал", "события", "логи"],
    run: () => workspaceActions.openPanel("logs"),
  },
  {
    id: "view.toggleMaximize",
    get title() {
      return t("shell:commands.toggleMaximize");
    },
    get group() {
      return t("shell:commandGroups.views");
    },
    icon: Expand,
    keywords: ["maximize", "zoom", "focus", "panel", "restore", "развернуть", "панель", "восстановить"],
    run: () => workspaceActions.toggleMaximize(),
  },
  {
    id: "window.fullscreen",
    get title() {
      return t("shell:commands.toggleFullscreen");
    },
    get group() {
      return t("shell:commandGroups.window");
    },
    icon: Maximize,
    keywords: ["full screen", "maximize", "zen", "полный экран", "полноэкранный"],
    shortcut: "F11",
    run: () => shellControls.toggleFullscreen(),
  },
  {
    id: "ai.stopAll",
    get title() {
      return t("shell:commands.stopAll");
    },
    get group() {
      return t("shell:commandGroups.safety");
    },
    icon: OctagonX,
    keywords: ["emergency", "kill", "halt", "panic", "stop agents", "экстренная остановка", "остановить", "стоп", "агенты"],
    shortcut: "Ctrl+Shift+Backspace",
    // Opens the security confirmation dialog (stop + re-arm live there).
    run: () => useSecurityStore.getState().setConfirmStopOpen(true),
  },
  {
    id: "language.switch",
    // Bilingual on purpose: whoever cannot read the current language still finds the switch.
    get title() {
      const target = nextLanguage();
      return `${t("shell:commands.switchLanguage")} / ${t("shell:commands.switchLanguage", { lng: target })}`;
    },
    get group() {
      return t("shell:commandGroups.general");
    },
    icon: Languages,
    keywords: ["language", "locale", "english", "russian", "translate", "язык", "русский", "английский", "локализация"],
    run: async () => {
      await switchLanguage(nextLanguage());
    },
  },
];
