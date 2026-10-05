/**
 * Undo / redo history of applied UI patches (§29: every AI-made UI change
 * can be reverted). Immutable helpers over a plain state object so the
 * zustand store stays serialisable and the logic is unit-testable.
 *
 * Each entry stores the patch that undoes it (`inverse`) and the patch that
 * re-applies it (`forward`); both come from the core (`ui.applyPatch`
 * returns the inverse of what it stored), so undo/redo are exact even when
 * the original patch used `-` appends or lenient shapes.
 */
import type { UiPatchActor, UiPatchOp } from "@orkestr/protocol";

/** Max entries kept in either direction. */
export const MAX_UNDO = 50;

export interface UndoEntry {
  /** Stable id (proposal id or a local id). */
  id: string;
  /** Summary shown in the history list. */
  summary: string;
  /** Who authored the original change. */
  author: UiPatchActor;
  /** Re-applies the change. */
  forward: UiPatchOp[];
  /** Reverts the change. */
  inverse: UiPatchOp[];
  /** When it was applied (ms). */
  at: number;
}

export interface UndoState {
  past: UndoEntry[];
  future: UndoEntry[];
}

export const EMPTY_UNDO: UndoState = { past: [], future: [] };

/** Records a newly applied change (clears the redo branch). */
export function recordApplied(s: UndoState, entry: UndoEntry): UndoState {
  if (entry.inverse.length === 0) return s; // nothing changed → nothing to undo
  return { past: [...s.past, entry].slice(-MAX_UNDO), future: [] };
}

export function canUndo(s: UndoState): boolean {
  return s.past.length > 0;
}

export function canRedo(s: UndoState): boolean {
  return s.future.length > 0;
}

/** The entry an undo would revert. */
export function peekUndo(s: UndoState): UndoEntry | null {
  return s.past[s.past.length - 1] ?? null;
}

/** The entry a redo would re-apply. */
export function peekRedo(s: UndoState): UndoEntry | null {
  return s.future[s.future.length - 1] ?? null;
}

/**
 * After the inverse of the newest entry was applied: moves it to the redo
 * branch. `redoPatch` is what the core returned as the inverse of the undo
 * (i.e. the exact patch that re-applies the change).
 */
export function completeUndo(s: UndoState, redoPatch: UiPatchOp[]): UndoState {
  const top = peekUndo(s);
  if (!top) return s;
  const moved: UndoEntry = { ...top, forward: redoPatch.length ? redoPatch : top.forward };
  return { past: s.past.slice(0, -1), future: [...s.future, moved].slice(-MAX_UNDO) };
}

/** After a redo was applied: moves the entry back with the fresh inverse. */
export function completeRedo(s: UndoState, undoPatch: UiPatchOp[]): UndoState {
  const top = peekRedo(s);
  if (!top) return s;
  const moved: UndoEntry = { ...top, inverse: undoPatch.length ? undoPatch : top.inverse, at: Date.now() };
  return { past: [...s.past, moved].slice(-MAX_UNDO), future: s.future.slice(0, -1) };
}
