/** Files/Code commands contributed to the command registry (§54). */
import { FileSearch, FolderTree, Search, SquareCode } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { useEditorStore } from "../../store/editor";
import type { AppCommand } from "../commands/registry";

const hasProject = (ctx: { projectId: string | null }) => ctx.projectId !== null;

export const FILE_COMMANDS: AppCommand[] = [
  {
    id: "files.quickOpen",
    title: "Go to File…",
    group: "Files",
    icon: FileSearch,
    keywords: ["open", "quick open", "find file"],
    shortcut: "Ctrl+P",
    enabled: hasProject,
    run: () => {
      workspaceActions.openPanel("code");
      useEditorStore.getState().setQuickOpen(true);
    },
  },
  {
    id: "files.searchProject",
    title: "Search in Project",
    group: "Files",
    icon: Search,
    keywords: ["find", "grep", "search in files"],
    shortcut: "Ctrl+Shift+F",
    enabled: hasProject,
    run: () => {
      workspaceActions.openPanel("code");
      useEditorStore.getState().setSearchOpen(true);
    },
  },
  {
    id: "files.showExplorer",
    title: "Show File Explorer",
    group: "Files",
    icon: FolderTree,
    keywords: ["tree", "explorer"],
    run: () => workspaceActions.openPanel("files"),
  },
  {
    id: "editor.clearAgentMarkers",
    title: "Clear Agent Change Markers",
    group: "Editor",
    icon: SquareCode,
    keywords: ["agent", "markers", "gutter"],
    run: () => useEditorStore.getState().clearAgentMarks(),
  },
];
