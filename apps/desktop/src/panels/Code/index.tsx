import "./code.css";

import { EmptyState, Spinner } from "@orkestr/ui";
import { FileCode } from "lucide-react";
import { lazy, Suspense, useEffect } from "react";

import { keyMatches } from "../../features/commands/keys";
import { RootFolderForm } from "../../features/files/RootFolderForm";
import { useT } from "../../i18n";
import { useActiveProject } from "../../store/app";
import { type RevealTarget, useEditorStore } from "../../store/editor";
import type { PanelProps } from "../types";
import { QuickOpen } from "./QuickOpen";
import { SearchView } from "./SearchView";

// Monaco is large: load it only when the Code panel is shown.
const Workbench = lazy(() => import("./Workbench"));

/** Code panel (§15): Monaco editor, tabs, quick open, project search. */
export default function CodePanel({ projectId }: PanelProps) {
  const { t } = useT("code");
  const project = useActiveProject();
  const searchOpen = useEditorStore((s) => s.searchOpen);
  const hasRoot = Boolean(project?.rootPath);

  useEffect(() => {
    useEditorStore.getState().setProject(projectId);
  }, [projectId]);

  // Ctrl+P quick open, Ctrl+Shift+F project search (while the panel is mounted).
  useEffect(() => {
    if (!projectId || !hasRoot) return;
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      if (keyMatches(e, "p") && !e.shiftKey) {
        e.preventDefault();
        useEditorStore.getState().setQuickOpen(true);
      } else if (keyMatches(e, "f") && e.shiftKey) {
        e.preventDefault();
        useEditorStore.getState().setSearchOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [projectId, hasRoot]);

  if (!projectId) {
    return (
      <EmptyState
        icon={<FileCode size={26} strokeWidth={1.6} />}
        title={t("noProject.title")}
        description={t("noProject.description")}
      />
    );
  }
  if (!hasRoot) return <RootFolderForm projectId={projectId} title={t("noFolder")} />;

  const open = (path: string, at?: RevealTarget) =>
    useEditorStore.getState().openFile(path, at);

  return (
    <div className="code-panel">
      {searchOpen && <SearchView projectId={projectId} onOpen={open} />}
      <div className="code-panel__main">
        <Suspense
          fallback={
            <div className="code-loading">
              <Spinner size={18} /> {t("loadingEditor")}
            </div>
          }
        >
          <Workbench projectId={projectId} />
        </Suspense>
      </div>
      <QuickOpen projectId={projectId} onOpen={(p) => open(p)} />
    </div>
  );
}
