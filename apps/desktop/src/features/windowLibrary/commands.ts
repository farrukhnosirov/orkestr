/**
 * Window Library command (§54 registry): palette entry + Ctrl+Shift+L.
 * Ctrl+Shift+L is free in the shell; inside Monaco it stays "select all
 * occurrences" (`inEditor: false`), in a terminal it reaches the shell.
 */
import { LibraryBig } from "lucide-react";

import { t } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import { useWindowLibraryStore } from "./store";

export const WINDOW_LIBRARY_SHORTCUT = "Ctrl+Shift+L";

export const windowLibraryCommands: AppCommand[] = [
  {
    id: "windowLibrary.open",
    get title() {
      return t("windowLibrary:command.open");
    },
    get group() {
      return t("windowLibrary:command.group");
    },
    icon: LibraryBig,
    keywords: ["window", "panel", "library", "gallery", "catalog", "open", "окно", "окна", "панель", "панели", "библиотека", "галерея", "каталог", "открыть"],
    shortcut: WINDOW_LIBRARY_SHORTCUT,
    inEditor: false,
    run: () => useWindowLibraryStore.getState().toggle(),
  },
];
