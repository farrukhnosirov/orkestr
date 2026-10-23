import clsx from "clsx";
import { type ReactNode, useEffect, useRef } from "react";
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
  const ref = useRef<HTMLDivElement>(null);
  // Modal focus: when the dialog did not focus a control itself (autoFocus),
  // move focus into it — otherwise Escape / Tab still go to the page behind
  // (e.g. Escape did not close «Запустить агента» opened from a button).
  // Focus returns to the opener when the overlay closes.
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!root.contains(document.activeElement)) {
      const target = root.querySelector<HTMLElement>("[role=dialog], [role=alertdialog], dialog, form, section") ?? root;
      if (!target.hasAttribute("tabindex")) target.setAttribute("tabindex", "-1");
      target.focus({ preventScroll: true });
    }
    return () => {
      const active = document.activeElement;
      if (opener?.isConnected && (!active || active === document.body || root.contains(active))) opener.focus({ preventScroll: true });
    };
  }, []);
  const node = (
    <div
      ref={ref}
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
