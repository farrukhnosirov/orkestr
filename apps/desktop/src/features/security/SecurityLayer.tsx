import "./security.css";

import { useEffect } from "react";

import { ApprovalsDrawer } from "./ApprovalsInbox";
import { EmergencyConfirmDialog } from "./EmergencyStop";
import { initSecurityLive, useSecurityStore } from "./store";
import { SecurityToasts } from "./Toasts";

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
      <SecurityToasts />
    </>
  );
}
