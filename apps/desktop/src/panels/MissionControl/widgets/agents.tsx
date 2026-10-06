import type { AgentInstance, AgentState } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Bot } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { openProject } from "../../../core/client";
import { useFormat, useLanguage, useT } from "../../../i18n";
import { agentActivity, agentName } from "../../../lib/coreText";
import { useAppStore } from "../../../store/app";
import { elapsed, roleInitials, roleLabel } from "../../Agents/model";
import { useAgentsUi } from "../../Agents/store";
import { type WidgetProps, useMission } from "../context";
import { useActiveAgents, visibleAgents } from "../data";
import { WidgetEmpty, WidgetStatus } from "./common";

/** Re-renders every `ms` while mounted (live elapsed counters). */
export function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(id);
  }, [ms]);
  return now;
}

/**
 * Procedural agent "core" (robot placeholder until the robots area binds
 * its renderer, §30): concentric rings whose motion encodes the state —
 * orbit (thinking), scan (coding), pulse (testing/reviewing), blink
 * (waiting), flare (completed), shake-free red (error). CSS-only; frozen
 * when the visual budget disables animation.
 */
export function AgentOrb({ state, initials, size = 44 }: { state: AgentState; initials: string; size?: number }) {
  return (
    <span className={clsx("mc-orb", `mc-orb--${state.toLowerCase()}`)} style={{ width: size, height: size }} aria-hidden="true">
      <svg viewBox="0 0 48 48" width={size} height={size}>
        <circle cx="24" cy="24" r="21" className="mc-orb__halo" />
        <circle cx="24" cy="24" r="17" className="mc-orb__ring mc-orb__ring--outer" />
        <circle cx="24" cy="24" r="12.5" className="mc-orb__ring mc-orb__ring--inner" />
        <circle cx="24" cy="24" r="9" className="mc-orb__core" />
        <rect x="10" y="23" width="28" height="2" rx="1" className="mc-orb__scan" />
      </svg>
      <span className="mc-orb__label">{initials}</span>
    </span>
  );
}

export function AgentCard({ agent, now, projectName }: { agent: AgentInstance; now: number; projectName: string | null }) {
  const { t } = useT(["missionControl", "common"]);
  const fmt = useFormat();
  const lng = useLanguage();
  const displayName = agentName(agent.name);
  const open = () => {
    if (useAppStore.getState().activeProjectId !== agent.projectId) void openProject(agent.projectId);
    const ui = useAgentsUi.getState();
    ui.setTab("agents");
    ui.select(agent.id);
    workspaceActions.openPanel("agents");
  };
  return (
    <button type="button" className={clsx("mc-agent", `mc-agent--${agent.state.toLowerCase()}`)} onClick={open} title={t("agents.openDetails", { name: displayName })}>
      <AgentOrb state={agent.state} initials={roleInitials(agent.role)} />
      <span className="mc-agent__body">
        <span className="mc-agent__name">{displayName}</span>
        <span className="mc-agent__state">
          {t(`common:status.agent.${agent.state}`)}
          {projectName && <span className="mc-agent__project"> · {projectName}</span>}
        </span>
        <span className="mc-agent__activity">{agent.activity ? agentActivity(agent.activity) : roleLabel(agent.role)}</span>
        <span className="mc-agent__meta">
          <span title={t("agents.model")}>{agent.model?.model ?? t("agents.noModel")}</span>
          <span title={t("agents.elapsed")}>{elapsed(agent, now, lng)}</span>
          <span title={t("agents.cost")}>{fmt.currency(agent.usage.costUsd)}</span>
        </span>
      </span>
    </button>
  );
}

/** Active agents grid with state animations. */
export function AgentsWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { budget } = useMission();
  const { agents, loading, error } = useActiveAgents();
  const projects = useAppStore((s) => s.projects);
  const now = useNow(1000);
  const shown = visibleAgents(agents, now);
  if (shown.length === 0) {
    if (loading || error) return <WidgetStatus loading={loading} error={error} />;
    return <WidgetEmpty icon={<Bot size={20} />} title={t("agents.empty")} hint={t("agents.emptyHint")} />;
  }
  const name = (id: string) => projects.find((p) => p.id === id)?.name ?? null;
  return (
    <div className="mc-agents" role="list" aria-label={t("agents.listLabel", { count: shown.length })}>
      <AnimatePresence initial={false}>
        {shown.map((a) => (
          <motion.div
            key={a.id}
            role="listitem"
            layout={budget.animate}
            initial={budget.animate ? { opacity: 0, scale: 0.94 } : false}
            animate={{ opacity: 1, scale: 1 }}
            exit={budget.animate ? { opacity: 0, scale: 0.94 } : { opacity: 0 }}
            transition={{ duration: budget.animate ? 0.24 : 0 }}
          >
            <AgentCard agent={a} now={now} projectName={name(a.projectId)} />
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
