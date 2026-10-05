import type { BudgetStatus } from "@orkestr/protocol";
import { Coins } from "lucide-react";

import { workspaceActions } from "../../../components/workspaceActions";
import { useFormat, useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { levelLabel, periodLabel, scopeLabel } from "../../Costs/format";
import { type ChartTone, Gauge } from "../../Monitoring/charts";
import { type WidgetProps, useMission } from "../context";
import { useBudgets } from "../data";
import { WidgetStatus } from "./common";

const LEVEL_CHART: Record<BudgetStatus["level"], ChartTone> = {
  Ok: "success",
  Warn: "warning",
  Exceeded: "danger",
  Unrestricted: "violet",
  Disabled: "info",
};

/** Budgets that matter most: enabled, limited, highest usage first. */
export function topBudgets(list: readonly BudgetStatus[], n = 4): BudgetStatus[] {
  return list
    .filter((b) => b.budget.enabled && !b.budget.unrestricted && b.budget.limitUsd !== null)
    .sort((a, b) => (b.fraction ?? 0) - (a.fraction ?? 0))
    .slice(0, n);
}

/** Spend today/month and the tightest budgets as gauges. */
export function BudgetsWidget(_props: WidgetProps) {
  const { t } = useT(["missionControl", "common"]);
  const fmt = useFormat();
  const { snapshot, budget: visual } = useMission();
  const budgets = useBudgets();
  const projects = useAppStore((s) => s.projects);
  const spend = snapshot.data?.spend;
  if (!spend) return <WidgetStatus loading={snapshot.loading || !snapshot.error} error={snapshot.error} />;
  const top = topBudgets(budgets.data ?? []);
  return (
    <div className="mc-money">
      <div className="mc-money__spend">
        <div className="mc-hero">
          <span className="mc-hero__label">{t("budgets.today")}</span>
          <span className="mc-hero__value">{fmt.currency(spend.todayUsd)}</span>
          <span className="mc-hero__hint">
            {spend.todayEstimatedUsd > 0
              ? t("budgets.estimated", { amount: fmt.currency(spend.todayEstimatedUsd) })
              : t("common:count.tokens", { count: spend.todayTokens })}
          </span>
        </div>
        <div className="mc-hero">
          <span className="mc-hero__label">{t("budgets.month")}</span>
          <span className="mc-hero__value">{fmt.currency(spend.monthUsd)}</span>
          {spend.monthEstimatedUsd > 0 && <span className="mc-hero__hint">{t("budgets.estimated", { amount: fmt.currency(spend.monthEstimatedUsd) })}</span>}
        </div>
      </div>
      {top.length === 0 ? (
        <div className="mc-money__none">
          <Coins size={16} aria-hidden="true" />
          <span>{t("budgets.none")}</span>
          <button type="button" className="mc-link" onClick={() => workspaceActions.openPanel("costs")}>
            {t("budgets.setUp")}
          </button>
        </div>
      ) : (
        <div className="mc-gauges">
          {top.map((b) => (
            <div key={b.budget.id} className="mc-gauge">
              <Gauge
                value={b.fraction}
                tone={LEVEL_CHART[b.level]}
                center={b.fraction === null ? "—" : fmt.percent(b.fraction)}
                caption={levelLabel(b.level)}
                label={t("budgets.gaugeLabel", { scope: scopeLabel(b.budget.scope, projects), value: b.fraction === null ? "—" : fmt.percent(b.fraction) })}
                size={84}
                rich={visual.rich}
              />
              <span className="mc-gauge__scope">{b.budget.name ?? scopeLabel(b.budget.scope, projects)}</span>
              <span className="mc-gauge__detail">
                {t("budgets.spentOf", { spent: fmt.currency(b.spentUsd), limit: fmt.currency(b.budget.limitUsd ?? 0) })} · {periodLabel(b.budget.period)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
