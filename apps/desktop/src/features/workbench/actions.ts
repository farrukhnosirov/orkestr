/**
 * File-menu actions (VS Code-like project/file management). Business data
 * goes through the core (`projects.*`, `files.*`, `vcs.clone`); only the
 * native pickers come from the shell (`lib/tauri`).
 */
import type { Project } from "@orkestr/protocol";

import { PROJECT_COLORS } from "../../components/CreateProjectDialog";
import { getClient, openProject } from "../../core/client";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { canPickPaths, pickFile, pickFolder, shellControls } from "../../lib/tauri";
import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import { openInEditor, saveAsEditor, saveEditor } from "../files/actions";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { hostBasename, hostDirname, projectAtFolder, projectForFile, relativeTo } from "./hostPaths";
import { type OutsideFile, useWorkbenchStore } from "./store";

/** Max project name length (core `MAX_NAME_LEN`). */
const MAX_NAME = 120;

function notifyError(titleKey: "openFolderFailed" | "openFileFailed" | "revealFailed" | "actionFailed", e: unknown): void {
  useNotificationsStore.getState().notify("Error", t(`workbench:notices.${titleKey}`), describeError(e));
}

/** The active project, if any. */
function activeProject(): Project | null {
  const s = useAppStore.getState();
  return s.projects.find((p) => p.id === s.activeProjectId) ?? null;
}

/**
 * Opens `folder` as a project: switches to the project that already uses
 * that folder, or creates one named after the folder and opens it.
 */
export async function openFolderAsProject(folder: string): Promise<{ project: Project; created: boolean }> {
  const path = folder.trim();
  const store = useAppStore.getState();
  const existing = projectAtFolder(store.projects, path);
  if (existing) {
    await openProject(existing.id);
    return { project: existing, created: false };
  }
  const name = (hostBasename(path) || path).slice(0, MAX_NAME);
  const color = PROJECT_COLORS[store.projects.length % PROJECT_COLORS.length] ?? PROJECT_COLORS[0];
  const { project } = await getClient().call("projects.create", { name, rootPath: path, color });
  useAppStore.getState().upsertProject(project);
  await openProject(project.id);
  return { project, created: true };
}

/** Opens `relPath` of `projectId` in the Code panel (binding the editor to that project first). */
function openInProject(projectId: string, relPath: string): void {
  useEditorStore.getState().setProject(projectId);
  openInEditor(relPath);
}

/**
 * Opens a file chosen on disk: inside a project → that project becomes active
 * and the file opens in the Code panel; otherwise the user is offered to open
 * its folder as a project.
 */
export async function openFileFromDisk(file: string): Promise<"opened" | "outside"> {
  const path = file.trim();
  const hit = projectForFile(useAppStore.getState().projects, path);
  if (!hit) {
    useWorkbenchStore.getState().setOutsideFile({ path, folder: hostDirname(path) });
    return "outside";
  }
  if (useAppStore.getState().activeProjectId !== hit.project.id) await openProject(hit.project.id);
  openInProject(hit.project.id, hit.relPath);
  return "opened";
}

/** "Open folder as project" for a file outside every project, then opens the file. */
export async function openOutsideFile(file: OutsideFile): Promise<void> {
  const { project } = await openFolderAsProject(file.folder);
  const rel = relativeTo(project.rootPath ?? file.folder, file.path);
  if (rel) openInProject(project.id, rel);
}

// ── Menu / command entry points ───────────────────────────────

/** File → Open folder… (native picker; a typed path without the shell). */
export async function openFolderCommand(): Promise<void> {
  if (!canPickPaths()) {
    useWorkbenchStore.getState().setPathPrompt("openFolder");
    return;
  }
  try {
    const folder = await pickFolder({ title: t("workbench:pick.openFolder"), defaultPath: activeProject()?.rootPath ?? undefined });
    if (folder) await openFolderAsProject(folder);
  } catch (e) {
    notifyError("openFolderFailed", e);
  }
}

/** File → Open file… */
export async function openFileCommand(): Promise<void> {
  if (!canPickPaths()) {
    useWorkbenchStore.getState().setPathPrompt("openFile");
    return;
  }
  try {
    const file = await pickFile({ title: t("workbench:pick.openFile"), defaultPath: activeProject()?.rootPath ?? undefined });
    if (file) await openFileFromDisk(file);
  } catch (e) {
    notifyError("openFileFailed", e);
  }
}

/** Whether the active project has a folder (New file / New folder / Reveal need one). */
export function activeHasFolder(projectId: string | null): boolean {
  if (!projectId) return false;
  return Boolean(useAppStore.getState().projects.find((p) => p.id === projectId)?.rootPath);
}

/** File → New file / New folder: asks for the name (the dialog creates it). */
export function newEntryCommand(kind: "file" | "folder"): void {
  if (!activeHasFolder(useAppStore.getState().activeProjectId)) {
    useNotificationsStore.getState().notify("Info", t("workbench:notices.needFolderTitle"), t("workbench:notices.needFolderBody"));
    return;
  }
  useWorkbenchStore.getState().setEntryPrompt(kind);
}

/** Creates a file/folder in the active project; a new file opens in the Code panel. */
export async function createEntry(projectId: string, kind: "file" | "folder", relPath: string): Promise<void> {
  const path = relPath.trim().replace(/\\/g, "/").replace(/^\/+/, "");
  if (kind === "file") {
    const { entry } = await getClient().call("files.create", { projectId, path });
    openInProject(projectId, entry.path);
  } else {
    await getClient().call("files.mkdir", { projectId, path });
    useNotificationsStore.getState().notify("Success", t("workbench:notices.folderCreated"), path);
  }
}

/** File → Save / Save all (Code panel). */
export function saveCommand(all: boolean): void {
  saveEditor(all);
}

/** File → Save As… (Code panel: native save dialog, the copy opens as the active tab). */
export function saveAsCommand(): void {
  saveAsEditor();
}

/** File → Show in file manager: opens the project folder in the OS file manager. */
export async function revealProjectCommand(projectId: string | null): Promise<void> {
  if (!projectId || !activeHasFolder(projectId)) return;
  try {
    await getClient().call("files.openExternal", { projectId, target: "default" });
  } catch (e) {
    notifyError("revealFailed", e);
  }
}

/** File → Close project (the Welcome page takes over the main area). */
export function closeProjectCommand(): void {
  useAppStore.getState().closeProject();
}

/** File → Exit. */
export function exitCommand(): void {
  void shellControls.quit();
}

/** Recent project → open it. */
export function openRecent(projectId: string): void {
  void openProject(projectId).catch((e: unknown) => notifyError("actionFailed", e));
}
