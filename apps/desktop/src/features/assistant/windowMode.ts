/**
 * Main-window geometry per UI mode (ADR-0035): Assistant mode turns the main
 * window into a compact, centred, always-on-top panel; Orchestra mode
 * restores the previous size / position / maximized state. Browsers and
 * tests: no-op (the layout still switches).
 */
import { getCurrentWindow, LogicalSize, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";

import { isMainWindow } from "../../lib/tauri";
import type { AssistantUiMode } from "./store";

const GEOMETRY_KEY = "orkestr.assistant.orchestraGeometry";
/** Assistant panel size (logical px). */
export const ASSISTANT_SIZE = { width: 460, height: 680 } as const;

interface Geometry {
  width: number;
  height: number;
  x: number;
  y: number;
  maximized: boolean;
}

function saveGeometry(g: Geometry): void {
  try {
    window.localStorage.setItem(GEOMETRY_KEY, JSON.stringify(g));
  } catch {
    /* not remembered */
  }
}

function loadGeometry(): Geometry | null {
  try {
    const raw = window.localStorage.getItem(GEOMETRY_KEY);
    return raw ? (JSON.parse(raw) as Geometry) : null;
  } catch {
    return null;
  }
}

let applied: AssistantUiMode | null = null;

/** Applies the window geometry of `mode` (idempotent). */
export async function applyWindowMode(mode: AssistantUiMode): Promise<void> {
  if (!isMainWindow() || applied === mode) return;
  const first = applied === null;
  applied = mode;
  const w = getCurrentWindow();
  try {
    if (mode === "assistant") {
      // On start-up in Assistant mode the stored Orchestra geometry is kept.
      if (!first || !loadGeometry()) {
        const [size, pos, maximized] = await Promise.all([w.outerSize(), w.outerPosition(), w.isMaximized()]);
        saveGeometry({ width: size.width, height: size.height, x: pos.x, y: pos.y, maximized });
      }
      if (await w.isMaximized()) await w.unmaximize();
      await w.setSize(new LogicalSize(ASSISTANT_SIZE.width, ASSISTANT_SIZE.height));
      await w.center();
      await w.setAlwaysOnTop(true);
    } else {
      await w.setAlwaysOnTop(false);
      if (first) return; // started in Orchestra mode: nothing to restore
      const g = loadGeometry();
      if (g) {
        await w.setSize(new PhysicalSize(g.width, g.height));
        await w.setPosition(new PhysicalPosition(g.x, g.y));
        if (g.maximized) await w.maximize();
      } else {
        await w.maximize();
      }
    }
  } catch (e) {
    console.warn("assistant window mode", e);
  }
}
