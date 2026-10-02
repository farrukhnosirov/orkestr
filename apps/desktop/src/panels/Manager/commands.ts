/** Commands contributed by the Manager panel (command registry §54). */
import { ClipboardCheck, Sparkles } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { errorMessage, managerActions } from "./data";
import { useManagerUi } from "./store";

/** The plan "Approve Plan" acts on (newest proposed plan of the active project). */
export function approveTarget(projectId: string | null): string | null {
  const p = useManagerUi.getState().proposedPlan;
  return p && projectId && p.projectId === projectId ? p.planId : null;
}

export const managerCommands: AppCommand[] = [
  {
    id: "manager.ask",
    title: "Ask Manager",
    group: "AI Manager",
    icon: Sparkles,
    keywords: ["manager", "chat", "ai", "plan", "build", "менеджер", "спросить"],
    shortcut: "Ctrl+Shift+M",
    run: () => {
      workspaceActions.openPanel("manager");
      useManagerUi.getState().requestFocus();
    },
  },
  {
    id: "manager.approvePlan",
    title: "Approve Plan",
    group: "AI Manager",
    icon: ClipboardCheck,
    keywords: ["plan", "approve", "run", "start", "одобрить", "план"],
    enabled: (ctx) => approveTarget(ctx.projectId) !== null,
    run: async (ctx) => {
      const planId = approveTarget(ctx.projectId);
      if (!planId) return;
      workspaceActions.openPanel("manager");
      const ui = useManagerUi.getState();
      try {
        const r = await managerActions.approvePlan(planId);
        ui.toast("success", r.run ? "Plan approved — the run started." : "Plan approved — tasks created.");
      } catch (e) {
        ui.toast("danger", errorMessage(e));
      }
    },
  },
];
