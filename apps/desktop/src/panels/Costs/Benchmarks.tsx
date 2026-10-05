import type { AgentGroupBy, AgentPerformanceRow, ModelBenchmarkRow } from "@orkestr/protocol";
import { Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useMemo, useState } from "react";

import { useFormat, useLanguage, useT } from "../../i18n";
import { useRpcQuery } from "../../lib/rpcQuery";
import {
  barFraction,
  benchmarkRoleLabel,
  LOWER_IS_BETTER,
  type ModelSortKey,
  type SortDir,
  sortModelRows,
} from "./benchmarksModel";
import "./benchmarks.css";
import { formatPct, formatTokens, formatUsd } from "./format";

const WINDOWS = ["7", "30", "90"] as const;
type WindowId = (typeof WINDOWS)[number];

const RELOAD_ON = ["AgentStopped", "TaskStatusChanged", "ReviewFinished", "TestRunFinished", "BuildFailed", "BuildSucceeded"] as const;

const MODEL_COLUMNS: ModelSortKey[] = ["model", "executions", "firstPass", "build", "defects", "retries", "latency", "tokens", "cost", "total", "score"];

function WindowTabs({ value, onChange }: { value: WindowId; onChange: (v: WindowId) => void }) {
  const { t } = useT("benchmarks");
  return (
    <Tabs
      aria-label={t("window.label")}
      items={WINDOWS.map((id) => ({ id, label: t("window.days", { count: Number(id) }) }))}
      value={value}
      onChange={onChange}
    />
  );
}

function Loading() {
  const { t } = useT("common");
  return (
    <div className="cost-loading">
      <Spinner size={16} /> {t("state.loading")}
    </div>
  );
}

function BarChart({ title, rows, value, format }: { title: string; rows: ModelBenchmarkRow[]; value: (r: ModelBenchmarkRow) => number | null; format: (v: number) => string }) {
  const values = rows.map(value);
  return (
    <figure className="bench-chart">
      <figcaption>{title}</figcaption>
      <ul>
        {rows.map((r, i) => {
          const v = values[i] ?? null;
          return (
            <li key={`${r.providerId}/${r.model}`}>
              <span className="bench-chart__label" title={r.model}>
                {r.model}
              </span>
              <span className="bench-chart__track" aria-hidden="true">
                <span className="bench-chart__bar" style={{ width: `${barFraction(v, values) * 100}%` }} />
              </span>
              <span className="bench-chart__value">{v === null ? "—" : format(v)}</span>
            </li>
          );
        })}
      </ul>
    </figure>
  );
}

/** Model benchmark table (§22): sortable, with small comparison charts. */
export function ModelBenchmarks({ projectId }: { projectId: string | null }) {
  const { t } = useT(["benchmarks", "common"]);
  const lng = useLanguage();
  const fmt = useFormat();
  const [win, setWin] = useState<WindowId>("30");
  const [role, setRole] = useState<string>("");
  const [sort, setSort] = useState<{ key: ModelSortKey; dir: SortDir }>({ key: "executions", dir: "desc" });
  const topics = projectId ? ["global", `project:${projectId}`] : ["*"];
  const query = useRpcQuery<ModelBenchmarkRow[]>(
    `bench-models:${projectId ?? "all"}:${win}:${role}`,
    async (c) =>
      (
        await c.call("benchmarks.models", {
          days: Number(win),
          ...(projectId ? { projectId } : {}),
          ...(role ? { role } : {}),
        })
      ).rows,
    { topics, reloadOn: RELOAD_ON },
  );
  const rows = query.data ?? [];
  const roles = useMemo(() => [...new Set(rows.flatMap((r) => r.roles))].sort(), [rows]);
  const sorted = useMemo(() => sortModelRows(rows, sort.key, sort.dir), [rows, sort]);
  const charted = sorted.filter((r) => r.executions > 0 || r.calls > 0).slice(0, 8);
  const pct = (v: number | null | undefined) => formatPct(v, lng);
  const dec = (v: number | null | undefined) => (v == null ? "—" : fmt.number(v, { maximumFractionDigits: 1 }));

  const toggle = (key: ModelSortKey) =>
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: LOWER_IS_BETTER.has(key) ? "asc" : "desc" }));

  return (
    <section className="cost-card" aria-labelledby="bench-models-title">
      <header className="cost-card__head">
        <h3 id="bench-models-title">{t("models.title")}</h3>
        <WindowTabs value={win} onChange={setWin} />
        <select className="bench-select" value={role} aria-label={t("role.label")} onChange={(e) => setRole(e.target.value)}>
          <option value="">{t("role.all")}</option>
          {(role && !roles.includes(role) ? [...roles, role] : roles).map((r) => (
            <option key={r} value={r}>
              {benchmarkRoleLabel(r)}
            </option>
          ))}
        </select>
        {query.loading && <Spinner size={14} />}
      </header>
      <p className="bench-hint">{t("models.hint")}</p>
      {query.error && (
        <p className="cost-error" role="alert">
          {query.error}
        </p>
      )}
      {query.loading && query.data === null ? (
        <Loading />
      ) : rows.length === 0 ? (
        <p className="cost-empty">{t("models.empty")}</p>
      ) : (
        <>
          <div className="bench-charts" role="group" aria-label={t("models.chartsLabel")}>
            <BarChart title={t("models.chartFirstPass")} rows={charted} value={(r) => r.firstPassSuccessRate} format={(v) => pct(v)} />
            <BarChart title={t("models.chartCost")} rows={charted} value={(r) => r.costAvgUsd} format={(v) => formatUsd(v, lng)} />
            <BarChart title={t("models.chartLatency")} rows={charted} value={(r) => r.latencyAvgMs} format={(v) => fmt.duration(Math.round(v))} />
          </div>
          <div className="cost-table-wrap">
            <table className="cost-table bench-table">
              <thead>
                <tr>
                  {MODEL_COLUMNS.map((key) => {
                    const label = t(`models.col.${key}`);
                    const active = sort.key === key;
                    return (
                      <th
                        key={key}
                        scope="col"
                        className={clsx(key !== "model" && "num")}
                        aria-sort={active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
                        title={key === "score" ? t("models.scoreHint") : undefined}
                      >
                        <button type="button" className="bench-sort" onClick={() => toggle(key)} aria-label={t("models.sortBy", { column: label })}>
                          {label}
                          {active && (sort.dir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
                        </button>
                      </th>
                    );
                  })}
                </tr>
              </thead>
              <tbody>
                {sorted.map((r) => (
                  <tr key={`${r.providerId}/${r.model}`}>
                    <th scope="row" title={`${r.providerId}/${r.model}`}>
                      <span className="bench-model">{r.model}</span>
                      <span className="bench-sub">
                        {t("models.calls", { count: Number(r.calls) })}
                        {r.callSuccessRate != null && ` · ${t("models.callSuccess", { pct: pct(r.callSuccessRate) })}`}
                      </span>
                      {r.roles.length > 0 && <span className="bench-sub">{r.roles.map(benchmarkRoleLabel).join(", ")}</span>}
                    </th>
                    <td className="num">{fmt.integer(Number(r.executions))}</td>
                    <td className="num">{pct(r.firstPassSuccessRate)}</td>
                    <td className="num">{pct(r.buildSuccessRate)}</td>
                    <td className="num">{dec(r.defectsFoundAvg)}</td>
                    <td className="num">{dec(r.retriesAvg)}</td>
                    <td className="num">
                      {r.latencyAvgMs != null ? fmt.duration(Math.round(r.latencyAvgMs)) : "—"}
                      {r.latencyP95Ms != null && <span className="cost-muted"> / {fmt.duration(Number(r.latencyP95Ms))}</span>}
                    </td>
                    <td className="num">{r.tokensAvg != null ? formatTokens(r.tokensAvg, lng) : "—"}</td>
                    <td className="num">{r.costAvgUsd != null ? formatUsd(r.costAvgUsd, lng) : "—"}</td>
                    <td className="num">{formatUsd(r.costTotalUsd, lng)}</td>
                    <td className="num">{r.routingScore != null ? fmt.number(r.routingScore, { maximumFractionDigits: 2 }) : "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </section>
  );
}

/** Agent performance analytics (§22) per template or instance. */
export function AgentPerformance({ projectId }: { projectId: string | null }) {
  const { t } = useT(["benchmarks", "common"]);
  const lng = useLanguage();
  const fmt = useFormat();
  const [win, setWin] = useState<WindowId>("30");
  const [groupBy, setGroupBy] = useState<AgentGroupBy>("Template");
  const topics = projectId ? ["global", `project:${projectId}`] : ["*"];
  const query = useRpcQuery<AgentPerformanceRow[]>(
    `bench-agents:${projectId ?? "all"}:${win}:${groupBy}`,
    async (c) => (await c.call("benchmarks.agents", { days: Number(win), groupBy, ...(projectId ? { projectId } : {}) })).rows,
    { topics, reloadOn: [...RELOAD_ON, "TaskCreated"] },
  );
  const rows = query.data ?? [];
  const pct = (v: number | null | undefined) => formatPct(v, lng);

  return (
    <section className="cost-card" aria-labelledby="bench-agents-title">
      <header className="cost-card__head">
        <h3 id="bench-agents-title">{t("agents.title")}</h3>
        <WindowTabs value={win} onChange={setWin} />
        <Tabs
          aria-label={t("groupBy.label")}
          items={(["Template", "Instance"] as const).map((id) => ({ id, label: t(`groupBy.${id}`) }))}
          value={groupBy}
          onChange={setGroupBy}
        />
        {query.loading && <Spinner size={14} />}
      </header>
      <p className="bench-hint">{t("agents.hint")}</p>
      {query.error && (
        <p className="cost-error" role="alert">
          {query.error}
        </p>
      )}
      {query.loading && query.data === null ? (
        <Loading />
      ) : rows.length === 0 ? (
        <p className="cost-empty">{t("agents.empty")}</p>
      ) : (
        <div className="cost-table-wrap">
          <table className="cost-table bench-table">
            <thead>
              <tr>
                {(["agent", "executions", "outcome", "firstPass", "rejection", "bugs", "retries", "duration", "cost", "models"] as const).map((k) => (
                  <th key={k} scope="col" className={clsx(k !== "agent" && k !== "models" && "num")}>
                    {t(`agents.col.${k}`)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.key}>
                  <th scope="row" title={r.key}>
                    <span className="bench-model">{r.name}</span>
                    <span className="bench-sub">{benchmarkRoleLabel(r.role)}</span>
                  </th>
                  <td className="num">{fmt.integer(Number(r.executions))}</td>
                  <td className="num">
                    <span className="bench-ok">{fmt.integer(Number(r.completed))}</span> / <span className={clsx(r.failed > 0 && "cost-fail")}>{fmt.integer(Number(r.failed))}</span>
                    {r.running > 0 && <span className="bench-sub">{t("agents.running", { count: Number(r.running) })}</span>}
                    {r.cancelled > 0 && <span className="bench-sub">{t("agents.cancelled", { count: Number(r.cancelled) })}</span>}
                  </td>
                  <td className="num">{pct(r.firstPassSuccessRate)}</td>
                  <td className="num">{pct(r.reviewRejectionRate)}</td>
                  <td className={clsx("num", r.postMergeBugs > 0 && "cost-fail")}>{fmt.integer(Number(r.postMergeBugs))}</td>
                  <td className="num">{fmt.integer(Number(r.retries))}</td>
                  <td className="num">{r.durationAvgMs != null ? fmt.duration(Math.round(r.durationAvgMs)) : "—"}</td>
                  <td className="num">{formatUsd(r.costUsd, lng)}</td>
                  <td className="bench-models">{r.models.join(", ") || "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
