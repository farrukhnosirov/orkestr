/**
 * Edit commands for text fields (`<input>`, `<textarea>`, contenteditable)
 * used by the default context menu: Undo, Redo, Cut, Copy, Paste, Paste as
 * plain text, Select all.
 *
 * `document.execCommand` keeps the field's native undo stack and fires the
 * `input` events React listens to; where it is unavailable (or refused) the
 * text-control fallbacks edit `value` with `setRangeText` and dispatch
 * `input` themselves.
 */
import { readClipboardText, writeClipboardText } from "./clipboard";

export type TextControl = HTMLInputElement | HTMLTextAreaElement;

/** Input types that hold editable text (others — checkbox, range, … — are not text fields). */
const TEXT_INPUT_TYPES = new Set(["text", "search", "url", "tel", "email", "password", "number", ""]);

export function isTextControl(el: Element | null): el is TextControl {
  if (!el) return false;
  if (el instanceof HTMLTextAreaElement) return true;
  return el instanceof HTMLInputElement && TEXT_INPUT_TYPES.has(el.type.toLowerCase());
}

/** The editable element at or above `el` (text control or contenteditable host). */
export function editableTarget(el: Element | null): HTMLElement | null {
  if (!el) return null;
  if (isTextControl(el)) return el;
  const host = el.closest<HTMLElement>("[contenteditable]:not([contenteditable='false'])");
  return host && host.isContentEditable ? host : null;
}

/** Whether the field accepts input now. */
export function isWritable(el: HTMLElement): boolean {
  if (isTextControl(el)) return !el.readOnly && !el.disabled;
  return el.isContentEditable;
}

/** Selected text of a field (`""` when nothing is selected). */
export function fieldSelection(el: HTMLElement): string {
  if (isTextControl(el)) {
    // `number` / `email` inputs have no selection API.
    try {
      const { selectionStart: s, selectionEnd: e } = el;
      if (s === null || e === null || s === e) return "";
      return el.type === "password" ? "" : el.value.slice(s, e);
    } catch {
      return "";
    }
  }
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || !sel.anchorNode || !el.contains(sel.anchorNode)) return "";
  return sel.toString();
}

function exec(command: string, value?: string): boolean {
  if (typeof document.execCommand !== "function") return false;
  try {
    return document.execCommand(command, false, value);
  } catch {
    return false;
  }
}

/** Whether the browser reports the command as currently possible (`true` when unknown). */
export function commandEnabled(command: "undo" | "redo"): boolean {
  if (typeof document.queryCommandEnabled !== "function") return true;
  try {
    return document.queryCommandEnabled(command);
  } catch {
    return true;
  }
}

function fireInput(el: HTMLElement, inputType: string, data: string | null): void {
  el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType, data }));
}

/** Replaces the field's selection with `text` (insert at the caret when collapsed). */
export function insertText(el: HTMLElement, text: string): void {
  el.focus();
  if (exec("insertText", text)) return;
  if (isTextControl(el)) {
    const s = el.selectionStart ?? el.value.length;
    const e = el.selectionEnd ?? s;
    // React tracks `value` through its setter; setRangeText bypasses the tracker
    // so the following `input` event is seen as a change.
    el.setRangeText(text, s, e, "end");
    fireInput(el, "insertText", text);
    return;
  }
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  range.deleteContents();
  range.insertNode(document.createTextNode(text));
  range.collapse(false);
  fireInput(el, "insertText", text);
}

function deleteSelection(el: HTMLElement): void {
  el.focus();
  if (exec("delete")) return;
  if (isTextControl(el)) {
    const s = el.selectionStart ?? 0;
    const e = el.selectionEnd ?? s;
    if (s === e) return;
    el.setRangeText("", s, e, "end");
    fireInput(el, "deleteContentBackward", null);
    return;
  }
  window.getSelection()?.getRangeAt(0)?.deleteContents();
  fireInput(el, "deleteContentBackward", null);
}

export async function copyField(el: HTMLElement): Promise<void> {
  const text = fieldSelection(el);
  if (text) await writeClipboardText(text);
}

export async function cutField(el: HTMLElement): Promise<void> {
  const text = fieldSelection(el);
  if (!text || !isWritable(el)) return;
  if (await writeClipboardText(text)) deleteSelection(el);
}

/** Paste (always as plain text: fields are plain, and pasted HTML is never trusted). */
export async function pasteIntoField(el: HTMLElement): Promise<void> {
  if (!isWritable(el)) return;
  const text = await readClipboardText();
  if (text) insertText(el, text);
}

export function selectAllInField(el: HTMLElement): void {
  el.focus();
  if (isTextControl(el)) {
    el.select();
    return;
  }
  const range = document.createRange();
  range.selectNodeContents(el);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
}

export function undoInField(el: HTMLElement): void {
  el.focus();
  exec("undo");
}

export function redoInField(el: HTMLElement): void {
  el.focus();
  exec("redo");
}
