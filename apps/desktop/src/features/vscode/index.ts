/**
 * Embedded VS Code (VSCodium REH web via the core's `vscode.*` area):
 * settings section and palette commands. The panel lives in `panels/VsCode`.
 */
import { Blocks, Code2, PackagePlus, SquareArrowOutUpRight, Square, TerminalSquare } from "lucide-react";

import { t } from "../../i18n";
import { usePaletteStore } from "../search/paletteStore";
import type { AppCommand } from "../commands/registry";
import type { SettingsSection } from "../settings/sections";
import { isServerActive } from "./logic";
import { EXTENSIONS_SHORTCUT, openExtensions, openProjectInVsCode, openVsCodeExtensions, openVsCodePanel } from "./quickAccess";
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
    run: () => openVsCodePanel(),
  },
  {
    id: "vscode.openProject",
    get title() {
      return t("vscode:commands.openProject");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: SquareArrowOutUpRight,
    keywords: ["vscode", "vs code", "open in vs code", "window", "folder", "открыть в vs code", "окно", "папка"],
    enabled: (ctx) => ctx.projectId !== null,
    run: (ctx) => openProjectInVsCode(ctx.projectId),
  },
  {
    id: "vscode.extensionCommands",
    get title() {
      return t("vscode:commands.extensionCommands");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: TerminalSquare,
    keywords: ["vscode", "vs code", "extension", "command", "run", "launcher", "расширение", "команда", "выполнить", "запустить"],
    run: () => usePaletteStore.getState().openPalette("VS Code: "),
  },
  {
    id: "vscode.installExtension",
    get title() {
      return t("vscode:commands.installExtension");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: PackagePlus,
    keywords: ["vscode", "extension", "install", "open vsx", "marketplace", "claude", "расширение", "установить"],
    run: () => openVsCodeExtensions(),
  },
  {
    id: "extensions.open",
    get title() {
      return t("vscode:commands.extensions");
    },
    get group() {
      return t("vscode:commands.group");
    },
    icon: Blocks,
    keywords: ["extensions", "plugins", "marketplace", "open vsx", "расширения", "плагины", "каталог"],
    shortcut: EXTENSIONS_SHORTCUT,
    run: () => openExtensions(),
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
