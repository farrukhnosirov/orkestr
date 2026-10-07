/**
 * Cross-panel actions: open a file in the Code panel, "Ask Manager about
 * selection" (the Manager panel listens for the `orkestr:ask-manager` window
 * event) and "Ask Claude about selection" (`orkestr:ask-claude`, consumed by
 * the Claude Code feature).
 *
 * A panel lives in exactly one window: docked in the main window, or popped
 * out into its own window (§28, ADR-0013). Each window is a separate webview
 * with its own stores, so an action aimed at a panel that lives in another
 * window is forwarded there as a *panel intent* over a same-origin
 * `BroadcastChannel` (plain data only), and only the window that hosts the
 * panel applies it. Windows declare what they host with `hostPanelIntents`.
 */
import { workspaceActions } from "../../components/workspaceActions";
import { windowControls } from "../../lib/tauri";
import { type RevealTarget, useEditorStore } from "../../store/editor";
import { requestSave } from "../workbench/saveBus";

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

/** Event name consumed by the Claude Code panel (detail: the prompt text to insert). */
export const ASK_CLAUDE_EVENT = "orkestr:ask-claude";

/** Work for a panel that may live in another window. */
export type PanelIntent =
  | { type: "openFile"; path: string; at?: RevealTarget }
  | { type: "askManager"; detail: AskManagerDetail }
  /** Text for the Claude Code composer ("Ask Claude about selection"). */
  | { type: "askClaude"; text: string }
  /** File → Save / Save all for the Code panel. */
  | { type: "saveFiles"; all: boolean };

const INTENT_CHANNEL = "orkestr.panel-intents";

/** The panel each intent is for. */
export function intentPanel(intent: PanelIntent): string {
  if (intent.type === "askManager") return "manager";
  if (intent.type === "askClaude") return "claudeCode";
  return "code";
}

/** Whether this window hosts `panelId` (`null` = not declared: everything is local). */
let hosts: ((panelId: string) => boolean) | null = null;
let channel: BroadcastChannel | null = null;

function applyLocally(intent: PanelIntent): void {
  if (intent.type === "openFile") {
    useEditorStore.getState().openFile(intent.path, intent.at);
    workspaceActions.openPanel("code");
  } else if (intent.type === "saveFiles") {
    // Unsaved tabs but no mounted editor: show the Code panel; the save runs when it mounts.
    if (requestSave(intent.all) === "queued") workspaceActions.openPanel("code");
  } else if (intent.type === "askClaude") {
    window.dispatchEvent(new CustomEvent<string>(ASK_CLAUDE_EVENT, { detail: intent.text }));
    workspaceActions.openPanel("claudeCode");
  } else {
    window.dispatchEvent(new CustomEvent<AskManagerDetail>(ASK_MANAGER_EVENT, { detail: intent.detail }));
    workspaceActions.openPanel("manager");
  }
}

function isIntent(v: unknown): v is PanelIntent {
  if (!v || typeof v !== "object") return false;
  const o = v as { type?: unknown; path?: unknown; detail?: unknown };
  if (o.type === "openFile") return typeof o.path === "string";
  if (o.type === "askManager") return !!o.detail && typeof (o.detail as { text?: unknown }).text === "string";
  if (o.type === "saveFiles") return typeof (o as { all?: unknown }).all === "boolean";
  if (o.type === "askClaude") return typeof (o as { text?: unknown }).text === "string";
  return false;
}

/** Applies the intent here when this window hosts its panel, else forwards it to the window that does. */
export function dispatchPanelIntent(intent: PanelIntent): void {
  const panelId = intentPanel(intent);
  if (!hosts || !channel || hosts(panelId)) {
    applyLocally(intent);
    return;
  }
  try {
    channel.postMessage(intent);
  } catch {
    applyLocally(intent);
    return;
  }
  // Main window: focuses the detached window that hosts the panel (a save
  // request leaves the focus where it is).
  if (intent.type !== "saveFiles") workspaceActions.openPanel(panelId);
}

/**
 * Declares which panels this window hosts and applies intents forwarded from
 * other windows for them. Returns a disposer.
 */
export function hostPanelIntents(hostsPanel: (panelId: string) => boolean): () => void {
  let ch: BroadcastChannel | null = null;
  try {
    ch = typeof BroadcastChannel === "function" ? new BroadcastChannel(INTENT_CHANNEL) : null;
  } catch {
    ch = null;
  }
  hosts = hostsPanel;
  channel = ch;
  if (ch) {
    ch.onmessage = (e: MessageEvent<unknown>) => {
      const intent = e.data;
      if (!isIntent(intent) || !hostsPanel(intentPanel(intent))) return;
      applyLocally(intent);
      if (intent.type !== "saveFiles") void windowControls.focus().catch(() => undefined);
    };
  }
  return () => {
    if (hosts === hostsPanel) {
      hosts = null;
      channel = null;
    }
    if (ch) {
      ch.onmessage = null;
      ch.close();
    }
  };
}

/** Opens `path` in the Code panel (focusing the panel), in whichever window hosts it. */
export function openInEditor(path: string, at?: RevealTarget): void {
  dispatchPanelIntent(at ? { type: "openFile", path, at } : { type: "openFile", path });
}

/** Saves the active Code tab (or all unsaved tabs), in whichever window hosts the Code panel. */
export function saveEditor(all: boolean): void {
  dispatchPanelIntent({ type: "saveFiles", all });
}

/** Sends text to the Claude Code composer, in whichever window hosts the panel. */
export function askClaude(text: string): void {
  dispatchPanelIntent({ type: "askClaude", text });
}

/** Sends the ask-manager request to the Manager panel, in whichever window hosts it. */
export function askManager(detail: AskManagerDetail): void {
  dispatchPanelIntent({ type: "askManager", detail });
}
