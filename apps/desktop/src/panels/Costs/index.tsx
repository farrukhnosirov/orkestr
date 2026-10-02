import type { BudgetRule, BudgetStatus, CostBreakdownRow, CostDimension, CostPoint, CostSummaryResult, CostWindow } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, Coins, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";

import { useFormat, useLanguage, useT } from "../../i18n";
import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { BudgetEditor } from "./BudgetEditor";
import "./costs.css";
import { formatPct, formatTokens, formatUsd, LEVEL_TONE, levelLabel, limitLabel, scopeLabel } from "./format";
import { SpendChart } from "./SpendChart";

const DIMENSIONS: CostDimension[] = ["Provider", "Model", "Agent", "Project", "Task"];

const WINDOWS: CostWindow[] = ["Today", "Month", "Last30Days", "All"];

type Scope = "project" | "all";

const RELOAD_ON = ["BudgetThreshold"] as const;

function Kpi({ label, value, sub, tone }: { label: string; value: string; sub?: string; tone?: "warning" | "danger" | undefined }) {
  return (
    <div className={clsx("cost-kpi", tone && `cost-kpi--${tone}`)}>
      <span className="cost-kpi__label">{label}</span>
      <span className="cost-kpi__value">{value}</span>
      {sub && <span className="cost-kpi__sub">{sub}</span>}
    </div>
  );
}

function Meter({ fraction, warnAt }: { fraction: number | null; warnAt: number }) {
  const { t } = useT("costs");
  if (fraction === null) return <span className="cost-meter cost-meter--none" aria-hidden="true" />;
  const pct = Math.min(1, Math.max(0, fraction));
  const state = fraction >= 1 ? "danger" : fraction >= warnAt ? "warning" : "ok";
  return (
    <span className="cost-meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction * 100)} aria-label={t("budgets.used")}>
      <span className={`cost-meter__fill cost-meter__fill--${state}`} style={{ width: `${pct * 100}%` }} />
      <span className="cost-meter__tick" style={{ left: `${warnAt * 100}%` }} aria-hidden="true" />
    </span>
  );
}

/** Cost & Limit Center (§22). All amounts include clearly labelled estimates. */
export default function CostsPanel({ projectId }: PanelProps) {
  const { t } = useT(["costs", "common"]);
  const lng = useLanguage();
  const fmt = useFormat();
  const usd = (v: number) => formatUsd(v, lng);
  const tokens = (n: number) => formatTokens(n, lng);
  const pct = (f: number | null | undefined) => formatPct(f, lng);
  const connected = useAppStore((s) => s.connection.state === "open");
  const projects = useAppStore((s) => s.projects);
  const [scope, setScope] = useState<Scope>(projectId ? "project" : "all");
  const [dimension, setDimension] = useState<CostDimension>("Provider");
  const [window, setWindow] = useState<CostWindow>("Month");
  const [editing, setEditing] = useState<BudgetRule | "new" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const pid = scope === "project" ? projectId : null;
  const filter = pid ? { projectId: pid } : {};
  const topics = pid ? ["global", topic.project(pid)] : ["*"];

  const summary = useRpcQuery<CostSummaryResult>(`cost-summary:${pid ?? "all"}`, (c) => c.call("cost.summary", filter), {
    topics,
    reloadOn: RELOAD_ON,
  });
  const breakdown = useRpcQuery<CostBreakdownRow[]>(
    `cost-dim:${pid ?? "all"}:${dimension}:${window}`,
    async (c) => (await c.call("cost.byDimension", { dimension, window, ...filter })).rows,
    { topics, reloadOn: RELOAD_ON },
  );
  const series = useRpcQuery<CostPoint[]>(
    `cost-ts:${pid ?? "all"}`,
    async (c) => (await c.call("cost.timeseries", { granularity: "Day", buckets: 30, ...filter })).points,
    { topics, reloadOn: RELOAD_ON },
  );
  const budgets = useRpcQuery<BudgetStatus[]>(
    `budgets:${pid ?? "all"}`,
    async (c) => (await c.call("budgets.list", filter)).budgets,
    { topics, reloadOn: RELOAD_ON },
  );

  const reloadAll = () => {
    summary.reload();
    breakdown.reload();
    series.reload();
    budgets.reload();
  };

  const removeBudget = async (b: BudgetRule) => {
    if (!globalThis.confirm(t("budgets.confirmDelete", { name: b.name ?? scopeLabel(b.scope, projects) }))) return;
    setActionError(null);
    try {
      await requireClient().call("budgets.delete", { id: b.id });
      reloadAll();
    } catch (e) {
      setActionError(errorMessage(e));
    }
  };

  if (!connected) {
    return (
      <div className="cost-panel cost-panel--center">
        <EmptyState icon={<Coins size={26} strokeWidth={1.6} />} title={t("notConnected.title")} description={t("notConnected.description")} />
      </div>
    );
  }

  const s = summary.data;
  const used = s?.budgetUsed ?? null;
  const rows = breakdown.data ?? [];
  const points = series.data ?? [];
  const hasSpend = points.some((p) => p.totals.requests > 0);
  const anyLoading = summary.loading || breakdown.loading || series.loading || budgets.loading;
  const loadError = summary.error ?? breakdown.error ?? series.error ?? budgets.error;
  const labelFor = (r: CostBreakdownRow): string => {
    if (r.key === "none") return t("breakdown.unattributed");
    if (dimension === "Project") return projects.find((p) => p.id === r.key)?.name ?? `…${r.key.slice(-6)}`;
    if (dimension === "Model") return r.model ?? r.key;
    return `…${r.key.slice(-8)}`;
  };

  return (
    <div className="cost-panel">
      <div className="cost-toolbar">
        <Tabs
          aria-label={t("scope.label")}
          items={[
            { id: "project" as const, label: t("scope.project"), disabled: !projectId },
            { id: "all" as const, label: t("scope.all") },
          ]}
          value={scope}
          onChange={setScope}
        />
        <Badge tone="violet" title={t("estimates.tooltip")}>
          {t("estimates.badge")}
        </Badge>
        <div className="cost-toolbar__spacer" />
        {anyLoading && <Spinner size={14} />}
        <IconButton label={t("common:actions.refresh")} icon={<RefreshCw size={14} />} onClick={reloadAll} tooltip="bottom" />
      </div>

      {(loadError || actionError) && (
        <div className="cost-error" role="alert">
          <AlertTriangle size={14} /> {actionError ?? loadError}
        </div>
      )}

      <div className="cost-scroll">
        <section className="cost-kpis" aria-label={t("kpi.label")}>
          <Kpi
            label={t("kpi.today")}
            value={s ? usd(s.today.costUsd) : "—"}
            sub={s ? t("kpi.todaySub", { estimated: usd(s.today.estimatedCostUsd), count: s.today.requests }) : undefined}
          />
          <Kpi
            label={t("kpi.month")}
            value={s ? usd(s.month.costUsd) : "—"}
            sub={s ? t("kpi.monthSub", { tokens: tokens(s.month.inputTokens + s.month.outputTokens), estimated: usd(s.month.estimatedCostUsd) }) : undefined}
          />
          <Kpi
            label={t("kpi.budgetUsed")}
            value={pct(used)}
            sub={used === null ? t("kpi.noBudget") : used >= 1 ? t("kpi.limitReached") : t("kpi.highestBudget")}
            tone={used === null ? undefined : used >= 1 ? "danger" : used >= 0.8 ? "warning" : undefined}
          />
          <Kpi
            label={t("kpi.allTime")}
            value={s ? usd(s.total.costUsd) : "—"}
            sub={s ? t("kpi.allTimeSub", { count: s.total.requests, failed: fmt.integer(s.total.failures) }) : undefined}
          />
        </section>

        <section className="cost-card" aria-labelledby="cost-daily-title">
          <header className="cost-card__head">
            <h3 id="cost-daily-title">{t("daily.title")}</h3>
          </header>
          {series.loading && series.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> {t("common:state.loading")}
            </div>
          ) : hasSpend ? (
            <SpendChart points={points} />
          ) : (
            <p className="cost-empty">{t("daily.empty")}</p>
          )}
        </section>

        <section className="cost-card" aria-labelledby="cost-breakdown-title">
          <header className="cost-card__head">
            <h3 id="cost-breakdown-title">{t("breakdown.title")}</h3>
            <Tabs
              aria-label={t("dimension.label")}
              items={DIMENSIONS.map((id) => ({ id, label: t(`dimension.${id}`) }))}
              value={dimension}
              onChange={setDimension}
            />
            <Tabs aria-label={t("window.label")} items={WINDOWS.map((id) => ({ id, label: t(`window.${id}`) }))} value={window} onChange={setWindow} />
          </header>
          {breakdown.loading && breakdown.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> {t("common:state.loading")}
            </div>
          ) : rows.length === 0 ? (
            <p className="cost-empty">{t("breakdown.empty")}</p>
          ) : (
            <div className="cost-table-wrap">
              <table className="cost-table">
                <thead>
                  <tr>
                    <th scope="col">{t(`dimension.${dimension}`)}</th>
                    <th scope="col" className="num">
                      {t("breakdown.cost")}
                    </th>
                    <th scope="col" className="num">
                      {t("breakdown.estimated")}
                    </th>
                    <th scope="col" className="num">
                      {t("breakdown.input")}
                    </th>
                    <th scope="col" className="num">
                      {t("breakdown.output")}
                    </th>
                    <th scope="col" className="num">
                      {t("breakdown.calls")}
                    </th>
                    <th scope="col" className="num">
                      {t("breakdown.latency")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <tr key={r.key}>
                      <th scope="row" title={r.key}>
                        {labelFor(r)}
                      </th>
                      <td className="num">{usd(r.totals.costUsd)}</td>
                      <td className="num cost-muted">{r.totals.estimatedCostUsd > 0 ? usd(r.totals.estimatedCostUsd) : "—"}</td>
                      <td className="num">{tokens(r.totals.inputTokens)}</td>
                      <td className="num">{tokens(r.totals.outputTokens)}</td>
                      <td className="num">
                        {fmt.integer(r.totals.requests)}
                        {r.totals.failures > 0 && <span className="cost-fail"> {t("breakdown.failed", { count: r.totals.failures })}</span>}
                      </td>
                      <td className="num">{r.avgLatencyMs != null ? fmt.duration(Math.round(r.avgLatencyMs)) : "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="cost-card" aria-labelledby="cost-budgets-title">
          <header className="cost-card__head">
            <h3 id="cost-budgets-title">{t("budgets.title")}</h3>
            <div className="cost-toolbar__spacer" />
            <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditing("new")}>
              {t("budgets.new")}
            </Button>
          </header>
          {budgets.loading && budgets.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> {t("common:state.loading")}
            </div>
          ) : (budgets.data ?? []).length === 0 ? (
            <p className="cost-empty">{t("budgets.empty")}</p>
          ) : (
            <ul className="cost-budgets">
              {(budgets.data ?? []).map((b) => (
                <li key={b.budget.id} className={clsx("cost-budget", !b.budget.enabled && "cost-budget--disabled")}>
                  <div className="cost-budget__main">
                    <div className="cost-budget__title">
                      <strong>{b.budget.name ?? scopeLabel(b.budget.scope, projects)}</strong>
                      <Badge tone={LEVEL_TONE[b.level]}>{levelLabel(b.level)}</Badge>
                    </div>
                    <span className="cost-budget__desc">
                      {[
                        ...(b.budget.name ? [scopeLabel(b.budget.scope, projects)] : []),
                        b.budget.unrestricted ? t("budgets.unrestricted") : limitLabel(b.budget.limitUsd ?? 0, b.budget.period),
                        b.budget.period === "PerTask"
                          ? t("budgets.spentTopTask", { amount: usd(b.spentUsd) })
                          : t("budgets.spent", { amount: usd(b.spentUsd) }),
                      ].join(" · ")}
                    </span>
                  </div>
                  <Meter fraction={b.budget.unrestricted ? null : b.fraction} warnAt={b.budget.warnAt} />
                  <span className="cost-budget__pct">{b.budget.unrestricted ? "∞" : pct(b.fraction)}</span>
                  <div className="cost-budget__actions">
                    <IconButton size="sm" label={t("budgets.edit")} icon={<Pencil size={14} />} onClick={() => setEditing(b.budget)} />
                    <IconButton size="sm" variant="danger" label={t("budgets.delete")} icon={<Trash2 size={14} />} onClick={() => void removeBudget(b.budget)} />
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      {editing && (
        <BudgetEditor
          budget={editing === "new" ? null : editing}
          projects={projects}
          defaultProjectId={projectId}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            reloadAll();
          }}
        />
      )}
    </div>
  );
}
