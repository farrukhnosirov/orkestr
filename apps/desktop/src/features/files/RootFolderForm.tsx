import "./shared.css";

/**
 * Shown when the active project has no root folder yet: lets the user set
 * one (`projects.update { rootPath }`). The core validates the path.
 */
import { Button, EmptyState } from "@orkestr/ui";
import { FolderOpen } from "lucide-react";
import { type FormEvent, useState } from "react";

import { PathInput } from "../../components/PathInput";
import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { errorMessage } from "./rpc";

export function RootFolderForm({ projectId, title }: { projectId: string; title: string }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT("fileOps");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const rootPath = path.trim();
    if (!rootPath) {
      setError(t("rootFolder.pathRequired"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const { project } = await getClient().call("projects.update", { projectId, patch: { rootPath } });
      useAppStore.getState().upsertProject(project);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <EmptyState
      icon={<FolderOpen size={26} strokeWidth={1.6} />}
      title={title}
      description={t("rootFolder.description")}
      action={
        <form className="fx-root-form" onSubmit={(e) => void submit(e)}>
          <PathInput
            aria-label={t("rootFolder.label")}
            placeholder={t("rootFolder.placeholder")}
            value={path}
            onPathChange={setPath}
            error={error ?? undefined}
          />
          <Button type="submit" variant="primary" size="sm" loading={busy}>
            {t("rootFolder.use")}
          </Button>
        </form>
      }
    />
  );
}
