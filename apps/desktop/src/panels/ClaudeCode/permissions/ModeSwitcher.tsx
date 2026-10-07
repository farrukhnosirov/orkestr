/**
 * Permission mode switch: По умолчанию / Принимать правки / План / Без
 * подтверждений. The last one (`bypassPermissions`) opens a danger dialog
 * and is applied only after an explicit confirmation.
 */
import type { ClaudePermissionMode } from "@orkestr/protocol";
import clsx from "clsx";
import { ClipboardList, FilePen, ShieldAlert, ShieldCheck } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useRef, useState } from "react";

import { ActionDialog } from "../../../features/files/Dialog";
import { useT } from "../../../i18n";
import type { ModeSwitcherProps } from "../types";

const MODES: readonly ClaudePermissionMode[] = ["default", "acceptEdits", "plan", "bypassPermissions"];

const ICONS: Record<ClaudePermissionMode, ReactNode> = {
  default: <ShieldCheck size={13} aria-hidden />,
  acceptEdits: <FilePen size={13} aria-hidden />,
  plan: <ClipboardList size={13} aria-hidden />,
  bypassPermissions: <ShieldAlert size={13} aria-hidden />,
};

export function ModeSwitcher({ mode, disabled, onChange }: ModeSwitcherProps) {
  const { t } = useT("claudeCodeComposer");
  const [confirming, setConfirming] = useState(false);
  const group = useRef<HTMLDivElement>(null);

  const pick = (next: ClaudePermissionMode) => {
    if (disabled || next === mode) return;
    if (next === "bypassPermissions") setConfirming(true);
    else onChange(next);
  };

  // Arrow keys move between the options (radio group pattern).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const buttons = [...(group.current?.querySelectorAll<HTMLButtonElement>("button[role='radio']") ?? [])];
    const idx = buttons.findIndex((b) => b === document.activeElement);
    if (idx < 0) return;
    e.preventDefault();
    const next = buttons[(idx + (e.key === "ArrowRight" ? 1 : buttons.length - 1)) % buttons.length];
    next?.focus();
  };

  return (
    <>
      <div ref={group} className="ccx-mode" role="radiogroup" aria-label={t("mode.label")} onKeyDown={onKeyDown}>
        {MODES.map((m) => (
          <button
            key={m}
            type="button"
            role="radio"
            aria-checked={m === mode}
            tabIndex={m === mode ? 0 : -1}
            disabled={disabled}
            title={t(`mode.${m}Hint`)}
            className={clsx("ccx-mode__opt", m === mode && "ccx-mode__opt--active", m === "bypassPermissions" && "ccx-mode__opt--danger")}
            onClick={() => pick(m)}
          >
            {ICONS[m]}
            <span>{t(`mode.${m}`)}</span>
          </button>
        ))}
      </div>
      {confirming && (
        <ActionDialog
          title={t("mode.confirmTitle")}
          tone="danger"
          icon={<ShieldAlert size={18} />}
          onCancel={() => setConfirming(false)}
          actions={[
            { id: "cancel", label: t("mode.cancel"), variant: "ghost", autoFocus: true, onSelect: () => setConfirming(false) },
            {
              id: "confirm",
              label: t("mode.confirm"),
              variant: "danger",
              onSelect: () => {
                setConfirming(false);
                onChange("bypassPermissions", true);
              },
            },
          ]}
        >
          <p>{t("mode.confirmBody")}</p>
        </ActionDialog>
      )}
    </>
  );
}
