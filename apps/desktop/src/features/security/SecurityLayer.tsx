import "./security.css";

import { useEffect } from "react";

import { ApprovalsDrawer } from "./ApprovalsInbox";
import { EmergencyConfirmDialog } from "./EmergencyStop";
import { initSecurityLive } from "./store";
import { SecurityToasts } from "./Toasts";

/**
 * Mounted once by the app: live approvals/emergency state + overlays.
 * The Stop All AI shortcut (`STOP_SHORTCUT`, Ctrl+Shift+Backspace) is the
 * registered `ai.stopAll` command, dispatched by the global shortcut handler.
 * Ctrl+Shift+Esc is not used: Windows reserves it for Task Manager.
 */
export function SecurityLayer({ connected }: { connected: boolean }) {
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
