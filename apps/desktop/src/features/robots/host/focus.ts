/**
 * Main-window reactions to robot requests: open the Agents panel at an
 * agent (robot click, Scenario H), assign a robot to a project.
 */
import { workspaceActions } from "../../../components/workspaceActions";
import { maybeClient, openProject } from "../../../core/client";
import { t } from "../../../i18n";
import { describeError } from "../../../i18n/errors";
import { useAgentsUi } from "../../../panels/Agents/store";
import { useAppStore } from "../../../store/app";
import { useNotificationsStore } from "../../notifications/notificationsStore";

/** Opens the Agents panel with `agentId` selected (switching project if needed). */
export async function focusAgent(agentId: string, projectId?: string | null): Promise<void> {
  let project = projectId ?? null;
  if (!project) {
    try {
      const c = maybeClient();
      if (c) project = (await c.call("agents.get", { agentId })).agent.projectId;
    } catch (e) {
      console.warn("robots: agents.get failed", e);
    }
  }
  if (project && useAppStore.getState().activeProjectId !== project) await openProject(project);
  workspaceActions.openPanel("agents");
  const ui = useAgentsUi.getState();
  ui.setTab("agents");
  ui.select(agentId);
}

/** Spawns `templateId` in `projectId` (the "assign" of a dropped robot). */
export async function spawnInProject(templateId: string, projectId: string): Promise<string | null> {
  const c = maybeClient();
  if (!c) return null;
  try {
    const { agent } = await c.call("agents.spawn", { projectId, templateId });
    useNotificationsStore.getState().notify("Success", t("robots:assign.doneTitle"), t("robots:assign.doneBody", { name: agent.name }));
    await focusAgent(agent.id, projectId);
    return agent.id;
  } catch (e) {
    useNotificationsStore.getState().notify("Error", t("robots:assign.failedTitle"), describeError(e));
    return null;
  }
}
