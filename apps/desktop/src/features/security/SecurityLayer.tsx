import "./security.css";

import { Tabs } from "@orkestr/ui";
import { KeyRound, ShieldCheck, X } from "lucide-react";
import { useEffect } from "react";

import { useAppStore } from "../../store/app";
import { ApprovalsDrawer } from "./ApprovalsInbox";
import { EmergencyConfirmDialog } from "./EmergencyStop";
import { PermissionsSettings } from "./PermissionsSettings";
import { SecretsSettings } from "./SecretsSettings";
import { type SecurityTab, initSecurityLive, useSecurityStore } from "./store";
import { SecurityToasts } from "./Toasts";

const TABS = [
  { id: "permissions" as const, label: "Security & Permissions", icon: <ShieldCheck size={14} /> },
  { id: "secrets" as const, label: "Secrets Vault", icon: <KeyRound size={14} /> },
];

/**
 * Security Center dialog. Renders the same section components that are
 * registered in the settings registry, so the feature is usable before the
 * shared Settings view exists.
 */
function SecurityCenter() {
  const open = useSecurityStore((s) => s.centerOpen);
  const tab = useSecurityStore((s) => s.centerTab);
  const openCenter = useSecurityStore((s) => s.openCenter);
  const close = useSecurityStore((s) => s.closeCenter);
  const projectId = useAppStore((s) => s.activeProjectId);
  if (!open) return null;
  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div
        className="dialog sec-center-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sec-center-title"
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <ShieldCheck size={18} />
          </div>
          <div>
            <h2 id="sec-center-title" className="dialog__title">
              Security
            </h2>
            <p className="dialog__subtitle">Approval modes, permission rules, emergency controls and secrets.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="sec-center-dialog__tabs">
          <Tabs<SecurityTab> items={TABS} value={tab} onChange={(t) => openCenter(t)} aria-label="Security sections" />
        </div>
        <div className="sec-center-dialog__body">
          {tab === "permissions" ? <PermissionsSettings projectId={projectId} /> : <SecretsSettings projectId={projectId} />}
        </div>
      </div>
    </div>
  );
}

/** Global shortcuts of the feature: Ctrl+Shift+Esc → Stop All AI confirm. */
function useSecurityShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && (e.key === "Escape" || e.key === "Esc")) {
        e.preventDefault();
        useSecurityStore.getState().setConfirmStopOpen(true);
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, []);
}

/** Mounted once by the app: live approvals/emergency state + overlays. */
export function SecurityLayer({ connected }: { connected: boolean }) {
  useSecurityShortcuts();
  useEffect(() => {
    if (!connected) return;
    return initSecurityLive();
  }, [connected]);
  return (
    <>
      <ApprovalsDrawer />
      <EmergencyConfirmDialog />
      <SecurityCenter />
      <SecurityToasts />
    </>
  );
}
