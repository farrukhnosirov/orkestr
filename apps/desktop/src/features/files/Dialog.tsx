import "./shared.css";

/**
 * Small modal dialog with action buttons (delete confirmation, unsaved
 * changes, save conflicts). Escape cancels; focus moves to the preferred
 * action on open and is trapped inside while open.
 */
import { Button } from "@orkestr/ui";
import { X } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef } from "react";

import { t } from "../../i18n";

export interface DialogAction {
  id: string;
  label: string;
  variant?: "primary" | "secondary" | "ghost" | "danger";
  icon?: ReactNode;
  busy?: boolean;
  disabled?: boolean;
  /** Receives initial focus. */
  autoFocus?: boolean;
  onSelect: () => void;
}

export interface ActionDialogProps {
  title: string;
  subtitle?: ReactNode;
  icon?: ReactNode;
  tone?: "accent" | "danger" | "warning";
  children?: ReactNode;
  actions: DialogAction[];
  onCancel: () => void;
  error?: string | null;
}

export function ActionDialog({ title, subtitle, icon, tone = "accent", children, actions, onCancel, error }: ActionDialogProps) {
  const titleId = useId();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const preferred = root.querySelector<HTMLButtonElement>("[data-autofocus='true']");
    (preferred ?? root.querySelector<HTMLButtonElement>("button"))?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onCancel();
      return;
    }
    if (e.key !== "Tab") return;
    const focusables = [...(ref.current?.querySelectorAll<HTMLElement>("button:not(:disabled), input, [tabindex='0']") ?? [])];
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div
        ref={ref}
        className={`dialog fx-dialog fx-dialog--${tone}`}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
      >
        <header className="dialog__header">
          {icon && <div className="dialog__icon">{icon}</div>}
          <div>
            <h2 id={titleId} className="dialog__title">
              {title}
            </h2>
            {subtitle && <p className="dialog__subtitle">{subtitle}</p>}
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={onCancel}>
            <X size={16} />
          </button>
        </header>
        {(children || error) && (
          <div className="dialog__body">
            {children}
            {error && (
              <div className="fx-dialog__error" role="alert">
                {error}
              </div>
            )}
          </div>
        )}
        <footer className="dialog__footer">
          {actions.map((a) => (
            <Button
              key={a.id}
              variant={a.variant ?? "secondary"}
              leftIcon={a.icon}
              loading={a.busy}
              disabled={a.disabled}
              data-autofocus={a.autoFocus ? "true" : undefined}
              onClick={a.onSelect}
            >
              {a.label}
            </Button>
          ))}
        </footer>
      </div>
    </div>
  );
}
