/** Commands contributed by the Manager panel (command registry §54). */
import { ClipboardCheck, Scale, Sparkles } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { useConsensusUi } from "../../features/reviews/store";
import { t } from "../../i18n";
import { errorMessage, managerActions } from "./data";
import { useManagerUi } from "./store";

/** The plan "Approve Plan" acts on (newest proposed plan of the active project). */
export function approveTarget(projectId: string | null): string | null {
  const p = useManagerUi.getState().proposedPlan;
  return p && projectId && p.projectId === projectId ? p.planId : null;
}

export const managerCommands: AppCommand[] = [
  {
    id: "manager.consensus",
    get title() {
      return t("consensus:command");
    },
    get group() {
      return t("manager:commands.group");
    },
    icon: Scale,
    keywords: ["consensus", "models", "decision", "compare", "консенсус", "модели", "решение", "сравнить"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      workspaceActions.openPanel("manager");
      useConsensusUi.getState().setOpen(true);
    },
  },
  {
    id: "manager.ask",
    get title() {
      return t("manager:commands.ask");
    },
    get group() {
      return t("manager:commands.group");
    },
    icon: Sparkles,
    keywords: ["manager", "chat", "ai", "plan", "build", "ask", "менеджер", "спросить", "чат"],
    shortcut: "Ctrl+Shift+M",
    run: () => {
      workspaceActions.openPanel("manager");
      useManagerUi.getState().requestFocus();
    },
  },
  {
    id: "manager.approvePlan",
    get title() {
      return t("manager:commands.approvePlan");
    },
    get group() {
      return t("manager:commands.group");
    },
    icon: ClipboardCheck,
    keywords: ["plan", "approve", "run", "start", "одобрить", "подтвердить", "план"],
    enabled: (ctx) => approveTarget(ctx.projectId) !== null,
    run: async (ctx) => {
      const planId = approveTarget(ctx.projectId);
      if (!planId) return;
      workspaceActions.openPanel("manager");
      const ui = useManagerUi.getState();
      try {
        const r = await managerActions.approvePlan(planId);
        ui.toast("success", r.run ? t("manager:plan.approvedRun") : t("manager:plan.approvedTasks"));
      } catch (e) {
        ui.toast("danger", errorMessage(e));
      }
    },
  },
];
