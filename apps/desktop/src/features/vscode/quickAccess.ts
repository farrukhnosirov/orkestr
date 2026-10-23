/**
 * Quick access to VS Code and extensions (project rail, File menu, palette,
 * Ctrl+Shift+X).
 *
 * Extensions entry point (Ctrl+Shift+X, project rail): opens the native
 * Extensions panel (`panels/Extensions`, ADR-0025: Open VSX extensions in
 * Orkestr's own editor). Should that panel ever be absent from the registry,
 * it falls back to the VS Code panel's "Extensions" section (extensions of
 * the embedded VSCodium). The "VS Code" button/commands stay separate.
 */
import { workspaceActions } from "../../components/workspaceActions";
import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { getPanel } from "../../panels/registry";
import { useAppStore } from "../../store/app";
import { embedHostFor } from "./embed";
import { useVsCodeStore } from "./store";

/** Panel id the native Extensions panel registers under. */
export const NATIVE_EXTENSIONS_PANEL_ID = "extensions";

/** Shortcut of the extensions entry point (VS Code's own). */
export const EXTENSIONS_SHORTCUT = "Ctrl+Shift+X";

/** Whether the native Extensions panel is available. */
export function hasNativeExtensionsPanel(): boolean {
  return getPanel(NATIVE_EXTENSIONS_PANEL_ID) !== undefined;
}

/** Opens the VS Code panel scrolled to its Extensions section. */
export function openVsCodeExtensions(): void {
  useVsCodeStore.getState().showView("extensions");
  workspaceActions.openPanel("vscode");
}

/** Ctrl+Shift+X: the native Extensions panel when present, else the VS Code fallback. */
export function openExtensions(): void {
  if (hasNativeExtensionsPanel()) {
    workspaceActions.openPanel(NATIVE_EXTENSIONS_PANEL_ID);
    return;
  }
  openVsCodeExtensions();
}

/** Opens the VS Code panel (main view). */
export function openVsCodePanel(): void {
  useVsCodeStore.getState().showView("main");
  workspaceActions.openPanel("vscode");
}

/** Whether VS Code opens inside the Orkestr window (setting + a page that can embed it). */
export function embedsVsCode(): boolean {
  const status = useVsCodeStore.getState().status;
  const host = typeof window === "undefined" ? "" : window.location.hostname;
  return (status?.config.embed ?? true) && embedHostFor(host) !== null;
}

/**
 * "Open in VS Code": opens the active project's folder in VS Code — inside
 * the VS Code panel (ADR-0043, default) or in the separate VS Code window —
 * when VS Code is installed; otherwise shows the panel (install flow).
 */
export async function openProjectInVsCode(projectId: string | null): Promise<void> {
  const store = useVsCodeStore.getState();
  if (!store.status) await store.load();
  const status = useVsCodeStore.getState().status;
  if (!status?.installed || status.state === "installing" || status.state === "notInstalled") {
    openVsCodePanel();
    return;
  }
  const project = useAppStore.getState().projects.find((p) => p.id === projectId) ?? null;
  const folderProject = project?.rootPath ? project.id : null;
  if (embedsVsCode()) {
    openVsCodePanel();
    await useVsCodeStore.getState().startEmbedded(folderProject);
    return;
  }
  const title = t("vscode:run.windowTitle", { name: project?.name ?? t("vscode:panel.label") });
  await useVsCodeStore.getState().open(folderProject, title);
  if (useVsCodeStore.getState().error) openVsCodePanel();
}

/** Absolute path of `path` (absolute already, or relative to the project root). */
export function absolutePath(path: string, root: string | null): string | null {
  if (/^([a-zA-Z]:[\\/]|\\\\|\/)/.test(path)) return path;
  if (!root) return null;
  const sep = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${path.replace(/^[\\/]+/, "").split(/[\\/]/).join(sep)}`;
}

/** Waits this long for VS Code to start and its bridge to connect. */
export const OPEN_FILE_WAIT_MS = 90_000;

/**
 * Opens a file at a line in Orkestr's VS Code (Code panel, search results,
 * Bug Detective, agents): shows the VS Code panel (or window), starts the
 * server on the project when needed and asks the bridge to reveal the line.
 * Resolves to an error message, or `null` on success.
 */
export async function openFileInVsCode(target: {
  projectId: string | null;
  path: string;
  line?: number;
  column?: number;
}): Promise<string | null> {
  const project = useAppStore.getState().projects.find((p) => p.id === target.projectId) ?? null;
  const abs = absolutePath(target.path, project?.rootPath ?? null);
  if (!abs) return t("vscode:openFile.noFolder");
  const vs = useVsCodeStore.getState();
  const connected = vs.status?.bridgeConnected === true && vs.status.state === "running";
  if (!connected || (embedsVsCode() && vs.embedProjectId !== (project?.rootPath ? project.id : null))) {
    await openProjectInVsCode(project?.id ?? null);
  } else if (embedsVsCode()) {
    openVsCodePanel();
  }
  const failed = useVsCodeStore.getState().error;
  if (failed) return failed;
  const client = maybeClient();
  if (!client) return t("common:errors.notInitialised");
  try {
    await client.call(
      "vscode.openFile",
      {
        path: abs,
        waitMs: OPEN_FILE_WAIT_MS,
        ...(target.line ? { line: target.line } : {}),
        ...(target.column ? { column: target.column } : {}),
      },
      { timeoutMs: OPEN_FILE_WAIT_MS + 30_000 },
    );
    return null;
  } catch (e) {
    return describeError(e);
  }
}
