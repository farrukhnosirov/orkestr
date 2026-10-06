/**
 * Confirmation for `projects.delete` (rail context menu, `project.delete`
 * command). The core removes the project's metadata only; files on disk are
 * never touched. The `ProjectDeleted` event updates the stores.
 */
import { FolderX, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import { getClient } from "../core/client";
import { ActionDialog } from "../features/files/Dialog";
import { useT } from "../i18n";
import { describeError } from "../i18n/errors";
import { useAppStore } from "../store/app";
import { unsavedTabCount, useEditorStore } from "../store/editor";

export function DeleteProjectDialog() {
  const { t } = useT("shell");
  const projectId = useAppStore((s) => s.deleteProjectId);
  const project = useAppStore((s) => s.projects.find((p) => p.id === s.deleteProjectId) ?? null);
  const close = useAppStore((s) => s.setDeleteProjectId);
  const unsaved = useEditorStore((s) => (projectId ? unsavedTabCount(s, projectId) : 0));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setBusy(false);
    setError(null);
  }, [projectId]);

  if (!projectId || !project) return null;

  const cancel = () => {
    if (!busy) close(null);
  };

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await getClient().call("projects.delete", { projectId });
      // The ProjectDeleted event removes it from the stores; drop it locally too
      // in case the event arrives late.
      useAppStore.getState().removeProject(projectId);
      useEditorStore.getState().forgetProject(projectId);
      close(null);
    } catch (e) {
      setError(`${t("deleteProject.failed")} ${describeError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={t("deleteProject.title", { name: project.name })}
      subtitle={t("deleteProject.subtitle")}
      icon={<FolderX size={18} />}
      tone="danger"
      error={error}
      onCancel={cancel}
      actions={[
        { id: "cancel", label: t("deleteProject.cancel"), variant: "ghost", autoFocus: true, disabled: busy, onSelect: cancel },
        {
          id: "delete",
          label: t("deleteProject.confirm"),
          variant: "danger",
          icon: <Trash2 size={14} />,
          busy,
          onSelect: () => void confirm(),
        },
      ]}
    >
      <p>{project.rootPath ? t("deleteProject.filesKept", { path: project.rootPath }) : t("deleteProject.noFolder")}</p>
      {unsaved > 0 && <p role="alert">{t("deleteProject.unsaved", { count: unsaved })}</p>}
    </ActionDialog>
  );
}
