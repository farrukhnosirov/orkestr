/**
 * What the Window Library's buttons do: open here (dock / focus), open in a
 * new window (detached, or floating outside the desktop shell), add to the
 * layout as a new split, pin to the sidebar. Every open is remembered in
 * "Recently opened" and closes the library.
 */
import { workspaceActions } from "../../components/workspaceActions";
import { t } from "../../i18n";
import { addPanelToLayout, detachPanel, openEditor as openLayoutEditor } from "../layout/controller";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { useSettingsStore } from "../settings/settingsStore";
import { useThemeStore } from "../theme/themeStore";
import type { LibraryEntry, WindowId } from "./catalog";
import { useWindowLibraryStore } from "./store";

const WINDOW_OPENERS: Record<WindowId, () => void> = {
  settings: () => useSettingsStore.getState().openSettings(),
  themeEditor: () => useThemeStore.getState().openEditor(),
  layoutEditor: () => openLayoutEditor(true),
};

function done(entry: LibraryEntry): void {
  const s = useWindowLibraryStore.getState();
  s.noteOpened(entry.id);
  s.setOpen(false);
}

/** Opens the entry in the main window (focuses it when already open). */
export function openHere(entry: LibraryEntry): void {
  done(entry);
  if (entry.kind === "window") WINDOW_OPENERS[entry.id as WindowId]?.();
  else workspaceActions.openPanel(entry.id);
}

/** Opens a panel in its own window (dialog windows open normally). */
export function openInNewWindow(entry: LibraryEntry): void {
  if (entry.kind === "window") {
    openHere(entry);
    return;
  }
  done(entry);
  void detachPanel(entry.id);
}

/** Adds a panel to the current layout as a new split. */
export function addToLayout(entry: LibraryEntry): void {
  if (entry.kind === "window") {
    openHere(entry);
    return;
  }
  if (!addPanelToLayout(entry.id)) {
    useNotificationsStore.getState().notify("Warning", t("windowLibrary:toast.layoutLocked"));
    return;
  }
  done(entry);
  useNotificationsStore.getState().notify("Success", t("windowLibrary:toast.addedToLayout", { title: entry.title }));
}

/** Pins / unpins an entry on the left sidebar. */
export function togglePin(entry: LibraryEntry): void {
  useWindowLibraryStore.getState().togglePin(entry.id);
}
