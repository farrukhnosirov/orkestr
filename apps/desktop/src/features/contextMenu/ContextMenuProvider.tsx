/**
 * App-wide context menu (one per window): replaces the browser / WebView2
 * menu everywhere in Orkestr's own windows.
 *
 * How a right-click becomes a menu:
 * 1. Components contribute items for the element under the pointer with
 *    `useContextMenu(items)` (spread the returned `onContextMenu` on the
 *    element) or `registerContextMenu(element, items)` for non-React DOM.
 *    Nested contributors all add their section, innermost first.
 * 2. A window-level (bubble phase) listener — the last to see the event —
 *    adds the default items for text fields, selections, links and images,
 *    cancels the native menu and opens the Orkestr menu.
 * 3. A component that renders its own menu (Monaco) simply calls
 *    `preventDefault()`: the listener then leaves the event alone.
 *
 * Iframes (plugin widgets) get their own events; they never reach this
 * listener, so their menus are untouched.
 */
import { type ReactNode, type MouseEvent as ReactMouseEvent, useCallback, useEffect, useRef } from "react";

import { useT } from "../../i18n";
import { ContextMenuView } from "./ContextMenuView";
import { defaultSections } from "./defaults";
import { mergeSections, type MenuEntry } from "./model";
import { getContextMenuEnvironment, openContextMenu, useContextMenuStore } from "./store";

/** Items, or a function computing them when the menu opens. */
export type MenuSource = readonly MenuEntry[] | (() => readonly MenuEntry[]);

export interface ContributeOptions {
  /** Drop the default items (selection / link / image) for this target. */
  replaceDefaults?: boolean;
}

interface Contribution {
  sections: MenuEntry[][];
  replaceDefaults: boolean;
}

const contributions = new WeakMap<Event, Contribution>();
const registered = new WeakMap<Element, { source: MenuSource; options: ContributeOptions }>();

function resolve(source: MenuSource): MenuEntry[] {
  return [...(typeof source === "function" ? source() : source)];
}

/** Adds a section of items to a `contextmenu` event (innermost contributor first). */
export function contributeToEvent(event: Event, source: MenuSource, options: ContributeOptions = {}): void {
  const c = contributions.get(event) ?? { sections: [], replaceDefaults: false };
  c.sections.push(resolve(source));
  if (options.replaceDefaults) c.replaceDefaults = true;
  contributions.set(event, c);
}

/**
 * Hook: items for an element. Returns props to spread on it:
 * `<div {...useContextMenu(() => [...])}>`.
 */
export function useContextMenu(source: MenuSource, options: ContributeOptions = {}): { onContextMenu: (e: ReactMouseEvent) => void } {
  const ref = useRef({ source, options });
  ref.current = { source, options };
  const onContextMenu = useCallback((e: ReactMouseEvent) => {
    if (e.nativeEvent.defaultPrevented) return;
    contributeToEvent(e.nativeEvent, ref.current.source, ref.current.options);
  }, []);
  return { onContextMenu };
}

/**
 * Non-hook variant for elements rendered in loops:
 * `<li onContextMenu={contextMenuHandler(() => rowItems(row))}>`.
 */
export function contextMenuHandler(source: MenuSource, options: ContributeOptions = {}): (e: ReactMouseEvent) => void {
  return (e) => {
    if (e.nativeEvent.defaultPrevented) return;
    contributeToEvent(e.nativeEvent, source, options);
  };
}

/** Declarative registration for DOM that React does not render. Returns a disposer. */
export function registerContextMenu(element: Element, source: MenuSource, options: ContributeOptions = {}): () => void {
  registered.set(element, { source, options });
  return () => {
    if (registered.get(element)?.source === source) registered.delete(element);
  };
}

function registeredSections(target: Element | null, event: Event): void {
  for (let el: Element | null = target; el; el = el.parentElement) {
    const r = registered.get(el);
    if (r) contributeToEvent(event, r.source, r.options);
  }
}

/** Point to open a keyboard-invoked menu at (Shift+F10 / the Menu key report 0,0 or the element's corner). */
function menuPoint(e: MouseEvent, target: Element | null): { x: number; y: number } {
  if (e.clientX !== 0 || e.clientY !== 0 || !target) return { x: e.clientX, y: e.clientY };
  const r = target.getBoundingClientRect();
  return { x: r.left + Math.min(24, r.width / 2), y: r.top + Math.min(r.height, 28) };
}

/** Builds the menu for a `contextmenu` event (exported for tests). */
export function menuForEvent(e: MouseEvent): { items: MenuEntry[]; field: HTMLElement | null } {
  const target = e.target instanceof Element ? e.target : e.target instanceof Node ? e.target.parentElement : null;
  registeredSections(target, e);
  const c = contributions.get(e);
  const defaults = c?.replaceDefaults ? { leading: [], trailing: [], field: null } : defaultSections(target, getContextMenuEnvironment());
  const items = mergeSections([...defaults.leading, ...(c?.sections ?? []), ...defaults.trailing]);
  return { items, field: defaults.field };
}

/** Installs the window-level handler. Returns a disposer. */
export function installContextMenuHandler(label: () => string): () => void {
  const onContextMenu = (e: MouseEvent) => {
    if (e.defaultPrevented) return; // a component shows its own menu
    // Never the browser menu in Orkestr's windows (also without the WebView2 setting).
    e.preventDefault();
    const target = e.target instanceof Element ? e.target : null;
    const { items, field } = menuForEvent(e);
    if (items.length === 0) return;
    const { x, y } = menuPoint(e, target);
    const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    openContextMenu({ x, y, items, label: label(), keepFocus: field !== null, returnFocus: field ?? active });
  };
  window.addEventListener("contextmenu", onContextMenu);
  return () => window.removeEventListener("contextmenu", onContextMenu);
}

/** Renders the open menu of this window. */
export function ContextMenuHost() {
  const menu = useContextMenuStore((s) => s.menu);
  const close = useContextMenuStore((s) => s.close);
  const { t } = useT("contextMenu");
  const onClose = useCallback(
    (restoreFocus: boolean) => {
      const target = useContextMenuStore.getState().menu?.returnFocus ?? null;
      close();
      if (restoreFocus && target?.isConnected) target.focus({ preventScroll: true });
    },
    [close],
  );
  if (!menu) return null;
  return <ContextMenuView key={menu.nonce} x={menu.x} y={menu.y} items={menu.items} label={menu.label ?? t("menu.label")} keepFocus={menu.keepFocus ?? false} onClose={onClose} />;
}

/** Wrap each window's root: installs the handler and renders the menu. */
export function ContextMenuProvider({ children }: { children: ReactNode }) {
  const { t } = useT("contextMenu");
  const tRef = useRef(t);
  tRef.current = t;
  useEffect(() => installContextMenuHandler(() => tRef.current("menu.label")), []);
  return (
    <>
      {children}
      <ContextMenuHost />
    </>
  );
}
