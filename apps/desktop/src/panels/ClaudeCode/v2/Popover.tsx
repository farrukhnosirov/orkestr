/**
 * Small anchored popover of the Claude Code panel (usage details, MCP,
 * context, add folder): opens under its trigger, closes on Esc and on a
 * click outside.
 */
import clsx from "clsx";
import { type ReactNode, useEffect, useRef } from "react";

export interface PopoverProps {
  open: boolean;
  onClose: () => void;
  /** Accessible name of the dialog. */
  label: string;
  align?: "start" | "end";
  className?: string;
  children: ReactNode;
}

export function Popover({ open, onClose, label, align = "start", className, children }: PopoverProps) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    const onDown = (e: MouseEvent) => {
      const el = ref.current;
      const anchor = el?.parentElement;
      if (el && anchor && !anchor.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div ref={ref} role="dialog" aria-label={label} className={clsx("cc2-pop", align === "end" && "cc2-pop--end", className)}>
      {children}
    </div>
  );
}
