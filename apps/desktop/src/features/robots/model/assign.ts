/**
 * Dropping a robot on a project in the ProjectRail (§30 "drag robot onto a
 * project = assign the agent"). An agent instance belongs to one project for
 * its whole life (worktree, budget, memory scope), so "assign" spawns the
 * robot's template as a new agent in the target project; the original agent
 * keeps running where it is.
 *
 * INTEGRATOR(P2): if Phase 2 (teams/PM) adds `agents.assign {agentId,
 * projectId}`, switch the "spawn" plan to it.
 */
export type AssignPlan =
  | { kind: "spawn"; templateId: string; projectId: string }
  | { kind: "sameProject" }
  | { kind: "noTemplate" }
  | { kind: "unknownProject" }
  | { kind: "unknownAgent" };

export interface AssignInput {
  agent: { projectId: string; templateId: string | null } | null;
  projectId: string;
  projectIds: readonly string[];
}

export function planAssign({ agent, projectId, projectIds }: AssignInput): AssignPlan {
  if (!agent) return { kind: "unknownAgent" };
  if (!projectIds.includes(projectId)) return { kind: "unknownProject" };
  if (agent.projectId === projectId) return { kind: "sameProject" };
  if (!agent.templateId) return { kind: "noTemplate" };
  return { kind: "spawn", templateId: agent.templateId, projectId };
}
