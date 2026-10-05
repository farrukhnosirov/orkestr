/**
 * HUD window management (§34) — window calls only, never business data
 * (§67): show / hide / toggle the always-on-top HUD window, collapse the
 * main window into it and back, and remember its position per device
 * (local storage shared by both windows of this app).
 *
 * In a plain browser (dev / tests) the HUD renders inside the page when the
 * URL has `?hud=1`; the window calls are no-ops.
 */
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isTauri } from "../../lib/tauri";
import { HUD_POSITION_KEY, parsePosition, serializePosition, type HudPosition } from "./hudModel";

/** Label of the HUD window (see src-tauri `hud::HUD_WINDOW`). */
export const HUD_WINDOW_LABEL = "hud";

/** Whether this webview renders the HUD. */
export function isHudWindow(): boolean {
  if (isTauri()) {
    try {
      return getCurrentWindow().label === HUD_WINDOW_LABEL;
    } catch {
      return false;
    }
  }
  try {
    return new URLSearchParams(window.location.search).get("hud") === "1";
  } catch {
    return false;
  }
}

export function loadHudPosition(): HudPosition | null {
  try {
    return parsePosition(window.localStorage.getItem(HUD_POSITION_KEY));
  } catch {
    return null;
  }
}

export function saveHudPosition(p: HudPosition): void {
  try {
    window.localStorage.setItem(HUD_POSITION_KEY, serializePosition(p));
  } catch {
    /* storage unavailable: the HUD opens in the default corner next time */
  }
}

export const hudControls = {
  /** Whether the HUD can be opened here (desktop app only). */
  available(): boolean {
    return isTauri();
  },
  async toggle(): Promise<boolean> {
    if (!isTauri()) return false;
    return invoke<boolean>("hud_toggle", { position: loadHudPosition() });
  },
  async show(): Promise<void> {
    if (isTauri()) await invoke("hud_show", { position: loadHudPosition() });
  },
  async hide(): Promise<void> {
    if (isTauri()) await invoke("hud_hide");
  },
  /** Hides the main window and shows the HUD (agents keep working). */
  async collapse(): Promise<void> {
    if (isTauri()) await invoke("hud_collapse", { position: loadHudPosition() });
  },
  /** Brings the main window back and hides the HUD. */
  async expand(): Promise<void> {
    if (isTauri()) await invoke("hud_expand");
  },
  /** Starts dragging the HUD window (from a pointer-down on its handle). */
  async startDragging(): Promise<void> {
    if (isTauri()) await getCurrentWindow().startDragging();
  },
  /**
   * Remembers the HUD position whenever it moves (debounced). Returns the
   * unsubscribe function.
   */
  async trackPosition(): Promise<() => void> {
    if (!isTauri()) return () => {};
    let timer = 0;
    const off = await getCurrentWindow().onMoved(({ payload }) => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => saveHudPosition({ x: payload.x, y: payload.y }), 300);
    });
    return () => {
      window.clearTimeout(timer);
      off();
    };
  },
};
