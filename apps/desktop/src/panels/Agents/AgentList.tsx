import type { AgentInstance, Task } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Copy, GitBranch, OctagonX, Pause, Play, SquareArrowOutUpRight } from "lucide-react";

import { contextMenuHandler, type MenuEntry, writeClipboardText } from "../../features/contextMenu";
import { agentActions, errorMessage } from "./data";
import { isActive } from "./model";

import { useLanguage, useT } from "../../i18n";
import { elapsed, formatCost, formatTokens, tokenPluralCount } from "./model";

import { RoleAvatar, StateChip } from "./parts";
import { useAgentsUi } from "./store";
import { agentActivity, agentName, PAUSED_ACTIVITY } from "../../lib/coreText";

/** Context menu of an agent row (exported for tests). */
export function agentRowMenu(a: AgentInstance, t: (key: string, opts?: Record<string, unknown>) => string): MenuEntry[] {
  const ui = useAgentsUi.getState();
  const active = isActive(a.state);
  const paused = a.state === "Waiting" && a.activity === PAUSED_ACTIVITY;
  const act = (fn: () => Promise<AgentInstance>, done?: (x: AgentInstance) => void) => async () => {
    try {
      const r = await fn();
      done?.(r);
    } catch (e) {
      ui.toast("danger", errorMessage(e));
    }
  };
  return [
    { id: "agent.open", label: t("contextMenu:common.open"), icon: <SquareArrowOutUpRight size={14} />, onSelect: () => ui.select(a.id) },
    { kind: "separator", id: "agent.s1" },
    {
      id: "agent.pause",
      label: t(paused ? "contextMenu:agents.resume" : "contextMenu:agents.pause"),
      icon: paused ? <Play size={14} /> : <Pause size={14} />,
      hidden: !active,
      onSelect: act(() => (paused ? agentActions.resume(a.id) : agentActions.pause(a.id))),
    },
    {
      id: "agent.stop",
      label: t("contextMenu:agents.stop"),
      icon: <OctagonX size={14} />,
      danger: true,
      hidden: !active,
      onSelect: act(() => agentActions.stop(a.id), (r) => ui.toast("info", t("agents:stopped", { name: agentName(r.name) }))),
    },
    { kind: "separator", id: "agent.s2" },
    { id: "agent.copyName", label: t("contextMenu:common.copyName"), icon: <Copy size={14} />, onSelect: () => void writeClipboardText(agentName(a.name)) },
    { id: "agent.copyId", label: t("contextMenu:common.copyId"), icon: <Copy size={14} />, onSelect: () => void writeClipboardText(a.id) },
    {
      id: "agent.copyBranch",
      label: t("contextMenu:common.copyBranch"),
      icon: <GitBranch size={14} />,
      hidden: !a.worktree?.branch,
      onSelect: () => void writeClipboardText(a.worktree?.branch ?? ""),
    },
  ];
}

export function AgentList({ agents, tasks, now }: { agents: AgentInstance[]; tasks: Task[]; now: number }) {
  const { t } = useT(["agents", "contextMenu"]);
  const lng = useLanguage();
  const selected = useAgentsUi((s) => s.selectedAgentId);
  const select = useAgentsUi((s) => s.select);
  const titles = new Map(tasks.map((x) => [x.id, x.title]));
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
              onContextMenu={contextMenuHandler(() => agentRowMenu(a, t as (key: string, opts?: Record<string, unknown>) => string))}
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
