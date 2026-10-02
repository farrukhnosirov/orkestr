/**
 * Thin wrapper around the Tauri APIs the shell needs: window controls and the
 * `core_connection` command. Business data never goes through Tauri (§67) —
 * everything else uses @orkestr/client.
 */
import { invoke, isTauri as tauriDetected } from "@tauri-apps/api/core";
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

/** Event emitted by the tray's "Stop All AI" item (see src-tauri `TRAY_STOP_ALL_EVENT`). */
export const TRAY_STOP_ALL_EVENT = "orkestr://tray/stop-all";

/**
 * Shell behaviour (tray, close-to-tray, quit, fullscreen). These are window
 * management calls, never business data (§67); in a plain browser they are no-ops.
 */
export const shellControls = {
  /** Mirrors window-behaviour settings into the native shell. */
  async applyPrefs(prefs: { closeToTray: boolean }): Promise<void> {
    if (isTauri()) await invoke("shell_set_prefs", { prefs });
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
    if (isTauri()) await invoke("set_ui_language", { language });
  },
  /** Subscribes to the tray's "Stop All AI" item. */
  async onTrayStopAll(handler: () => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen(TRAY_STOP_ALL_EVENT, () => handler());
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
  /** Subscribes to resize events (maximize state changes). */
  async onResized(handler: () => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return getCurrentWindow().onResized(handler);
  },
};
