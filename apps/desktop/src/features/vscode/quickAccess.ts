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
import { t } from "../../i18n";
import { getPanel } from "../../panels/registry";
import { useAppStore } from "../../store/app";
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

/**
 * "Open in VS Code": opens the active project's folder in the VS Code
 * window when VS Code is installed; otherwise shows the panel (install flow).
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
  const title = t("vscode:run.windowTitle", { name: project?.name ?? t("vscode:panel.label") });
  await useVsCodeStore.getState().open(project?.rootPath ? project.id : null, title);
  if (useVsCodeStore.getState().error) openVsCodePanel();
}
