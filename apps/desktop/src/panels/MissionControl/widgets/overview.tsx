import type { ProjectActivity } from "@orkestr/protocol";
import { Badge, type Tone } from "@orkestr/ui";
import { clsx } from "clsx";
import { FolderOpen, FolderPlus, OctagonX } from "lucide-react";

import { openProject } from "../../../core/client";
import { useFormat, useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { type WidgetProps, useMission } from "../context";
import { Stat, WidgetEmpty, WidgetStatus } from "./common";

const ACTIVITY_TONE: Record<ProjectActivity, Tone> = { Idle: "neutral", Active: "accent", Attention: "warning" };

/** KPI strip: the whole system at a glance. */
export function OverviewWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const fmt = useFormat();
  const { snapshot } = useMission();
  const s = snapshot.data;
  if (!s) return <WidgetStatus loading={snapshot.loading || !snapshot.error} error={snapshot.error} />;
  return (
    <div className="mc-overview">
      {s.emergencyStopped && (
        <div className="mc-banner mc-banner--danger" role="alert">
          <OctagonX size={15} aria-hidden="true" />
          {t("overview.emergency")}
        </div>
      )}
      <div className="mc-kpis">
        <Stat value={fmt.integer(s.totals.projects)} label={t("overview.projects")} />
        <Stat value={fmt.integer(s.totals.activeAgents)} label={t("overview.activeAgents")} tone={s.totals.activeAgents > 0 ? "accent" : undefined} hint={t("overview.ofTotal", { total: fmt.integer(s.totals.totalAgents) })} />
        <Stat value={fmt.integer(s.totals.runningTasks)} label={t("overview.runningTasks")} tone={s.totals.runningTasks > 0 ? "violet" : undefined} />
        <Stat value={fmt.integer(s.totals.liveRuns)} label={t("overview.liveRuns")} />
        <Stat value={fmt.integer(s.totals.pendingApprovals)} label={t("overview.pendingApprovals")} tone={s.totals.pendingApprovals > 0 ? "warning" : undefined} />
        <Stat value={fmt.integer(s.totals.failedTasks)} label={t("overview.failedTasks")} tone={s.totals.failedTasks > 0 ? "danger" : undefined} />
        <Stat value={fmt.currency(s.spend.todayUsd)} label={t("overview.spendToday")} hint={t("overview.spendMonth", { amount: fmt.currency(s.spend.monthUsd) })} />
      </div>
    </div>
  );
}

/** Projects overview: status, agents, tasks, spend. */
export function ProjectsWidget(_props: WidgetProps) {
  const { t } = useT(["missionControl", "common"]);
  const fmt = useFormat();
  const { snapshot, focusProjectId, setFocusProjectId } = useMission();
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const rows = snapshot.data?.projects;
  if (!rows) return <WidgetStatus loading={snapshot.loading || !snapshot.error} error={snapshot.error} />;
  if (rows.length === 0) {
    return (
      <WidgetEmpty
        icon={<FolderPlus size={20} />}
        title={t("projects.empty")}
        hint={t("projects.emptyHint")}
      />
    );
  }
  return (
    <ul className="mc-projects" aria-label={t("projects.listLabel")}>
      {rows.map((p) => (
        <li key={p.projectId}>
          <button
            type="button"
            className={clsx("mc-project", focusProjectId === p.projectId && "mc-project--focus")}
            onClick={() => setFocusProjectId(p.projectId)}
            aria-pressed={focusProjectId === p.projectId}
            title={t("projects.focus", { name: p.name })}
          >
            <span className="mc-project__dot" style={p.color ? { background: p.color } : undefined} aria-hidden="true" />
            <span className="mc-project__name">
              {p.name}
              {p.projectId === activeProjectId && <span className="mc-project__current">{t("projects.current")}</span>}
            </span>
            <Badge tone={ACTIVITY_TONE[p.activity]} className="mc-project__activity">
              {t(`projects.activity.${p.activity}`)}
            </Badge>
            <span className="mc-project__metric" title={t("projects.agentsTitle")}>
              {t("projects.agents", { active: fmt.integer(p.activeAgents), total: fmt.integer(p.totalAgents) })}
            </span>
            <span className="mc-project__metric" title={t("projects.tasksTitle")}>
              {t("projects.tasks", { running: fmt.integer(p.runningTasks), open: fmt.integer(p.openTasks) })}
            </span>
            {p.failedTasks > 0 && (
              <span className="mc-project__metric mc-project__metric--danger">{t("projects.failed", { count: p.failedTasks })}</span>
            )}
            {p.pendingApprovals > 0 && (
              <span className="mc-project__metric mc-project__metric--warning">{t("common:count.approvals", { count: p.pendingApprovals })}</span>
            )}
            <span className="mc-project__spend" title={t("projects.spendTitle")}>
              {fmt.currency(p.spendMonthUsd)}
            </span>
          </button>
          {p.projectId !== activeProjectId && (
            <button
              type="button"
              className="mc-icon-btn"
              aria-label={t("projects.open", { name: p.name })}
              title={t("projects.open", { name: p.name })}
              onClick={() => void openProject(p.projectId)}
            >
              <FolderOpen size={14} />
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}
