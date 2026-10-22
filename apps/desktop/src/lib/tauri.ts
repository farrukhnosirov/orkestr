/**
 * Thin wrapper around the Tauri APIs the shell needs: window controls, the
 * `core_connection` command and the native open dialogs. Business data never
 * goes through Tauri (§67) — everything else uses @orkestr/client.
 */
import { invoke, isTauri as tauriDetected } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { t } from "../i18n";

export interface CoreConnection {
  url: string;
  token: string;
  mode: "embedded" | "external" | "browser";
  startupMs: number;
}

/** True when running inside the Tauri webview (false in a plain browser / tests). */
export function isTauri(): boolean {
  try {
    return tauriDetected();
  } catch {
    return false;
  }
}

/** Label of the main window (src-tauri `MAIN_WINDOW`). */
export const MAIN_WINDOW_LABEL = "main";

/**
 * True in the main shell window. Shell-wide commands (prefs, tray language,
 * quit…) are granted to the main window only (src-tauri capabilities); detached
 * panels and the HUD leave them to it — it gets every change as an event.
 */
export function isMainWindow(): boolean {
  if (!isTauri()) return false;
  try {
    return getCurrentWindow().label === MAIN_WINDOW_LABEL;
  } catch {
    return false;
  }
}

/** Asks the shell for the core's `{url, token}` (waits until the core is ready). */
export async function getCoreConnection(): Promise<CoreConnection> {
  if (isTauri()) {
    return invoke<CoreConnection>("core_connection");
  }
  // Browser dev mode: `?core=ws://127.0.0.1:PORT/rpc&token=…` or VITE_ORKESTR_URL / VITE_ORKESTR_TOKEN.
  const params = new URLSearchParams(window.location.search);
  const url = params.get("core") ?? (import.meta.env["VITE_ORKESTR_URL"] as string | undefined);
  const token = params.get("token") ?? (import.meta.env["VITE_ORKESTR_TOKEN"] as string | undefined);
  if (!url || !token) {
    throw new Error(t("shell:connection.notInShell"));
  }
  return { url, token, mode: "browser", startupMs: 0 };
}

/** Event with the outcome of the tray's "Stop All AI" (see src-tauri `TRAY_STOP_ALL_EVENT`). */
export const TRAY_STOP_ALL_EVENT = "orkestr://tray/stop-all";

/**
 * Outcome of the tray's "Stop All AI" (src-tauri `stop_all::StopAllOutcome`).
 * The shell executes the stop itself — even when this page is hidden, hung or
 * disconnected — and only reports the result here for feedback.
 */
export interface TrayStopAllOutcome {
  ok: boolean;
  agentsStopped: number;
  approvalsCancelled: number;
  error: string | null;
}

/**
 * Shell behaviour (tray, close-to-tray, quit, fullscreen). These are window
 * management calls, never business data (§67); in a plain browser they are no-ops.
 */
export const shellControls = {
  /** Mirrors window-behaviour settings into the native shell. */
  async applyPrefs(prefs: { closeToTray: boolean }): Promise<void> {
    if (isMainWindow()) await invoke("shell_set_prefs", { prefs });
  },
  /** Quits the app (stops an embedded core) even when close-to-tray is on. */
  async quit(): Promise<void> {
    if (isTauri()) await invoke("shell_quit");
  },
  async hideWindow(): Promise<void> {
    if (isTauri()) await getCurrentWindow().hide();
  },
  async toggleFullscreen(): Promise<void> {
    if (isTauri()) {
      const w = getCurrentWindow();
      await w.setFullscreen(!(await w.isFullscreen()));
    } else if (typeof document !== "undefined") {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await document.documentElement.requestFullscreen();
    }
  },
  /** Localizes the native tray menu (and other shell-owned texts). */
  async setUiLanguage(language: string): Promise<void> {
    if (isMainWindow()) await invoke("set_ui_language", { language });
  },
  /** Subscribes to the outcome of the tray's "Stop All AI" (already executed by the shell). */
  async onTrayStopAll(handler: (outcome: TrayStopAllOutcome) => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen<TrayStopAllOutcome>(TRAY_STOP_ALL_EVENT, (e) => handler(e.payload));
  },
};

export const windowControls = {
  async minimize(): Promise<void> {
    if (isTauri()) await getCurrentWindow().minimize();
  },
  async toggleMaximize(): Promise<void> {
    if (isTauri()) await getCurrentWindow().toggleMaximize();
  },
  async close(): Promise<void> {
    if (isTauri()) await getCurrentWindow().close();
  },
  async isMaximized(): Promise<boolean> {
    return isTauri() ? getCurrentWindow().isMaximized() : false;
  },
  /** Brings this window to the front (e.g. it received work from another window). */
  async focus(): Promise<void> {
    if (isTauri()) await getCurrentWindow().setFocus();
    else if (typeof window !== "undefined") window.focus();
  },
  /** Subscribes to resize events (maximize state changes). */
  async onResized(handler: () => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return getCurrentWindow().onResized(handler);
  },
};

/** Options of the native pickers. */
export interface PickOptions {
  /** Dialog title (localized by the caller). */
  title?: string;
  /** Folder (or file) the dialog starts in. */
  defaultPath?: string;
}

/**
 * Whether native folder/file pickers are available: only inside the desktop
 * shell (the main window and detached panels are granted `dialog:allow-open`).
 * In a plain browser the "Browse…" buttons are hidden and paths are typed.
 */
export function canPickPaths(): boolean {
  return isTauri();
}

/** First path of a dialog result (`null` when cancelled). */
function firstPath(result: unknown): string | null {
  if (typeof result === "string") return result || null;
  if (Array.isArray(result)) {
    const first: unknown = result[0];
    return typeof first === "string" && first ? first : null;
  }
  return null;
}

async function pick(directory: boolean, opts: PickOptions): Promise<string | null> {
  if (!canPickPaths()) return null;
  const options: { directory: boolean; multiple: false; title?: string; defaultPath?: string } = { directory, multiple: false };
  if (opts.title) options.title = opts.title;
  if (opts.defaultPath) options.defaultPath = opts.defaultPath;
  return firstPath(await openDialog(options));
}

/** Shows the native "choose folder" dialog; resolves to the absolute path, or `null` (cancelled / not in the shell). */
export function pickFolder(opts: PickOptions = {}): Promise<string | null> {
  return pick(true, opts);
}

/** Shows the native "open file" dialog; resolves to the absolute path, or `null` (cancelled / not in the shell). */
export function pickFile(opts: PickOptions = {}): Promise<string | null> {
  return pick(false, opts);
}

/** Shows the native "save file" dialog; resolves to the chosen absolute path, or `null` (cancelled / not in the shell). */
export async function pickSavePath(opts: PickOptions = {}): Promise<string | null> {
  if (!canPickPaths()) return null;
  const options: { title?: string; defaultPath?: string } = {};
  if (opts.title) options.title = opts.title;
  if (opts.defaultPath) options.defaultPath = opts.defaultPath;
  return firstPath(await saveDialog(options));
}
