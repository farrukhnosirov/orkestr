import { Button, Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { OctagonX, RotateCcw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useT } from "../../i18n";
import { activeFlags } from "./logic";
import { errorMessage, resetEmergency, stopAllAi, useSecurityStore } from "./store";

/**
 * Shortcut of the registered `ai.stopAll` command (coreCommands). Not
 * Ctrl+Shift+Esc: Windows takes that chord for Task Manager before the
 * webview sees it.
 */
export const STOP_SHORTCUT = "Ctrl+Shift+Backspace";

/** Always-visible red Emergency Stop button (title bar). */
export function EmergencyStopButton() {
  const setConfirm = useSecurityStore((s) => s.setConfirmStopOpen);
  const stopped = useSecurityStore((s) => s.flags.allStopped);
  const { t } = useT("security");
  return (
    <button
      type="button"
      className={clsx("sec-estop", stopped && "sec-estop--engaged")}
      aria-label={stopped ? t("emergency.buttonEngagedAria") : t("emergency.buttonAria")}
      title={stopped ? t("emergency.buttonEngagedTitle") : t("emergency.buttonTitle", { shortcut: STOP_SHORTCUT })}
      onClick={() => setConfirm(true)}
    >
      <OctagonX size={15} />
      <span>{stopped ? t("emergency.buttonEngaged") : t("emergency.button")}</span>
    </button>
  );
}

/** Confirmation dialog for Stop All AI (and re-arming afterwards). */
export function EmergencyConfirmDialog() {
  const open = useSecurityStore((s) => s.confirmStopOpen);
  const setOpen = useSecurityStore((s) => s.setConfirmStopOpen);
  const flags = useSecurityStore((s) => s.flags);
  const pushToast = useSecurityStore((s) => s.pushToast);
  const { t } = useT(["security", "common"]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (open) {
      setError(null);
      setBusy(false);
      requestAnimationFrame(() => confirmRef.current?.focus());
    }
  }, [open]);

  if (!open) return null;
  const close = () => !busy && setOpen(false);
  const active = activeFlags(flags);

  const stop = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await stopAllAi();
      pushToast({
        title: t("toast.stoppedTitle"),
        body: t("toast.stoppedBody", {
          agents: t("toast.agentsStopped", { count: r.agentsStopped }),
          approvals: t("toast.approvalsCancelled", { count: r.approvalsCancelled }),
        }),
        tone: "danger",
      });
      setOpen(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const rearm = async () => {
    setBusy(true);
    setError(null);
    try {
      await resetEmergency();
      pushToast({ title: t("toast.rearmedTitle"), body: t("toast.rearmedBody"), tone: "success" });
      setOpen(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div
        className="dialog sec-estop-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="sec-estop-title"
        aria-describedby="sec-estop-desc"
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon sec-estop-dialog__icon">
            <OctagonX size={18} />
          </div>
          <div>
            <h2 id="sec-estop-title" className="dialog__title">
              {flags.allStopped ? t("emergency.titleEngaged") : t("emergency.title")}
            </h2>
            <p id="sec-estop-desc" className="dialog__subtitle">
              {flags.allStopped ? t("emergency.descriptionEngaged") : t("emergency.description")}
            </p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          {active.length > 0 && (
            <ul className="sec-flag-list" aria-label={t("emergency.activeControls")}>
              {active.map((f) => (
                <li key={f}>{f}</li>
              ))}
            </ul>
          )}
          {error && (
            <p className="sec-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          {flags.allStopped ? (
            <Button ref={confirmRef} variant="primary" leftIcon={<RotateCcw size={14} />} loading={busy} onClick={() => void rearm()}>
              {t("emergency.rearm")}
            </Button>
          ) : (
            <Button ref={confirmRef} variant="danger" leftIcon={<OctagonX size={14} />} loading={busy} onClick={() => void stop()}>
              {t("emergency.stopAll")} <Kbd>Enter</Kbd>
            </Button>
          )}
        </footer>
      </div>
    </div>
  );
}
