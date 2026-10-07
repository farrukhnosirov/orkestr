import clsx from "clsx";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";

interface OverlayProps {
  /** Extra classes next to `overlay` (e.g. `overlay--top`). */
  className?: string;
  /** Called on a mouse-down on the backdrop itself (not on the dialog). */
  onDismiss?: () => void;
  children: ReactNode;
}

/**
 * Full-window modal backdrop. Always rendered into `document.body` through a
 * portal: a dock group (or any panel ancestor) with `backdrop-filter`,
 * `transform` or `filter` becomes the containing block of `position: fixed`
 * descendants, so an inline `.overlay` would be sized to and clipped by its
 * group instead of covering the window. React events still bubble through
 * the React tree, so panel-level handlers keep working.
 */
export function Overlay({ className, onDismiss, children }: OverlayProps) {
  const node = (
    <div
      className={clsx("overlay", className)}
      role="presentation"
      onMouseDown={(e) => {
        if (onDismiss && e.target === e.currentTarget) onDismiss();
      }}
    >
      {children}
    </div>
  );
  return typeof document === "undefined" ? node : createPortal(node, document.body);
}
