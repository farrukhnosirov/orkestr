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
/** Assistant-mode size / position (remembered separately from Orchestra mode). */
const ASSISTANT_GEOMETRY_KEY = "orkestr.assistant.assistantGeometry";
/** Assistant panel default size (logical px). */
export const ASSISTANT_SIZE = { width: 460, height: 680 } as const;
/** Smallest Assistant window the user can resize to (logical px). */
export const ASSISTANT_MIN_SIZE = { width: 400, height: 520 } as const;
/** Orchestra-mode minimum (tauri.conf.json `minWidth` / `minHeight`). */
const ORCHESTRA_MIN_SIZE = { width: 960, height: 600 } as const;

interface Geometry {
  width: number;
  height: number;
  x: number;
  y: number;
  maximized: boolean;
}

function saveGeometry(g: Geometry, key = GEOMETRY_KEY): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(g));
  } catch {
    /* not remembered */
  }
}

function loadGeometry(key = GEOMETRY_KEY): Geometry | null {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as Geometry) : null;
  } catch {
    return null;
  }
}

let applied: AssistantUiMode | null = null;
let stopTracking: (() => void) | null = null;

/** Remembers the Assistant window's size / position (not while minimized). */
async function captureAssistantGeometry(): Promise<void> {
  const w = getCurrentWindow();
  const [size, pos] = await Promise.all([w.outerSize(), w.outerPosition()]);
  if (size.width < 100 || size.height < 100 || pos.x <= -30000 || pos.y <= -30000) return; // minimized
  saveGeometry({ width: size.width, height: size.height, x: pos.x, y: pos.y, maximized: false }, ASSISTANT_GEOMETRY_KEY);
}

/** While in Assistant mode, user resizes / moves are remembered (debounced). */
async function trackAssistantGeometry(): Promise<void> {
  const w = getCurrentWindow();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (applied === "assistant") void captureAssistantGeometry().catch(() => {});
    }, 400);
  };
  const [a, b] = await Promise.all([w.onResized(schedule), w.onMoved(schedule)]);
  stopTracking = () => {
    clearTimeout(timer);
    a();
    b();
  };
}

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
      await w.setMinSize(new LogicalSize(ASSISTANT_MIN_SIZE.width, ASSISTANT_MIN_SIZE.height));
      const own = loadGeometry(ASSISTANT_GEOMETRY_KEY);
      if (own) {
        await w.setSize(new PhysicalSize(own.width, own.height));
        await w.setPosition(new PhysicalPosition(own.x, own.y));
      } else {
        await w.setSize(new LogicalSize(ASSISTANT_SIZE.width, ASSISTANT_SIZE.height));
        await w.center();
      }
      await w.setAlwaysOnTop(true);
      stopTracking?.();
      await trackAssistantGeometry();
    } else {
      stopTracking?.();
      stopTracking = null;
      if (!first) await captureAssistantGeometry().catch(() => {});
      await w.setAlwaysOnTop(false);
      await w.setMinSize(new LogicalSize(ORCHESTRA_MIN_SIZE.width, ORCHESTRA_MIN_SIZE.height));
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
