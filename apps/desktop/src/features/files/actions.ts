/**
 * Cross-panel actions: open a file in the Code panel, and the inline AI
 * placeholder "Ask Manager about selection" (the Manager panel listens for
 * the `orkestr:ask-manager` window event in wave B).
 */
import { workspaceActions } from "../../components/workspaceActions";
import { type RevealTarget, useEditorStore } from "../../store/editor";

/** Opens `path` in the Code panel (focusing the panel). */
export function openInEditor(path: string, at?: RevealTarget): void {
  useEditorStore.getState().openFile(path, at);
  workspaceActions.openPanel("code");
}

/** Detail of the `orkestr:ask-manager` event. */
export interface AskManagerDetail {
  text: string;
  projectId: string | null;
  path?: string;
  startLine?: number;
  endLine?: number;
  language?: string;
}

/** Event name consumed by the Manager panel. */
export const ASK_MANAGER_EVENT = "orkestr:ask-manager";

/** Dispatches the ask-manager request. */
export function askManager(detail: AskManagerDetail): void {
  window.dispatchEvent(new CustomEvent<AskManagerDetail>(ASK_MANAGER_EVENT, { detail }));
  workspaceActions.openPanel("manager");
}
