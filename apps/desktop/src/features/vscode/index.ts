/**
 * Embedded VS Code (VSCodium REH web via the core's `vscode.*` area):
 * settings section and palette commands. The panel lives in `panels/VsCode`.
 */
import { Code2, Square } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { t } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import type { SettingsSection } from "../settings/sections";
import { isServerActive } from "./logic";
import { useVsCodeStore } from "./store";
import { VsCodeSettingsSection } from "./VsCodeSettingsSection";

export const vscodeSettingsSection: SettingsSection = {
  id: "vscode",
  get title() {
    return t("vscode:settings.title");
  },
  icon: Code2,
  order: 400,
  component: VsCodeSettingsSection,
};

export const vscodeCommands: AppCommand[] = [
  {
    id: "vscode.open",
    get title() {
      return t("vscode:commands.open");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: Code2,
    keywords: ["vscode", "vs code", "vscodium", "extensions", "editor", "ide", "расширения", "редактор"],
    run: () => workspaceActions.openPanel("vscode"),
  },
  {
    id: "vscode.stop",
    get title() {
      return t("vscode:commands.stop");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: Square,
    keywords: ["vscode", "vs code", "server", "stop", "сервер", "остановить"],
    enabled: () => isServerActive(useVsCodeStore.getState().status?.state ?? ""),
    run: () => useVsCodeStore.getState().stop(),
  },
];
