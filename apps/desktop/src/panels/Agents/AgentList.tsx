import type { AgentInstance, Task } from "@orkestr/protocol";
import { clsx } from "clsx";
import { GitBranch } from "lucide-react";

import { elapsed, formatCost, formatTokens } from "./model";
import { RoleAvatar, StateChip } from "./parts";
import { useAgentsUi } from "./store";

export function AgentList({ agents, tasks, now }: { agents: AgentInstance[]; tasks: Task[]; now: number }) {
  const selected = useAgentsUi((s) => s.selectedAgentId);
  const select = useAgentsUi((s) => s.select);
  const titles = new Map(tasks.map((t) => [t.id, t.title]));
  return (
    <ul className="agents-list" aria-label="Agents">
      {agents.map((a) => {
        const task = a.taskId ? titles.get(a.taskId) : undefined;
        return (
          <li key={a.id}>
            <button
              type="button"
              className={clsx("agents-row", selected === a.id && "agents-row--selected")}
              aria-current={selected === a.id ? "true" : undefined}
              onClick={() => select(a.id)}
            >
              <RoleAvatar role={a.role} state={a.state} />
              <span className="agents-row__main">
                <span className="agents-row__top">
                  <span className="agents-row__name">{a.name}</span>
                  <StateChip state={a.state} />
                </span>
                <span className="agents-row__task" title={task}>
                  {task ?? "No task"}
                </span>
                {a.activity && <span className="agents-row__activity">{a.activity}</span>}
                <span className="agents-row__meta">
                  <span title="Model">{a.model?.model ?? "—"}</span>
                  {a.worktree?.branch && (
                    <span className="agents-row__branch" title={a.worktree.path}>
                      <GitBranch size={11} aria-hidden="true" />
                      {a.worktree.branch}
                    </span>
                  )}
                  <span title="Tokens (in + out)">{formatTokens(a.usage.inputTokens + a.usage.outputTokens)} tok</span>
                  <span title="Cost (estimated)">{formatCost(a.usage.costUsd)}</span>
                  <span title="Elapsed">{elapsed(a, now)}</span>
                </span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
