import { Button, Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { OctagonX, RotateCcw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { activeFlags } from "./logic";
import { errorMessage, resetEmergency, stopAllAi, useSecurityStore } from "./store";

/** Always-visible red Emergency Stop button (title bar). */
export function EmergencyStopButton() {
  const setConfirm = useSecurityStore((s) => s.setConfirmStopOpen);
  const stopped = useSecurityStore((s) => s.flags.allStopped);
  return (
    <button
      type="button"
      className={clsx("sec-estop", stopped && "sec-estop--engaged")}
      aria-label={stopped ? "Emergency stop engaged — manage" : "Emergency stop: stop all AI"}
      title={stopped ? "All AI is stopped (click to re-arm)" : "Stop All AI (Ctrl+Shift+Esc)"}
      onClick={() => setConfirm(true)}
    >
      <OctagonX size={15} />
      <span>{stopped ? "Stopped" : "Stop"}</span>
    </button>
  );
}

/** Confirmation dialog for Stop All AI (and re-arming afterwards). */
export function EmergencyConfirmDialog() {
  const open = useSecurityStore((s) => s.confirmStopOpen);
  const setOpen = useSecurityStore((s) => s.setConfirmStopOpen);
  const flags = useSecurityStore((s) => s.flags);
  const pushToast = useSecurityStore((s) => s.pushToast);
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
        title: "All AI stopped",
        body: `${r.agentsStopped} agent(s) stopped, ${r.approvalsCancelled} pending approval(s) cancelled.`,
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
      pushToast({ title: "Emergency controls re-armed", body: "Agents may start again.", tone: "success" });
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
              {flags.allStopped ? "All AI is stopped" : "Stop all AI?"}
            </h2>
            <p id="sec-estop-desc" className="dialog__subtitle">
              {flags.allStopped
                ? "Agents and the Manager are blocked until you re-arm."
                : "Immediately cancels every agent, the Manager and their pending approvals. Works offline."}
            </p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          {active.length > 0 && (
            <ul className="sec-flag-list" aria-label="Active emergency controls">
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
            Cancel
          </Button>
          {flags.allStopped ? (
            <Button ref={confirmRef} variant="primary" leftIcon={<RotateCcw size={14} />} loading={busy} onClick={() => void rearm()}>
              Re-arm
            </Button>
          ) : (
            <Button ref={confirmRef} variant="danger" leftIcon={<OctagonX size={14} />} loading={busy} onClick={() => void stop()}>
              Stop all AI <Kbd>Enter</Kbd>
            </Button>
          )}
        </footer>
      </div>
    </div>
  );
}
