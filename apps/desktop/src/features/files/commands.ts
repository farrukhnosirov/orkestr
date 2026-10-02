/** Files/Code commands contributed to the command registry (§54). */
import { FileSearch, FolderTree, Search, SquareCode } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { t } from "../../i18n";
import { useEditorStore } from "../../store/editor";
import type { AppCommand } from "../commands/registry";

const hasProject = (ctx: { projectId: string | null }) => ctx.projectId !== null;

export const FILE_COMMANDS: AppCommand[] = [
  {
    id: "files.quickOpen",
    get title() {
      return t("commands:files.quickOpen");
    },
    get group() {
      return t("commands:groups.files");
    },
    icon: FileSearch,
    keywords: ["open", "quick open", "find file", "файл", "открыть", "перейти"],
    shortcut: "Ctrl+P",
    enabled: hasProject,
    run: () => {
      workspaceActions.openPanel("code");
      useEditorStore.getState().setQuickOpen(true);
    },
  },
  {
    id: "files.searchProject",
    get title() {
      return t("commands:files.searchProject");
    },
    get group() {
      return t("commands:groups.files");
    },
    icon: Search,
    keywords: ["find", "grep", "search in files", "найти", "поиск", "в файлах"],
    shortcut: "Ctrl+Shift+F",
    enabled: hasProject,
    run: () => {
      workspaceActions.openPanel("code");
      useEditorStore.getState().setSearchOpen(true);
    },
  },
  {
    id: "files.showExplorer",
    get title() {
      return t("commands:files.showExplorer");
    },
    get group() {
      return t("commands:groups.files");
    },
    icon: FolderTree,
    keywords: ["tree", "explorer", "дерево", "проводник", "файлы"],
    run: () => workspaceActions.openPanel("files"),
  },
  {
    id: "editor.clearAgentMarkers",
    get title() {
      return t("commands:files.clearAgentMarkers");
    },
    get group() {
      return t("commands:groups.editor");
    },
    icon: SquareCode,
    keywords: ["agent", "markers", "gutter", "агент", "отметки", "изменения"],
    run: () => useEditorStore.getState().clearAgentMarks(),
  },
];
