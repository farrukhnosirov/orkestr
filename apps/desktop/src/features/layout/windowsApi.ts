/**
 * Shell window management for detached panels and monitors
 * (`src-tauri/src/windows.rs`). Geometry only — never business data (§67).
 * Outside Tauri (browser dev mode, tests) every call is a harmless no-op and
 * detaching falls back to an in-app floating group.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { isTauri } from "../../lib/tauri";
import type { MonitorInfo, WindowInfo } from "./selection";

export const EVT_REDOCK = "orkestr://layout/redock";
export const EVT_PANEL_CLOSED = "orkestr://layout/panel-closed";
export const EVT_WINDOWS_CHANGED = "orkestr://layout/windows-changed";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface OpenPanelRequest {
  panelId: string;
  projectId?: string | null;
  title?: string;
  monitor?: string | null;
  bounds?: Rect | null;
  maximized?: boolean;
}

export interface RedockPayload {
  label: string;
  panelId: string;
  projectId: string | null;
}

/** Whether panels can be detached into OS windows here. */
export const canDetach = (): boolean => isTauri();

export const windowsApi = {
  async listMonitors(): Promise<MonitorInfo[]> {
    return isTauri() ? invoke<MonitorInfo[]>("window_list_monitors") : [];
  },
  async openPanel(req: OpenPanelRequest): Promise<WindowInfo | null> {
    if (!isTauri()) return null;
    return invoke<WindowInfo>("window_open_panel", {
      req: {
        panelId: req.panelId,
        projectId: req.projectId ?? null,
        title: req.title ?? null,
        monitor: req.monitor ?? null,
        bounds: req.bounds ?? null,
        maximized: req.maximized ?? false,
      },
    });
  },
  async closePanel(label: string): Promise<void> {
    if (isTauri()) await invoke("window_close_panel", { label });
  },
  /** From inside a detached window: give the panel back to the main window and close. */
  async redockSelf(): Promise<void> {
    if (isTauri()) await invoke("window_redock_panel");
  },
  async listPanels(): Promise<WindowInfo[]> {
    return isTauri() ? invoke<WindowInfo[]>("window_list_panels") : [];
  },
  async mainPlacement(): Promise<WindowInfo | null> {
    return isTauri() ? invoke<WindowInfo | null>("window_main_placement") : null;
  },
  async moveToMonitor(label: string, monitor: string, bounds?: Rect | null, maximized?: boolean): Promise<WindowInfo | null> {
    if (!isTauri()) return null;
    return invoke<WindowInfo>("window_move_to_monitor", { label, monitor, bounds: bounds ?? null, maximized: maximized ?? null });
  },
  /** Moves windows stranded on a disconnected monitor back to the primary one. */
  async reconcile(): Promise<string[]> {
    return isTauri() ? invoke<string[]>("window_reconcile") : [];
  },
  async onRedock(handler: (p: RedockPayload) => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen<RedockPayload>(EVT_REDOCK, (e) => handler(e.payload));
  },
  async onPanelClosed(handler: (p: RedockPayload) => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen<RedockPayload>(EVT_PANEL_CLOSED, (e) => handler(e.payload));
  },
  async onWindowsChanged(handler: () => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen(EVT_WINDOWS_CHANGED, () => handler());
  },
};
