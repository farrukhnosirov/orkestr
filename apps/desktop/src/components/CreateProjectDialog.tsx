import { OrkestrError } from "@orkestr/client";
import { Button, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { FolderPlus, X } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { createProject } from "../core/client";
import { useT } from "../i18n";
import { describeError } from "../i18n/errors";
import { hostBasename, isAbsoluteHostPath } from "../features/workbench/hostPaths";
import { useAppStore } from "../store/app";
import { Overlay } from "./Overlay";
import { PathInput } from "./PathInput";

/** Accent swatches offered for new projects (stored as `#RRGGBB` on the project). */
export const PROJECT_COLORS = ["#22d3ee", "#a78bfa", "#f472b6", "#34d399", "#fbbf24", "#60a5fa", "#fb7185"] as const;

/** Modal to create a project via `projects.create`. */
export function CreateProjectDialog() {
  const open = useAppStore((s) => s.createProjectOpen);
  const setOpen = useAppStore((s) => s.setCreateProjectOpen);
  const connected = useAppStore((s) => s.connection.state === "open");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [folder, setFolder] = useState("");
  const [folderError, setFolderError] = useState<string | null>(null);
  const [color, setColor] = useState<string>(PROJECT_COLORS[0]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const { t } = useT(["shell", "common"]);

  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      setFolder("");
      setFolderError(null);
      setError(null);
      setBusy(false);
      requestAnimationFrame(() => nameRef.current?.focus());
    }
  }, [open]);

  if (!open) return null;

  const close = () => {
    if (!busy) setOpen(false);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError(t("shell:createProject.nameRequired"));
      return;
    }
    const rootPath = folder.trim();
    if (rootPath && !isAbsoluteHostPath(rootPath)) {
      setFolderError(t("shell:createProject.folderNotAbsolute"));
      return;
    }
    setBusy(true);
    setError(null);
    setFolderError(null);
    try {
      await createProject({ name: trimmed, description: description.trim(), color, rootPath });
      setOpen(false);
    } catch (err) {
      setError(err instanceof OrkestrError ? describeError(err) : t("shell:createProject.failed"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Overlay onDismiss={close}>
      <form
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-project-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <FolderPlus size={18} />
          </div>
          <div>
            <h2 id="create-project-title" className="dialog__title">
              {t("shell:createProject.title")}
            </h2>
            <p className="dialog__subtitle">{t("shell:createProject.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <Input
            ref={nameRef}
            label={t("shell:createProject.name")}
            placeholder={t("shell:createProject.namePlaceholder")}
            value={name}
            maxLength={120}
            onChange={(e) => setName(e.target.value)}
            error={error ?? undefined}
          />
          <PathInput
            label={t("shell:createProject.folder")}
            placeholder={t("shell:createProject.folderPlaceholder")}
            hint={t("shell:createProject.folderHint")}
            value={folder}
            error={folderError ?? undefined}
            onPathChange={setFolder}
            mapPicked={(picked) => {
              // A picked folder names a still unnamed project.
              if (!name.trim()) setName(hostBasename(picked));
              return picked;
            }}
          />
          <Input
            label={t("shell:createProject.description")}
            placeholder={t("shell:createProject.descriptionPlaceholder")}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
          <div className="ok-field">
            <span className="ok-field__label">{t("shell:createProject.accent")}</span>
            <div className="swatches" role="radiogroup" aria-label={t("shell:createProject.accentGroup")}>
              {PROJECT_COLORS.map((c) => (
                <button
                  key={c}
                  type="button"
                  role="radio"
                  aria-checked={c === color}
                  aria-label={c}
                  className={clsx("swatch", c === color && "swatch--active")}
                  style={{ ["--swatch" as string]: c }}
                  onClick={() => setColor(c)}
                />
              ))}
            </div>
          </div>
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy} disabled={!connected}>
            {t("shell:createProject.submit")}
          </Button>
        </footer>
      </form>
    </Overlay>
  );
}
