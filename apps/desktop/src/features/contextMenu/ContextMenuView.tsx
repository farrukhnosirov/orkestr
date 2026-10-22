/**
 * Renders an open context menu: the root list plus any open submenus, each a
 * `role="menu"` portaled to `<body>` (a dock group with `backdrop-filter`
 * would otherwise be the containing block of `position: fixed`).
 *
 * Keyboard (WAI-ARIA menu pattern): ↑/↓ Home/End move, Enter/Space
 * activate, → opens a submenu, ← / Esc close one level, Tab closes all,
 * printable keys jump by type-ahead (any script). Keys are handled by a
 * window-level capture listener, so the menu also works while focus stays in
 * the text field it was opened on (`keepFocus`).
 */
import "./contextMenu.css";

import { clsx } from "clsx";
import { Check, ChevronRight, Dot } from "lucide-react";
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import {
  edgeFocusable,
  type InteractiveEntry,
  isFocusable,
  isInteractive,
  isTypeaheadKey,
  type MenuEntry,
  nextFocusable,
  placeMenu,
  placeSubmenu,
  typeaheadMatch,
} from "./model";

export interface ContextMenuViewProps {
  x: number;
  y: number;
  items: MenuEntry[];
  /** Accessible name. */
  label: string;
  /** Leave keyboard focus where it is (see module docs). */
  keepFocus?: boolean;
  /** Called when the menu closes; `restoreFocus` = return focus to the opener. */
  onClose: (restoreFocus: boolean) => void;
}

const TYPEAHEAD_RESET_MS = 600;
const SUBMENU_HOVER_MS = 120;

function viewport() {
  return { width: window.innerWidth, height: window.innerHeight };
}

export function ContextMenuView({ x, y, items, label, keepFocus = false, onClose }: ContextMenuViewProps) {
  const baseId = useId();
  /** Active index per open level (root = level 0; length = open levels). */
  const [active, setActive] = useState<number[]>(() => [keepFocus ? -1 : edgeFocusable(items, "first")]);
  const [positions, setPositions] = useState<Array<{ left: number; top: number } | null>>([]);
  const levelRefs = useRef<Array<HTMLDivElement | null>>([]);
  const typeahead = useRef<{ text: string; at: number }>({ text: "", at: 0 });
  const hoverTimer = useRef<number | null>(null);
  const closedRef = useRef(false);

  const levels = useMemo(() => {
    const out: MenuEntry[][] = [items];
    for (let k = 0; k < active.length - 1; k++) {
      const e = out[k]?.[active[k] ?? -1];
      if (!e || e.kind !== "submenu") break;
      out.push(e.items);
    }
    return out;
  }, [items, active]);
  const depth = levels.length;

  const itemId = useCallback((level: number, index: number) => `${baseId}-m${level}-i${index}`, [baseId]);

  const close = useCallback(
    (restoreFocus: boolean) => {
      if (closedRef.current) return;
      closedRef.current = true;
      onClose(restoreFocus);
    },
    [onClose],
  );

  const activate = useCallback(
    (level: number, index: number, viaKeyboard: boolean) => {
      const entry = levels[level]?.[index];
      if (!entry || !isInteractive(entry) || entry.disabled) return;
      if (entry.kind === "submenu") {
        setActive((a) => [...a.slice(0, level), index, viaKeyboard ? edgeFocusable(entry.items, "first") : -1]);
        return;
      }
      // Close first: `onClose` returns focus to the opener synchronously, so
      // edit commands act on the focused field.
      close(true);
      const r = entry.kind === "checkbox" ? entry.onSelect(!entry.checked) : entry.onSelect();
      if (r && typeof (r as Promise<void>).catch === "function") (r as Promise<void>).catch((e: unknown) => console.warn("context menu action failed", e));
    },
    [levels, close],
  );

  // ── Positioning (measure, then place; submenus next to their item) ──
  useLayoutEffect(() => {
    const next: Array<{ left: number; top: number } | null> = [];
    for (let k = 0; k < depth; k++) {
      const el = levelRefs.current[k];
      if (!el) {
        next.push(null);
        continue;
      }
      const size = { width: el.offsetWidth, height: el.offsetHeight };
      if (k === 0) {
        next.push(placeMenu({ x, y }, size, viewport()));
      } else {
        const anchor = document.getElementById(itemId(k - 1, active[k - 1] ?? -1));
        const r = anchor?.getBoundingClientRect();
        next.push(r ? placeSubmenu(r, size, viewport()) : null);
      }
    }
    setPositions((prev) => (prev.length === next.length && prev.every((p, i) => p?.left === next[i]?.left && p?.top === next[i]?.top) ? prev : next));
  }, [depth, x, y, active, itemId]);

  // ── Focus: the deepest level's container (unless the opener keeps it) ──
  useEffect(() => {
    if (keepFocus) return;
    levelRefs.current[depth - 1]?.focus({ preventScroll: true });
  }, [depth, keepFocus]);

  // Keep the active item visible in long (scrolling) menus.
  useEffect(() => {
    const k = depth - 1;
    const el = document.getElementById(itemId(k, active[k] ?? -1));
    el?.scrollIntoView?.({ block: "nearest" });
  }, [active, depth, itemId]);

  // ── Keyboard ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      const k = depth - 1;
      const list = levels[k] ?? [];
      const cur = active[k] ?? -1;
      const setAt = (i: number) => setActive((a) => [...a.slice(0, k), i]);
      let handled = true;
      switch (e.key) {
        case "ArrowDown":
          setAt(nextFocusable(list, cur, 1));
          break;
        case "ArrowUp":
          setAt(nextFocusable(list, cur < 0 ? list.length : cur, -1));
          break;
        case "Home":
        case "PageUp":
          setAt(edgeFocusable(list, "first"));
          break;
        case "End":
        case "PageDown":
          setAt(edgeFocusable(list, "last"));
          break;
        case "ArrowRight": {
          const entry = list[cur];
          if (entry && entry.kind === "submenu" && !entry.disabled) activate(k, cur, true);
          break;
        }
        case "ArrowLeft":
          if (k > 0) setActive((a) => a.slice(0, k));
          break;
        case "Escape":
          if (k > 0) setActive((a) => a.slice(0, k));
          else close(true);
          break;
        case "Tab":
          close(true);
          break;
        case "Enter":
        case " ":
          if (cur >= 0) activate(k, cur, true);
          break;
        default:
          if (e.key === "Shift" || e.key === "Control" || e.key === "Alt" || e.key === "Meta") {
            handled = false;
          } else if (isTypeaheadKey(e.key, e.ctrlKey, e.altKey, e.metaKey)) {
            const now = Date.now();
            const t = typeahead.current;
            t.text = now - t.at > TYPEAHEAD_RESET_MS ? e.key : t.text + e.key;
            t.at = now;
            const i = typeaheadMatch(list, cur, t.text);
            if (i >= 0) setAt(i);
          } else {
            // Any other key (shortcuts, F-keys) dismisses the menu and goes through.
            handled = false;
            close(false);
          }
      }
      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [levels, active, depth, activate, close]);

  // ── Dismissal: pointer outside, window blur / resize, scrolling outside ──
  useEffect(() => {
    const inside = (t: EventTarget | null) => t instanceof Node && levelRefs.current.some((el) => el?.contains(t));
    const onDown = (e: PointerEvent | MouseEvent) => {
      if (!inside(e.target)) close(false);
    };
    const onScroll = (e: Event) => {
      if (!inside(e.target)) close(false);
    };
    const onBlur = () => close(false);
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("wheel", onScroll, { capture: true, passive: true });
    window.addEventListener("resize", onBlur);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("wheel", onScroll, true);
      window.removeEventListener("resize", onBlur);
      window.removeEventListener("blur", onBlur);
    };
  }, [close]);

  useEffect(
    () => () => {
      if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
    },
    [],
  );

  const onHover = (level: number, index: number) => {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
    const entry = levels[level]?.[index];
    setActive((a) => (a[level] === index && a.length > level + 1 ? a : [...a.slice(0, level), entry && isFocusable(entry) ? index : -1]));
    if (entry && entry.kind === "submenu" && !entry.disabled) {
      hoverTimer.current = window.setTimeout(() => {
        setActive((a) => (a[level] === index ? [...a.slice(0, level + 1), -1] : a));
      }, SUBMENU_HOVER_MS);
    }
  };

  return createPortal(
    <>
      {levels.map((list, level) => {
        const pos = positions[level];
        const activeIndex = active[level] ?? -1;
        return (
          <div
            key={level}
            ref={(el) => {
              levelRefs.current[level] = el;
            }}
            className="ok-ctxmenu"
            role="menu"
            aria-label={level === 0 ? label : (levels[level - 1]?.[active[level - 1] ?? -1] as InteractiveEntry | undefined)?.label}
            aria-orientation="vertical"
            aria-activedescendant={activeIndex >= 0 ? itemId(level, activeIndex) : undefined}
            tabIndex={-1}
            data-level={level}
            style={pos ? { left: pos.left, top: pos.top } : { left: x, top: y, visibility: "hidden" }}
            // Keep focus (and the text selection) where it is on clicks.
            onMouseDown={(e) => e.preventDefault()}
            onContextMenu={(e) => e.preventDefault()}
          >
            {list.map((entry, index) => {
              if (entry.kind === "separator") return <div key={entry.id ?? `sep-${index}`} className="ok-ctxmenu__sep" role="separator" />;
              if (entry.kind === "label")
                return (
                  <div key={entry.id} className="ok-ctxmenu__group" role="presentation">
                    {entry.label}
                  </div>
                );
              const isActive = activeIndex === index;
              const expanded = entry.kind === "submenu" ? level + 1 < depth && isActive : undefined;
              const role = entry.kind === "checkbox" ? "menuitemcheckbox" : entry.kind === "radio" ? "menuitemradio" : "menuitem";
              return (
                <div
                  key={entry.id}
                  id={itemId(level, index)}
                  role={role}
                  className={clsx(
                    "ok-ctxmenu__item",
                    isActive && "ok-ctxmenu__item--active",
                    entry.kind !== "submenu" && entry.kind !== "checkbox" && entry.kind !== "radio" && entry.danger && "ok-ctxmenu__item--danger",
                  )}
                  aria-disabled={entry.disabled || undefined}
                  aria-haspopup={entry.kind === "submenu" ? "menu" : undefined}
                  aria-expanded={expanded}
                  aria-checked={entry.kind === "checkbox" || entry.kind === "radio" ? entry.checked : undefined}
                  data-id={entry.id}
                  onMouseEnter={() => onHover(level, index)}
                  onClick={() => activate(level, index, false)}
                >
                  <span className="ok-ctxmenu__icon" aria-hidden="true">
                    {entry.kind === "checkbox" ? entry.checked ? <Check size={14} /> : null : entry.kind === "radio" ? entry.checked ? <Dot size={18} /> : null : entry.icon}
                  </span>
                  <span className="ok-ctxmenu__label">{entry.label}</span>
                  {entry.kind === "submenu" ? (
                    <ChevronRight className="ok-ctxmenu__chevron" size={14} aria-hidden="true" />
                  ) : entry.kind !== "radio" && entry.shortcut ? (
                    <span className="ok-ctxmenu__shortcut" aria-hidden="true">
                      {entry.shortcut}
                    </span>
                  ) : null}
                </div>
              );
            })}
          </div>
        );
      })}
    </>,
    document.body,
  );
}
