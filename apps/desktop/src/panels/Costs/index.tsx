import type { BudgetRule, BudgetStatus, CostBreakdownRow, CostDimension, CostPoint, CostSummaryResult, CostWindow } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, Coins, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";

import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { BudgetEditor } from "./BudgetEditor";
import "./costs.css";
import { formatPct, formatTokens, formatUsd, LEVEL_TONE, PERIOD_LABEL, scopeLabel } from "./format";
import { SpendChart } from "./SpendChart";

const DIMENSIONS: { id: CostDimension; label: string }[] = [
  { id: "Provider", label: "Provider" },
  { id: "Model", label: "Model" },
  { id: "Agent", label: "Agent" },
  { id: "Project", label: "Project" },
  { id: "Task", label: "Task" },
];

const WINDOWS: { id: CostWindow; label: string }[] = [
  { id: "Today", label: "Today" },
  { id: "Month", label: "Month" },
  { id: "Last30Days", label: "30 days" },
  { id: "All", label: "All" },
];

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
  if (fraction === null) return <span className="cost-meter cost-meter--none" aria-hidden="true" />;
  const pct = Math.min(1, Math.max(0, fraction));
  const state = fraction >= 1 ? "danger" : fraction >= warnAt ? "warning" : "ok";
  return (
    <span className="cost-meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction * 100)} aria-label="Budget used">
      <span className={`cost-meter__fill cost-meter__fill--${state}`} style={{ width: `${pct * 100}%` }} />
      <span className="cost-meter__tick" style={{ left: `${warnAt * 100}%` }} aria-hidden="true" />
    </span>
  );
}

/** Cost & Limit Center (§22). All amounts include clearly labelled estimates. */
export default function CostsPanel({ projectId }: PanelProps) {
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
    if (!globalThis.confirm(`Delete budget "${b.name ?? scopeLabel(b.scope, projects)}"?`)) return;
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
        <EmptyState icon={<Coins size={26} strokeWidth={1.6} />} title="Not connected" description="Waiting for the Orkestr core…" />
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
    if (r.key === "none") return "Unattributed";
    if (dimension === "Project") return projects.find((p) => p.id === r.key)?.name ?? `…${r.key.slice(-6)}`;
    if (dimension === "Model") return r.model ?? r.key;
    return `…${r.key.slice(-8)}`;
  };

  return (
    <div className="cost-panel">
      <div className="cost-toolbar">
        <Tabs
          aria-label="Cost scope"
          items={[
            { id: "project" as const, label: "This project", disabled: !projectId },
            { id: "all" as const, label: "All projects" },
          ]}
          value={scope}
          onChange={setScope}
        />
        <Badge tone="violet" title="Subscription / CLI usage is costed from list prices">
          Includes estimates
        </Badge>
        <div className="cost-toolbar__spacer" />
        {anyLoading && <Spinner size={14} />}
        <IconButton label="Refresh" icon={<RefreshCw size={14} />} onClick={reloadAll} tooltip="bottom" />
      </div>

      {(loadError || actionError) && (
        <div className="cost-error" role="alert">
          <AlertTriangle size={14} /> {actionError ?? loadError}
        </div>
      )}

      <div className="cost-scroll">
        <section className="cost-kpis" aria-label="Key figures">
          <Kpi
            label="Today"
            value={s ? formatUsd(s.today.costUsd) : "—"}
            sub={s ? `${formatUsd(s.today.estimatedCostUsd)} estimated · ${s.today.requests} calls` : undefined}
          />
          <Kpi
            label="This month"
            value={s ? formatUsd(s.month.costUsd) : "—"}
            sub={s ? `${formatTokens(s.month.inputTokens + s.month.outputTokens)} tokens · ${formatUsd(s.month.estimatedCostUsd)} est.` : undefined}
          />
          <Kpi
            label="Budget used"
            value={formatPct(used)}
            sub={used === null ? "No limited budget applies" : used >= 1 ? "Limit reached" : "Highest applicable budget"}
            tone={used === null ? undefined : used >= 1 ? "danger" : used >= 0.8 ? "warning" : undefined}
          />
          <Kpi
            label="All time"
            value={s ? formatUsd(s.total.costUsd) : "—"}
            sub={s ? `${s.total.requests} calls · ${s.total.failures} failed` : undefined}
          />
        </section>

        <section className="cost-card" aria-labelledby="cost-daily-title">
          <header className="cost-card__head">
            <h3 id="cost-daily-title">Daily spend · last 30 days</h3>
          </header>
          {series.loading && series.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> Loading…
            </div>
          ) : hasSpend ? (
            <SpendChart points={points} />
          ) : (
            <p className="cost-empty">No model usage recorded in the last 30 days. Usage appears here as agents and the Manager call providers.</p>
          )}
        </section>

        <section className="cost-card" aria-labelledby="cost-breakdown-title">
          <header className="cost-card__head">
            <h3 id="cost-breakdown-title">Breakdown</h3>
            <Tabs aria-label="Breakdown dimension" items={DIMENSIONS} value={dimension} onChange={setDimension} />
            <Tabs aria-label="Time window" items={WINDOWS} value={window} onChange={setWindow} />
          </header>
          {breakdown.loading && breakdown.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> Loading…
            </div>
          ) : rows.length === 0 ? (
            <p className="cost-empty">No usage in this window.</p>
          ) : (
            <div className="cost-table-wrap">
              <table className="cost-table">
                <thead>
                  <tr>
                    <th scope="col">{DIMENSIONS.find((d) => d.id === dimension)?.label}</th>
                    <th scope="col" className="num">
                      Cost
                    </th>
                    <th scope="col" className="num">
                      Estimated
                    </th>
                    <th scope="col" className="num">
                      Input
                    </th>
                    <th scope="col" className="num">
                      Output
                    </th>
                    <th scope="col" className="num">
                      Calls
                    </th>
                    <th scope="col" className="num">
                      Avg latency
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <tr key={r.key}>
                      <th scope="row" title={r.key}>
                        {labelFor(r)}
                      </th>
                      <td className="num">{formatUsd(r.totals.costUsd)}</td>
                      <td className="num cost-muted">{r.totals.estimatedCostUsd > 0 ? formatUsd(r.totals.estimatedCostUsd) : "—"}</td>
                      <td className="num">{formatTokens(r.totals.inputTokens)}</td>
                      <td className="num">{formatTokens(r.totals.outputTokens)}</td>
                      <td className="num">
                        {r.totals.requests}
                        {r.totals.failures > 0 && <span className="cost-fail"> ({r.totals.failures} failed)</span>}
                      </td>
                      <td className="num">{r.avgLatencyMs != null ? `${Math.round(r.avgLatencyMs)} ms` : "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="cost-card" aria-labelledby="cost-budgets-title">
          <header className="cost-card__head">
            <h3 id="cost-budgets-title">Budgets &amp; limits</h3>
            <div className="cost-toolbar__spacer" />
            <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditing("new")}>
              New budget
            </Button>
          </header>
          {budgets.loading && budgets.data === null ? (
            <div className="cost-loading">
              <Spinner size={16} /> Loading…
            </div>
          ) : (budgets.data ?? []).length === 0 ? (
            <p className="cost-empty">No budgets yet. Example: Project $100 per month, Task max $5, local models unrestricted.</p>
          ) : (
            <ul className="cost-budgets">
              {(budgets.data ?? []).map((b) => (
                <li key={b.budget.id} className={clsx("cost-budget", !b.budget.enabled && "cost-budget--disabled")}>
                  <div className="cost-budget__main">
                    <div className="cost-budget__title">
                      <strong>{b.budget.name ?? scopeLabel(b.budget.scope, projects)}</strong>
                      <Badge tone={LEVEL_TONE[b.level]}>{b.level === "Warn" ? "Warning" : b.level}</Badge>
                    </div>
                    <span className="cost-budget__desc">
                      {b.budget.name ? `${scopeLabel(b.budget.scope, projects)} · ` : ""}
                      {b.budget.unrestricted ? "unrestricted" : `${formatUsd(b.budget.limitUsd ?? 0)} ${PERIOD_LABEL[b.budget.period]}`}
                      {" · "}spent {formatUsd(b.spentUsd)}
                      {b.budget.period === "PerTask" ? " (top task)" : ""}
                    </span>
                  </div>
                  <Meter fraction={b.budget.unrestricted ? null : b.fraction} warnAt={b.budget.warnAt} />
                  <span className="cost-budget__pct">{b.budget.unrestricted ? "∞" : formatPct(b.fraction)}</span>
                  <div className="cost-budget__actions">
                    <IconButton size="sm" label="Edit budget" icon={<Pencil size={14} />} onClick={() => setEditing(b.budget)} />
                    <IconButton size="sm" variant="danger" label="Delete budget" icon={<Trash2 size={14} />} onClick={() => void removeBudget(b.budget)} />
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
