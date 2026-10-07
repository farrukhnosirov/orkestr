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
  FolderX,
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
import { shellControls, type TrayStopAllOutcome } from "../../lib/tauri";
import { useAppStore } from "../../store/app";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { useOnboardingStore } from "../onboarding/onboardingStore";
import { SEARCH_PREFIX, usePaletteStore } from "../search/paletteStore";
import { refreshAfterStop, stopAllAi as stopAllAiNow, useSecurityStore } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import type { AppCommand } from "./registry";

function notifyStopped(r: { agentsStopped: number; approvalsCancelled: number }): void {
  useNotificationsStore.getState().notify(
    "Critical",
    t("shell:stopAll.doneTitle"),
    t("shell:stopAll.doneBody", {
      agents: t("shell:stopAll.agents", { count: r.agentsStopped }),
      approvals: t("shell:stopAll.approvals", { count: r.approvalsCancelled }),
    }),
  );
}

/**
 * Emergency stop of all AI activity (§52): `emergency.stopAll`, handled
 * locally in the core, without a confirmation dialog (the user explicitly
 * picked "Stop All AI"). Returns whether the stop was executed.
 */
export async function stopAllAi(): Promise<boolean> {
  try {
    notifyStopped(await stopAllAiNow());
    return true;
  } catch (e) {
    useNotificationsStore.getState().notify("Error", t("shell:stopAll.failedTitle"), describeError(e));
    return false;
  }
}

/**
 * Feedback for the tray's "Stop All AI". The shell already executed the stop
 * natively (it must work without this page); here we only show the outcome
 * and refresh the emergency state.
 */
export function reportTrayStopAll(outcome: TrayStopAllOutcome): void {
  if (outcome.ok) {
    notifyStopped(outcome);
    void refreshAfterStop();
  } else {
    useNotificationsStore.getState().notify("Error", t("shell:stopAll.failedTitle"), outcome.error ?? "");
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
    id: "project.delete",
    get title() {
      return t("shell:commands.projectDelete");
    },
    get group() {
      return t("shell:commandGroups.projects");
    },
    icon: FolderX,
    keywords: ["delete", "remove", "forget", "удалить", "убрать", "проект"],
    enabled: (ctx) => ctx.projectId !== null,
    run: (ctx) => {
      if (ctx.projectId) useAppStore.getState().setDeleteProjectId(ctx.projectId);
    },
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
    inTerminal: true,
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
    inTerminal: true,
    // Opens the security confirmation dialog (stop + re-arm live there).
    run: () => useSecurityStore.getState().setConfirmStopOpen(true),
  },
  {
    id: "ai.stopProject",
    get title() {
      return t("shell:commands.stopProject");
    },
    get group() {
      return t("shell:commandGroups.safety");
    },
    icon: FolderX,
    keywords: ["emergency", "stop project", "halt project", "экстренная остановка", "остановить проект", "стоп проект"],
    enabled: (ctx) => ctx.projectId !== null,
    // Confirms in the security dialog, scoped to the active project (§52 Stop Project).
    run: (ctx) => {
      if (ctx.projectId) useSecurityStore.getState().openStopProject(ctx.projectId);
    },
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
