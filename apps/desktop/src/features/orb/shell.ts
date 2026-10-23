/**
 * Window management of the orb (src-tauri `orb.rs`). These calls only show,
 * hide, lay out and move the orb window and pass window-level signals; agent
 * data always comes from the core over RPC (§67). Outside Tauri they are no-ops.
 *
 * The orb's position is remembered per device and monitor in local storage
 * (shared by every window of this app), most recently used monitor first.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isTauri } from "../../lib/tauri";
import type { HitShape } from "./model";

/** Label of the orb window (see src-tauri `orb::ORB_WINDOW`). */
export const ORB_WINDOW_LABEL = "orb";

export const ORB_EVENTS = {
  /** shell → orb: cursor entered / left the interactive shape (`boolean`). */
  hover: "orkestr://orb/hover",
  /** shell → orb: drag started / ended (`boolean`). */
  drag: "orkestr://orb/drag",
  /** shell → orb: settled position after snapping (`{x, y, monitor}`). */
  moved: "orkestr://orb/moved",
  /** shell → orb: toggle the hub (hotkey). */
  hub: "orkestr://orb/hub",
  /** orb → main (via `orb_to_main`). */
  openMain: "orkestr://orb/open-main",
  openAgent: "orkestr://orb/open-agent",
  openClaude: "orkestr://orb/open-claude",
  openSettings: "orkestr://orb/open-settings",
  openApprovals: "orkestr://orb/open-approvals",
} as const;

export type MainRequest = "open-main" | "open-agent" | "open-claude" | "open-settings" | "open-approvals";

export interface OrbMoved {
  x: number;
  y: number;
  monitor: string | null;
}

export interface OrbLayout {
  orbX: number;
  orbY: number;
  hubX: number;
  hubY: number;
  side: "left" | "right";
}

export interface SavedPosition {
  monitor: string | null;
  x: number;
  y: number;
}

export const POSITIONS_KEY = "orkestr.orb.positions";
const MAX_POSITIONS = 8;

/** Parses the stored positions (most recent first); garbage → []. */
export function parsePositions(raw: string | null): SavedPosition[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v
      .filter(
        (p): p is SavedPosition =>
          !!p &&
          typeof p === "object" &&
          Number.isFinite((p as SavedPosition).x) &&
          Number.isFinite((p as SavedPosition).y) &&
          ((p as SavedPosition).monitor === null || typeof (p as SavedPosition).monitor === "string"),
      )
      .slice(0, MAX_POSITIONS);
  } catch {
    return [];
  }
}

/** Puts `p` first, replacing the entry of the same monitor. */
export function rememberPosition(list: readonly SavedPosition[], p: SavedPosition): SavedPosition[] {
  return [p, ...list.filter((q) => q.monitor !== p.monitor)].slice(0, MAX_POSITIONS);
}

export function loadPositions(): SavedPosition[] {
  try {
    return parsePositions(window.localStorage.getItem(POSITIONS_KEY));
  } catch {
    return [];
  }
}

export function savePosition(p: SavedPosition): void {
  try {
    window.localStorage.setItem(POSITIONS_KEY, JSON.stringify(rememberPosition(loadPositions(), p)));
  } catch {
    // Storage unavailable: the orb opens in the default corner next time.
  }
}

export function clearPositions(): void {
  try {
    window.localStorage.removeItem(POSITIONS_KEY);
  } catch {
    // Nothing remembered anyway.
  }
}

/** Whether this webview renders the orb (window label, or `?orb=1` in a browser). */
export function isOrbWindow(): boolean {
  if (isTauri()) {
    try {
      return getCurrentWindow().label === ORB_WINDOW_LABEL;
    } catch {
      return false;
    }
  }
  try {
    return window.location.hash === "#/orb" || new URLSearchParams(window.location.search).get("orb") === "1";
  } catch {
    return false;
  }
}

export const orbShell = {
  available: (): boolean => isTauri(),
  /** Main window: shows the orb (or applies a new box size). */
  async show(boxSize: number): Promise<void> {
    if (isTauri()) await invoke("orb_show", { args: { boxSize, positions: loadPositions() } });
  },
  async hide(): Promise<void> {
    if (isTauri()) await invoke("orb_hide");
  },
  /** Main window: toggles the hub (focuses the orb). `false` when the orb is not shown. */
  async toggleHub(): Promise<boolean> {
    if (!isTauri()) return false;
    return invoke<boolean>("orb_toggle_hub");
  },
  /** Main window: forget remembered positions and move to the default corner. */
  async resetPosition(): Promise<void> {
    clearPositions();
    if (isTauri()) await invoke("orb_reset_position");
  },
  /** Orb window: interactive shape (CSS px). */
  async setShape(shape: HitShape): Promise<void> {
    if (isTauri()) await invoke("orb_set_shape", { shape });
  },
  /** Orb window: open (`[w, h]`) or close (`null`) the hub area; returns where to draw. */
  async setLayout(hub: readonly [number, number] | null): Promise<OrbLayout | null> {
    if (!isTauri()) return null;
    return invoke<OrbLayout>("orb_set_layout", { hub });
  },
  async toMain(request: MainRequest, payload: Record<string, unknown> = {}): Promise<void> {
    if (isTauri()) await invoke("orb_to_main", { request, payload });
  },
  async startDragging(): Promise<void> {
    if (isTauri()) await getCurrentWindow().startDragging();
  },
  async on<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen<T>(event, (e) => handler(e.payload));
  },
};
