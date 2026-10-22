/** Orkestr's own context menu (replaces the WebView2 menu in every app window). */
export { ContextMenuHost, ContextMenuProvider, contextMenuHandler, contributeToEvent, installContextMenuHandler, menuForEvent, registerContextMenu, useContextMenu } from "./ContextMenuProvider";
export type { ContributeOptions, MenuSource } from "./ContextMenuProvider";
export { ContextMenuView } from "./ContextMenuView";
export { readClipboardText, writeClipboardText } from "./clipboard";
export type { ActionEntry, CheckboxEntry, LabelEntry, MenuEntry, RadioEntry, SeparatorEntry, SubmenuEntry } from "./model";
export { closeContextMenu, openContextMenu, setContextMenuEnvironment } from "./store";
export type { ContextMenuEnvironment } from "./store";
