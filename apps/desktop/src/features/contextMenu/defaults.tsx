/**
 * Default context menu items by what is under the pointer: text fields
 * (edit commands), selected text (Copy, Search in Orkestr, Ask Manager),
 * links (Open, Copy link) and images (Copy image, Save image as…).
 *
 * Spelling suggestions are not offered: WebView2 exposes them only to its
 * own (disabled) menu.
 */
import { ClipboardPaste, ClipboardType, Copy, ExternalLink, Image as ImageIcon, Link, Redo2, Scissors, Search, Sparkles, TextSelect, Undo2 } from "lucide-react";

import { t } from "../../i18n";
import { saveImage, writeClipboardImage, writeClipboardText } from "./clipboard";
import {
  commandEnabled,
  copyField,
  cutField,
  editableTarget,
  fieldSelection,
  isTextControl,
  isWritable,
  pasteIntoField,
  redoInField,
  selectAllInField,
  undoInField,
} from "./editing";
import type { MenuEntry } from "./model";
import type { ContextMenuEnvironment } from "./store";

const ICON = 14;

/** Shortens a selection for a menu label ("Search for “…”"). */
export function snippet(text: string, max = 28): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}

/** The selected text of the document (outside text fields). */
export function documentSelection(): string {
  const sel = typeof window !== "undefined" ? window.getSelection() : null;
  return sel && !sel.isCollapsed ? sel.toString() : "";
}

/** Edit commands for a text field. */
export function editItems(field: HTMLElement): MenuEntry[] {
  const writable = isWritable(field);
  const hasSelection = fieldSelection(field) !== "";
  const plainControl = isTextControl(field);
  return [
    { id: "edit.undo", label: t("contextMenu:edit.undo"), icon: <Undo2 size={ICON} />, shortcut: "Ctrl+Z", disabled: !writable || !commandEnabled("undo"), onSelect: () => undoInField(field) },
    { id: "edit.redo", label: t("contextMenu:edit.redo"), icon: <Redo2 size={ICON} />, shortcut: "Ctrl+Y", disabled: !writable || !commandEnabled("redo"), onSelect: () => redoInField(field) },
    { kind: "separator", id: "edit.s1" },
    { id: "edit.cut", label: t("contextMenu:edit.cut"), icon: <Scissors size={ICON} />, shortcut: "Ctrl+X", disabled: !writable || !hasSelection, onSelect: () => cutField(field) },
    { id: "edit.copy", label: t("contextMenu:edit.copy"), icon: <Copy size={ICON} />, shortcut: "Ctrl+C", disabled: !hasSelection, onSelect: () => copyField(field) },
    { id: "edit.paste", label: t("contextMenu:edit.paste"), icon: <ClipboardPaste size={ICON} />, shortcut: "Ctrl+V", disabled: !writable, onSelect: () => pasteIntoField(field) },
    {
      id: "edit.pastePlain",
      label: t("contextMenu:edit.pastePlain"),
      icon: <ClipboardType size={ICON} />,
      shortcut: "Ctrl+Shift+V",
      // Inputs and textareas only hold plain text: the item would duplicate Paste.
      hidden: plainControl,
      disabled: !writable,
      onSelect: () => pasteIntoField(field),
    },
    { kind: "separator", id: "edit.s2" },
    { id: "edit.selectAll", label: t("contextMenu:edit.selectAll"), icon: <TextSelect size={ICON} />, shortcut: "Ctrl+A", onSelect: () => selectAllInField(field) },
  ];
}

/** Items for selected text. `withCopy = false` inside text fields (Copy is already in the edit group). */
export function selectionItems(text: string, env: ContextMenuEnvironment, withCopy: boolean): MenuEntry[] {
  if (!text.trim()) return [];
  const { searchInOrkestr, askManager } = env;
  return [
    { id: "selection.copy", label: t("contextMenu:selection.copy"), icon: <Copy size={ICON} />, shortcut: "Ctrl+C", hidden: !withCopy, onSelect: () => void writeClipboardText(text) },
    {
      id: "selection.search",
      label: t("contextMenu:selection.search", { text: snippet(text) }),
      icon: <Search size={ICON} />,
      hidden: !searchInOrkestr,
      onSelect: () => searchInOrkestr?.(text.trim()),
    },
    {
      id: "selection.askManager",
      label: t("contextMenu:selection.askManager"),
      icon: <Sparkles size={ICON} />,
      hidden: !askManager,
      onSelect: () => askManager?.(text),
    },
  ];
}

export function linkItems(anchor: HTMLAnchorElement): MenuEntry[] {
  const href = anchor.href;
  if (!href || href.startsWith("javascript:")) return [];
  return [
    { id: "link.open", label: t("contextMenu:link.open"), icon: <ExternalLink size={ICON} />, onSelect: () => anchor.click() },
    { id: "link.copy", label: t("contextMenu:link.copy"), icon: <Link size={ICON} />, onSelect: () => void writeClipboardText(href) },
  ];
}

export function imageItems(img: HTMLImageElement): MenuEntry[] {
  const src = img.currentSrc || img.src;
  if (!src) return [];
  return [
    { id: "image.copy", label: t("contextMenu:image.copy"), icon: <ImageIcon size={ICON} />, onSelect: () => void writeClipboardImage(img) },
    { id: "image.save", label: t("contextMenu:image.save"), icon: <ImageIcon size={ICON} />, onSelect: () => saveImage(src) },
  ];
}

export interface DefaultSections {
  /** Shown before items contributed by components (text field edit commands). */
  leading: MenuEntry[][];
  /** Shown after contributed items (selection, link, image). */
  trailing: MenuEntry[][];
  /** The text field the menu was opened on (focus stays there). */
  field: HTMLElement | null;
}

/** Default sections for a `contextmenu` target. */
export function defaultSections(target: Element | null, env: ContextMenuEnvironment): DefaultSections {
  const field = editableTarget(target);
  if (field) {
    const sel = fieldSelection(field);
    return { leading: [editItems(field)], trailing: [selectionItems(sel, env, false)], field };
  }
  const trailing: MenuEntry[][] = [selectionItems(documentSelection(), env, true)];
  const anchor = target?.closest<HTMLAnchorElement>("a[href]") ?? null;
  if (anchor) trailing.push(linkItems(anchor));
  if (target instanceof HTMLImageElement) trailing.push(imageItems(target));
  return { leading: [], trailing, field: null };
}
