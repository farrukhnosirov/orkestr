/**
 * Thin wrapper around the Tauri APIs the shell needs: window controls and the
 * `core_connection` command. Business data never goes through Tauri (§67) —
 * everything else uses @orkestr/client.
 */
import { invoke, isTauri as tauriDetected } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

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
    throw new Error(
      "Not running inside the desktop shell. Start `orkestrd` and open this page with ?core=<ws url>&token=<token> " +
        "(see <data dir>/core.json and core.token).",
    );
  }
  return { url, token, mode: "browser", startupMs: 0 };
}

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
