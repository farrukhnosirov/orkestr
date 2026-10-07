/**
 * Cross-panel glue of the Claude Code panel:
 *
 * - the Code editor registers a *selection source* while it is mounted, so
 *   the palette command «Claude Code: спросить о выделенном» can read the
 *   current selection;
 * - "Ask Claude about selection" (editor context menu / command) sends the
 *   selection with `path:line` as a panel intent (it may live in another
 *   window); the window hosting the panel puts it into the composer;
 * - opening the panel and files changed by Claude.
 */
import { useEffect, useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { ASK_CLAUDE_EVENT, askClaude, openInEditor } from "../files/actions";
import { selectionPrompt } from "./model";
import { useClaudeStore } from "./store";

/** Panel id. */
export const CLAUDE_PANEL_ID = "claudeCode";

/** A selection in the Code editor. */
export interface EditorSelection {
  path: string;
  startLine: number;
  endLine: number;
  text: string;
  language?: string | null;
}

let selectionSource: (() => EditorSelection | null) | null = null;
const selectionListeners = new Set<() => void>();

/** Tells subscribers (the composer's selection chip) that the editor selection changed. */
export function notifySelectionChanged(): void {
  for (const l of selectionListeners) l();
}

/** Registers the Code editor's selection getter (returns a disposer). */
export function setSelectionSource(source: () => EditorSelection | null): () => void {
  selectionSource = source;
  notifySelectionChanged();
  return () => {
    if (selectionSource === source) {
      selectionSource = null;
      notifySelectionChanged();
    }
  };
}

/** The current editor selection, re-rendering when it changes. */
export function useEditorSelection(): EditorSelection | null {
  const [, setVersion] = useState(0);
  useEffect(() => {
    const l = () => setVersion((v) => v + 1);
    selectionListeners.add(l);
    return () => {
      selectionListeners.delete(l);
    };
  }, []);
  return currentSelection();
}

/** The current non-empty editor selection, if any. */
export function currentSelection(): EditorSelection | null {
  const sel = selectionSource?.() ?? null;
  return sel && sel.text.trim() ? sel : null;
}

/** Opens the Claude Code panel. */
export function openClaudePanel(): void {
  workspaceActions.openPanel(CLAUDE_PANEL_ID);
}

/** Sends a selection to the Claude Code composer (with `@path:lines`). */
export function askClaudeAbout(sel: EditorSelection): void {
  askClaude(selectionPrompt(sel));
}

/** Palette command: the current selection, or just the panel. */
export function askClaudeAboutCurrentSelection(): void {
  const sel = currentSelection();
  if (sel) askClaudeAbout(sel);
  else openClaudePanel();
}

/** Opens a project file Claude touched in the Code panel. */
export function openChangedFile(relPath: string, line?: number): void {
  openInEditor(relPath, line ? { line } : undefined);
}

// The window hosting the panel receives "ask" intents as a window event.
if (typeof window !== "undefined") {
  window.addEventListener(ASK_CLAUDE_EVENT, (e) => {
    const text = (e as CustomEvent<unknown>).detail;
    if (typeof text === "string" && text) useClaudeStore.getState().requestInsert(text);
  });
}
