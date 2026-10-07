/**
 * Dialogs of the File menu: Clone repository, New file / New folder name
 * prompt, typed path (Open folder / Open file without a native picker) and
 * "file outside every project".
 */
import "./workbench.css";

import { Input } from "@orkestr/ui";
import { FilePlus, FileInput, FolderGit2, FolderOpen, FolderPlus } from "lucide-react";
import { type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from "react";

import { PathInput } from "../../components/PathInput";
import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useActiveProject } from "../../store/app";
import { ActionDialog } from "../files/Dialog";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { createEntry, openFileFromDisk, openFolderAsProject, openOutsideFile } from "./actions";
import { isAbsoluteHostPath } from "./hostPaths";
import { useWorkbenchStore } from "./store";

/** Enter in a text field submits (the dialog's buttons live outside the form). */
function submitOnEnter(e: KeyboardEvent<HTMLFormElement>, submit: () => void): void {
  if (e.key === "Enter" && e.target instanceof HTMLInputElement && !e.nativeEvent.isComposing) {
    e.preventDefault();
    submit();
  }
}

/** Client-side check of a project-relative path (the core re-validates). */
export function validateRelativeEntry(path: string): "required" | "invalid" | null {
  const p = path.trim().replace(/\\/g, "/");
  if (!p) return "required";
  if (isAbsoluteHostPath(p) || p.startsWith("/") || p.split("/").some((part) => part === "..")) return "invalid";
  return null;
}

/** Folder name git would pick for a URL (mirrors `orkestr_vcs::clone::folder_name_from_url`; display only). */
export function repoNameFromUrl(url: string): string {
  const last = url.trim().replace(/[\\/]+$/, "").split(/[\\/:]/).pop() ?? "";
  const name = last.replace(/\.git$/, "");
  return name === "." || name === ".." ? "" : name;
}

function CloneDialog() {
  const { t } = useT(["workbench", "common"]);
  const close = () => useWorkbenchStore.getState().setCloneOpen(false);
  const project = useActiveProject();
  const [url, setUrl] = useState("");
  const [parent, setParent] = useState("");
  const [folderName, setFolderName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const urlRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    urlRef.current?.focus();
  }, []);

  const derived = repoNameFromUrl(url);

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (busy) return;
    if (!url.trim()) return setError(t("workbench:clone.urlRequired"));
    if (!parent.trim()) return setError(t("workbench:clone.parentRequired"));
    if (!isAbsoluteHostPath(parent)) return setError(t("workbench:clone.parentNotAbsolute"));
    setBusy(true);
    setError(null);
    try {
      const params: { url: string; parentDir: string; folderName?: string } = { url: url.trim(), parentDir: parent.trim() };
      if (folderName.trim()) params.folderName = folderName.trim();
      const result = await getClient().call("vcs.clone", params);
      await openFolderAsProject(result.path);
      useNotificationsStore.getState().notify("Success", t("workbench:clone.done"), result.path);
      close();
    } catch (err) {
      setError(`${t("workbench:clone.failed")} ${describeError(err)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={t("workbench:clone.title")}
      subtitle={t("workbench:clone.subtitle")}
      icon={<FolderGit2 size={18} />}
      onCancel={() => !busy && close()}
      error={error}
      actions={[
        { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", disabled: busy, onSelect: close },
        { id: "clone", label: t("workbench:clone.submit"), variant: "primary", busy, onSelect: () => void submit() },
      ]}
    >
      <form className="wb-form" onSubmit={(e) => void submit(e)} onKeyDown={(e) => submitOnEnter(e, () => void submit())}>
        <Input
          ref={urlRef}
          label={t("workbench:clone.url")}
          placeholder={t("workbench:clone.urlPlaceholder")}
          value={url}
          spellCheck={false}
          disabled={busy}
          onChange={(e) => setUrl(e.target.value)}
        />
        <PathInput
          label={t("workbench:clone.parent")}
          placeholder={t("workbench:clone.parentPlaceholder")}
          hint={t("workbench:clone.parentHint")}
          dialogTitle={t("workbench:pick.cloneParent")}
          defaultPath={parent.trim() || (project?.rootPath ?? undefined)}
          value={parent}
          disabled={busy}
          onPathChange={setParent}
        />
        <Input
          label={t("workbench:clone.folderName")}
          placeholder={derived ? t("workbench:clone.folderNamePlaceholder", { name: derived }) : t("workbench:clone.folderNameEmptyPlaceholder")}
          value={folderName}
          spellCheck={false}
          disabled={busy}
          onChange={(e) => setFolderName(e.target.value)}
        />
        {busy && (
          <p className="wb-form__note" role="status">
            {t("workbench:clone.cloning")}
          </p>
        )}
      </form>
    </ActionDialog>
  );
}

function EntryDialog({ kind }: { kind: "file" | "folder" }) {
  const { t } = useT(["workbench", "common"]);
  const project = useActiveProject();
  const close = () => useWorkbenchStore.getState().setEntryPrompt(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  if (!project) return null;

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (busy) return;
    const problem = validateRelativeEntry(name);
    if (problem) return setError(t(`workbench:entry.${problem}`));
    setBusy(true);
    setError(null);
    try {
      await createEntry(project.id, kind, name);
      close();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={kind === "file" ? t("workbench:entry.fileTitle") : t("workbench:entry.folderTitle")}
      subtitle={t("workbench:entry.subtitle", { name: project.name })}
      icon={kind === "file" ? <FilePlus size={18} /> : <FolderPlus size={18} />}
      onCancel={() => !busy && close()}
      actions={[
        { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", disabled: busy, onSelect: close },
        { id: "create", label: t("workbench:entry.create"), variant: "primary", busy, onSelect: () => void submit() },
      ]}
    >
      <form className="wb-form" onSubmit={(e) => void submit(e)} onKeyDown={(e) => submitOnEnter(e, () => void submit())}>
        <Input
          ref={inputRef}
          label={kind === "file" ? t("workbench:entry.fileLabel") : t("workbench:entry.folderLabel")}
          placeholder={kind === "file" ? t("workbench:entry.filePlaceholder") : t("workbench:entry.folderPlaceholder")}
          hint={t("workbench:entry.hint")}
          value={name}
          spellCheck={false}
          disabled={busy}
          error={error ?? undefined}
          onChange={(e) => setName(e.target.value)}
        />
      </form>
    </ActionDialog>
  );
}

function PathPromptDialog({ kind }: { kind: "openFolder" | "openFile" }) {
  const { t } = useT(["workbench", "common"]);
  const close = () => useWorkbenchStore.getState().setPathPrompt(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const folder = kind === "openFolder";

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (busy) return;
    if (!isAbsoluteHostPath(path)) return setError(t("workbench:pathPrompt.notAbsolute"));
    setBusy(true);
    setError(null);
    try {
      if (folder) await openFolderAsProject(path);
      else await openFileFromDisk(path);
      close();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={folder ? t("workbench:pathPrompt.openFolderTitle") : t("workbench:pathPrompt.openFileTitle")}
      subtitle={t("workbench:pathPrompt.subtitle")}
      icon={folder ? <FolderOpen size={18} /> : <FileInput size={18} />}
      onCancel={() => !busy && close()}
      actions={[
        { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", disabled: busy, onSelect: close },
        { id: "open", label: t("workbench:pathPrompt.open"), variant: "primary", busy, onSelect: () => void submit() },
      ]}
    >
      <form className="wb-form" onSubmit={(e) => void submit(e)} onKeyDown={(e) => submitOnEnter(e, () => void submit())}>
        <PathInput
          ref={inputRef}
          kind={folder ? "folder" : "file"}
          label={folder ? t("workbench:pathPrompt.folderLabel") : t("workbench:pathPrompt.fileLabel")}
          placeholder={folder ? t("workbench:pathPrompt.folderPlaceholder") : t("workbench:pathPrompt.filePlaceholder")}
          value={path}
          disabled={busy}
          error={error ?? undefined}
          onPathChange={setPath}
        />
      </form>
    </ActionDialog>
  );
}

function OutsideFileDialog() {
  const { t } = useT(["workbench", "common"]);
  const file = useWorkbenchStore((s) => s.outsideFile);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!file) return null;
  const close = () => useWorkbenchStore.getState().setOutsideFile(null);

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await openOutsideFile(file);
      close();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={t("workbench:outside.title")}
      subtitle={<span className="wb-path">{file.path}</span>}
      icon={<FileInput size={18} />}
      tone="warning"
      error={error}
      onCancel={() => !busy && close()}
      actions={[
        { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", disabled: busy, onSelect: close },
        { id: "open", label: t("workbench:outside.openFolder"), variant: "primary", busy, autoFocus: true, onSelect: () => void confirm() },
      ]}
    >
      <p className="fx-dialog__text">{t("workbench:outside.text", { folder: file.folder })}</p>
    </ActionDialog>
  );
}

/** Mounted once in the main window. */
export function WorkbenchDialogs() {
  const cloneOpen = useWorkbenchStore((s) => s.cloneOpen);
  const entry = useWorkbenchStore((s) => s.entryPrompt);
  const pathPrompt = useWorkbenchStore((s) => s.pathPrompt);
  return (
    <>
      {cloneOpen && <CloneDialog />}
      {entry && <EntryDialog key={entry} kind={entry} />}
      {pathPrompt && <PathPromptDialog key={pathPrompt} kind={pathPrompt} />}
      <OutsideFileDialog />
    </>
  );
}
