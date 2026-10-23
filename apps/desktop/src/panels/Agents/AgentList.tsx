import type { AgentInstance, Task } from "@orkestr/protocol";
import { clsx } from "clsx";
import { GitBranch } from "lucide-react";

import { AgentEngineBadge } from "../../features/engines/AgentEngineBadge";
import { useProvidersLive } from "../../features/providers/store";
import { useLanguage, useT } from "../../i18n";
import { elapsed, formatCost, formatTokens, tokenPluralCount } from "./model";
import { RoleAvatar, StateChip } from "./parts";
import { useAgentsUi } from "./store";
import { agentActivity, agentName } from "../../lib/coreText";

export function AgentList({ agents, tasks, now }: { agents: AgentInstance[]; tasks: Task[]; now: number }) {
  const { t } = useT("agents");
  const lng = useLanguage();
  const selected = useAgentsUi((s) => s.selectedAgentId);
  const select = useAgentsUi((s) => s.select);
  const titles = new Map(tasks.map((x) => [x.id, x.title]));
  // Providers tell which engine (Claude Code / Codex / API) each agent runs on.
  useProvidersLive();
  return (
    <ul className="agents-list" aria-label={t("list.label")}>
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
                  <span className="agents-row__name">{agentName(a.name)}</span>
                  <StateChip state={a.state} />
                </span>
                <span className="agents-row__task" title={task}>
                  {task ?? t("list.noTask")}
                </span>
                {a.activity && <span className="agents-row__activity">{agentActivity(a.activity)}</span>}
                <span className="agents-row__meta">
                  <AgentEngineBadge providerId={a.model?.providerId} />
                  <span title={t("list.model")}>{a.model?.model ?? "—"}</span>
                  {a.worktree?.branch && (
                    <span className="agents-row__branch" title={a.worktree.path}>
                      <GitBranch size={11} aria-hidden="true" />
                      {a.worktree.branch}
                    </span>
                  )}
                  <span title={t("list.tokensTitle")}>
                    {t("list.tokens", {
                      // Compact values («6,6 тыс.») take the noun form of «тысяча» / «миллион»: «тыс. токенов».
                      count: tokenPluralCount(a.usage.inputTokens + a.usage.outputTokens),
                      value: formatTokens(a.usage.inputTokens + a.usage.outputTokens, lng),
                    })}
                  </span>
                  <span title={t("list.costTitle")}>{formatCost(a.usage.costUsd, lng)}</span>
                  <span title={t("list.elapsedTitle")}>{elapsed(a, now, lng)}</span>
                </span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
