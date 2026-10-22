/**
 * File → Save / Save all (Ctrl+S, Ctrl+Shift+S) for the Code panel.
 *
 * The Code panel's workbench registers the handler while it is mounted (it
 * owns save-conflict handling). A request made while it is not mounted but
 * the editor has unsaved tabs opens the panel and is delivered on mount —
 * unless it is stale by then (the user moved on).
 */
import { useEditorStore } from "../../store/editor";

export type SaveHandler = (all: boolean) => void;

/** How long a request waits for the Code panel to mount. */
export const PENDING_SAVE_TTL_MS = 5000;

let handler: SaveHandler | null = null;
let pending: { all: boolean; at: number } | null = null;

/** Registers the workbench save handler; delivers a fresh pending request. Returns the disposer. */
export function registerSaveHandler(h: SaveHandler): () => void {
  handler = h;
  const p = pending;
  pending = null;
  if (p && Date.now() - p.at <= PENDING_SAVE_TTL_MS) h(p.all);
  return () => {
    if (handler === h) handler = null;
  };
}

/**
 * Saves the active editor tab (`all = false`) or every unsaved tab. Returns
 * whether the request was taken (`"saved"`), queued until the Code panel
 * mounts (`"queued"`; the caller should open the panel) or there was
 * nothing to save (`"nothing"`).
 */
export function requestSave(all: boolean): "saved" | "queued" | "nothing" {
  if (handler) {
    handler(all);
    return "saved";
  }
  if (!useEditorStore.getState().tabs.some((t) => t.dirty)) return "nothing";
  pending = { all, at: Date.now() };
  return "queued";
}

let saveAsHandler: (() => void) | null = null;

/** Registers the workbench "Save As…" handler (it shows the save dialog). Returns the disposer. */
export function registerSaveAsHandler(h: () => void): () => void {
  saveAsHandler = h;
  return () => {
    if (saveAsHandler === h) saveAsHandler = null;
  };
}

/** File → Save As… for the active editor tab. Returns whether the Code panel took it (false = no editor is mounted). */
export function requestSaveAs(): boolean {
  if (!saveAsHandler) return false;
  saveAsHandler();
  return true;
}

/** Test hook. */
export function resetSaveBus(): void {
  handler = null;
  pending = null;
  saveAsHandler = null;
}
