/** Commands contributed by the Agents panel (command registry §54). */
import { Bot, OctagonX } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { agentActions, errorMessage } from "./data";
import { useAgentsUi } from "./store";

/** The agent "Stop Agent" acts on: the selected one if active, else the only active one. */
export function stopTarget(): string | null {
  const { selectedAgentId, activeAgentIds } = useAgentsUi.getState();
  if (selectedAgentId && activeAgentIds.includes(selectedAgentId)) return selectedAgentId;
  return activeAgentIds.length === 1 ? (activeAgentIds[0] ?? null) : null;
}

export const agentCommands: AppCommand[] = [
  {
    id: "agents.spawn",
    title: "Spawn Agent",
    group: "Agents",
    icon: Bot,
    keywords: ["agent", "new", "start", "run", "агент"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      workspaceActions.openPanel("agents");
      useAgentsUi.getState().openSpawn();
    },
  },
  {
    id: "agents.stop",
    title: "Stop Agent",
    group: "Agents",
    icon: OctagonX,
    keywords: ["agent", "cancel", "halt", "остановить"],
    enabled: () => stopTarget() !== null,
    run: async () => {
      const id = stopTarget();
      if (!id) return;
      const ui = useAgentsUi.getState();
      try {
        const a = await agentActions.stop(id);
        ui.toast("info", `${a.name} stopped.`);
      } catch (e) {
        ui.toast("danger", errorMessage(e));
      }
    },
  },
];
