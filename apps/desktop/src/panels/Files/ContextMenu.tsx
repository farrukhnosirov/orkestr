/**
 * File tree menu: the Files panel's item model rendered by Orkestr's shared
 * context menu (`features/contextMenu`: keyboard navigation, type-ahead,
 * viewport-aware placement, theme tokens).
 */
import { type ReactNode, useRef } from "react";

import { ContextMenuView, type MenuEntry } from "../../features/contextMenu";

export type MenuItem =
  | { type: "item"; id: string; label: string; icon?: ReactNode; shortcut?: string; danger?: boolean; disabled?: boolean; onSelect: () => void }
  | { type: "separator"; id: string };

export function toMenuEntries(items: readonly MenuItem[]): MenuEntry[] {
  return items.map((i) =>
    i.type === "separator"
      ? { kind: "separator", id: i.id }
      : {
          id: i.id,
          label: i.label,
          icon: i.icon,
          shortcut: i.shortcut,
          danger: i.danger,
          disabled: i.disabled,
          onSelect: i.onSelect,
        },
  );
}

export function ContextMenu({ x, y, items, onClose, label }: { x: number; y: number; items: MenuItem[]; onClose: () => void; label: string }) {
  // The tree keeps keyboard focus after the menu closes (Esc / an action).
  const opener = useRef(document.activeElement instanceof HTMLElement ? document.activeElement : null);
  return (
    <ContextMenuView
      x={x}
      y={y}
      items={toMenuEntries(items)}
      label={label}
      onClose={(restoreFocus) => {
        onClose();
        if (restoreFocus && opener.current?.isConnected) opener.current.focus({ preventScroll: true });
      }}
    />
  );
}
