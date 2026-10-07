/**
 * The VS Code window (src-tauri `vscode_window.rs`). Window management only:
 * the URL and connection token come from the core (`vscode.start`). In a
 * plain browser (dev / web) the UI opens in a new browser tab instead.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { isTauri } from "../../lib/tauri";

/** Event emitted by the shell when the VS Code window opens or closes. */
export const VSCODE_WINDOW_EVENT = "orkestr://vscode/window";

export const vscodeWindow = {
  /** Opens (or focuses and re-targets) the VS Code window. */
  async open(url: string, title: string): Promise<void> {
    if (isTauri()) {
      await invoke("vscode_window_open", { url, title });
      return;
    }
    window.open(url, "orkestr-vscode", "noopener");
  },
  async close(): Promise<void> {
    if (isTauri()) await invoke("vscode_window_close");
  },
  async reload(): Promise<void> {
    if (isTauri()) await invoke("vscode_window_reload");
  },
  async isOpen(): Promise<boolean> {
    if (!isTauri()) return false;
    return invoke<boolean>("vscode_window_is_open");
  },
  /** Opens the VS Code UI in the system browser (loopback URLs only). */
  async openExternal(url: string): Promise<void> {
    if (isTauri()) {
      await invoke("vscode_open_external", { url });
      return;
    }
    window.open(url, "_blank", "noopener");
  },
  /** Subscribes to window open/close (no-op outside Tauri). */
  async onChange(cb: (open: boolean) => void): Promise<UnlistenFn> {
    if (!isTauri()) return () => undefined;
    return listen<{ open: boolean }>(VSCODE_WINDOW_EVENT, (e) => cb(e.payload.open));
  },
};
