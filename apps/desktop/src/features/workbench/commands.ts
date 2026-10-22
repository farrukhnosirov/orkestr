/**
 * File-menu commands (§54): the same actions as the title-bar File menu and
 * the Welcome page, reachable from the palette and by shortcut. Shortcuts are
 * matched on the physical key (`keys.ts`), so they work on the Russian layout.
 *
 * Shortcut choices (checked against the registry): Ctrl+O, Ctrl+N, Ctrl+S
 * are free; "Open folder" is the VS Code chord Ctrl+K Ctrl+O (Ctrl+K opens
 * the palette and arms the chord). "Save all" is Ctrl+Shift+S — VS Code's
 * Ctrl+K S would swallow an "s" typed into the palette Ctrl+K just opened.
 */
import {
  DoorOpen,
  FilePlus,
  FolderGit2,
  FolderInput,
  FolderOpen,
  FolderPlus,
  FolderX,
  Menu,
  Save,
  SaveAll,
  FileInput,
  FileOutput,
} from "lucide-react";

import { t } from "../../i18n";
import { isTauri } from "../../lib/tauri";
import type { AppCommand, CommandContext } from "../commands/registry";
import {
  activeHasFolder,
  closeProjectCommand,
  exitCommand,
  newEntryCommand,
  openFileCommand,
  openFolderCommand,
  revealProjectCommand,
  saveAsCommand,
  saveCommand,
} from "./actions";
import { useWorkbenchStore } from "./store";

export const SHORTCUTS = {
  openFolder: "Ctrl+K Ctrl+O",
  openFile: "Ctrl+O",
  newFile: "Ctrl+N",
  save: "Ctrl+S",
  saveAll: "Ctrl+Shift+S",
  fileMenu: "Alt+F",
} as const;

const group = () => t("workbench:commands.group");
const hasProject = (ctx: CommandContext) => ctx.projectId !== null;
const hasFolder = (ctx: CommandContext) => activeHasFolder(ctx.projectId);

export const WORKBENCH_COMMANDS: AppCommand[] = [
  {
    id: "file.openFolder",
    get title() {
      return t("workbench:menu.openFolder");
    },
    get group() {
      return group();
    },
    icon: FolderOpen,
    keywords: ["open folder", "open project", "workspace", "открыть папку", "открыть проект", "папка"],
    shortcut: SHORTCUTS.openFolder,
    run: () => openFolderCommand(),
  },
  {
    id: "file.openFile",
    get title() {
      return t("workbench:menu.openFile");
    },
    get group() {
      return group();
    },
    icon: FileInput,
    keywords: ["open file", "открыть файл", "файл"],
    shortcut: SHORTCUTS.openFile,
    run: () => openFileCommand(),
  },
  {
    id: "file.newFile",
    get title() {
      return t("workbench:menu.newFile");
    },
    get group() {
      return group();
    },
    icon: FilePlus,
    keywords: ["new file", "create file", "новый файл", "создать файл"],
    shortcut: SHORTCUTS.newFile,
    enabled: hasFolder,
    run: () => newEntryCommand("file"),
  },
  {
    id: "file.newFolder",
    get title() {
      return t("workbench:menu.newFolder");
    },
    get group() {
      return group();
    },
    icon: FolderPlus,
    keywords: ["new folder", "create folder", "mkdir", "новая папка", "создать папку"],
    enabled: hasFolder,
    run: () => newEntryCommand("folder"),
  },
  {
    id: "file.save",
    get title() {
      return t("workbench:menu.save");
    },
    get group() {
      return group();
    },
    icon: Save,
    keywords: ["save", "write", "сохранить"],
    shortcut: SHORTCUTS.save,
    enabled: hasProject,
    run: () => saveCommand(false),
  },
  {
    id: "file.saveAs",
    get title() {
      return t("workbench:menu.saveAs");
    },
    get group() {
      return group();
    },
    icon: FileOutput,
    keywords: ["save as", "save copy", "сохранить как", "сохранить копию"],
    enabled: hasFolder,
    run: () => saveAsCommand(),
  },
  {
    id: "file.saveAll",
    get title() {
      return t("workbench:menu.saveAll");
    },
    get group() {
      return group();
    },
    icon: SaveAll,
    keywords: ["save all", "сохранить всё", "сохранить все"],
    shortcut: SHORTCUTS.saveAll,
    enabled: hasProject,
    run: () => saveCommand(true),
  },
  {
    id: "file.clone",
    get title() {
      return t("workbench:menu.clone");
    },
    get group() {
      return group();
    },
    icon: FolderGit2,
    keywords: ["clone", "git clone", "repository", "клонировать", "репозиторий", "git"],
    run: () => useWorkbenchStore.getState().setCloneOpen(true),
  },
  {
    id: "file.reveal",
    get title() {
      return t("workbench:menu.reveal");
    },
    get group() {
      return group();
    },
    icon: FolderInput,
    keywords: ["reveal", "explorer", "finder", "file manager", "проводник", "показать папку"],
    enabled: hasFolder,
    run: (ctx) => revealProjectCommand(ctx.projectId),
  },
  {
    id: "file.closeProject",
    get title() {
      return t("workbench:menu.closeProject");
    },
    get group() {
      return group();
    },
    icon: FolderX,
    keywords: ["close project", "close folder", "закрыть проект", "закрыть папку"],
    enabled: hasProject,
    run: () => closeProjectCommand(),
  },
  {
    id: "file.menu",
    get title() {
      return t("workbench:commands.openMenu");
    },
    get group() {
      return group();
    },
    icon: Menu,
    keywords: ["file menu", "меню файл", "меню"],
    shortcut: SHORTCUTS.fileMenu,
    run: () => useWorkbenchStore.getState().setMenuOpen(true),
  },
  {
    id: "app.exit",
    get title() {
      return t("workbench:menu.exit");
    },
    get group() {
      return group();
    },
    icon: DoorOpen,
    keywords: ["exit", "quit", "close app", "выход", "выйти", "закрыть приложение"],
    enabled: () => isTauri(),
    run: () => exitCommand(),
  },
];
